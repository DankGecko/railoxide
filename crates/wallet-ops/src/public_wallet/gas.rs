use alloy::network::TransactionBuilder as _;
use alloy::primitives::{B256, U256, keccak256};
use alloy::providers::Provider;
use broadcaster_core::query_rpc_pool::QueryRpcPool;
use eyre::{Result, WrapErr, eyre};

use super::actions::{public_send_transaction_request, validate_public_transaction_intent};
use super::runtime::public_chain_runtime_config;
use super::types::{
    PublicActionGasFeeQuote, PublicActionGasFeeSelection, PublicActionKind,
    PublicActionProgressStep, PublicAdvancedTransactionEstimate,
    PublicAdvancedTransactionEstimateRequest, PublicAssetId, PublicTransactionIntent,
};
use crate::settings::EffectiveChainConfig;
use crate::{
    GAS_LIMIT_BUFFER, HttpContext, RAILGUN_PROTOCOL_FEE_BPS, SelfBroadcastTipFallback,
    query_rpc_pool_with_http_client, railgun_protocol_fee_amount, resolve_self_broadcast_gas_fee,
    self_broadcast_gas_fee_quote_from_rpc_pool_with_tip_fallback,
};

pub(super) const PUBLIC_NATIVE_SEND_GAS_UNITS: u64 = 21_000;
const PUBLIC_ERC20_SEND_GAS_UNITS: u64 = 65_000;
pub(super) const PUBLIC_NATIVE_WRAP_GAS_UNITS: u64 = 50_000;
pub(super) const PUBLIC_NATIVE_APPROVE_GAS_UNITS: u64 = 65_000;
pub(super) const PUBLIC_NATIVE_SHIELD_GAS_UNITS: u64 = 650_000;
pub(super) const PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS: u64 = 800_000;
const PUBLIC_ACTION_BNB_CHAIN_ID: u64 = 56;

#[must_use]
pub fn public_native_action_gas_units(steps: &[PublicActionProgressStep]) -> u64 {
    public_native_action_gas_units_with_buffer(steps, GAS_LIMIT_BUFFER)
}

#[must_use]
pub(super) fn public_native_action_gas_units_with_buffer(
    steps: &[PublicActionProgressStep],
    gas_limit_buffer: u64,
) -> u64 {
    steps.iter().fold(0_u64, |total, step| {
        let gas_units = public_native_step_gas_units(*step);
        if gas_units == 0 {
            total
        } else {
            total.saturating_add(gas_units + gas_limit_buffer)
        }
    })
}

#[must_use]
pub fn public_native_action_gas_reserve(
    max_fee_per_gas: u128,
    steps: &[PublicActionProgressStep],
) -> U256 {
    public_native_action_gas_reserve_with_buffer(max_fee_per_gas, steps, GAS_LIMIT_BUFFER)
}

pub fn estimate_public_action_gas_cost(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    kind: PublicActionKind,
    asset: PublicAssetId,
    gas_fee: PublicActionGasFeeSelection,
    quote: Option<PublicActionGasFeeQuote>,
) -> Result<U256> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let max_fee_per_gas = match gas_fee {
        PublicActionGasFeeSelection::Auto => {
            quote
                .ok_or_else(|| eyre!("public action gas fee quote is not ready"))?
                .suggested_max_fee_per_gas
        }
        PublicActionGasFeeSelection::Custom {
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } => {
            if max_fee_per_gas == 0 {
                return Err(eyre!("max fee per gas must be greater than zero"));
            }
            if max_priority_fee_per_gas > max_fee_per_gas {
                return Err(eyre!(
                    "max priority fee per gas cannot exceed max fee per gas"
                ));
            }
            max_fee_per_gas
        }
    };
    let gas_units =
        public_action_estimated_gas_units_with_buffer(kind, asset, chain.gas.gas_limit_buffer);
    Ok(U256::from(gas_units) * U256::from(max_fee_per_gas))
}

#[must_use]
pub fn public_shield_protocol_fee_amount(amount: U256) -> U256 {
    railgun_protocol_fee_amount(amount, RAILGUN_PROTOCOL_FEE_BPS)
}

const fn public_action_estimated_gas_units_with_buffer(
    kind: PublicActionKind,
    asset: PublicAssetId,
    gas_limit_buffer: u64,
) -> u64 {
    match kind {
        PublicActionKind::Send => {
            let gas_units = match asset {
                PublicAssetId::Native => PUBLIC_NATIVE_SEND_GAS_UNITS,
                PublicAssetId::Erc20(_) => PUBLIC_ERC20_SEND_GAS_UNITS,
            };
            gas_units.saturating_add(gas_limit_buffer)
        }
        PublicActionKind::Shield => match asset {
            PublicAssetId::Native => {
                PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS.saturating_add(gas_limit_buffer)
            }
            PublicAssetId::Erc20(_) => PUBLIC_NATIVE_APPROVE_GAS_UNITS
                .saturating_add(gas_limit_buffer)
                .saturating_add(PUBLIC_NATIVE_SHIELD_GAS_UNITS)
                .saturating_add(gas_limit_buffer),
        },
    }
}

#[must_use]
fn public_native_action_gas_reserve_with_buffer(
    max_fee_per_gas: u128,
    steps: &[PublicActionProgressStep],
    gas_limit_buffer: u64,
) -> U256 {
    U256::from(public_native_action_gas_units_with_buffer(
        steps,
        gas_limit_buffer,
    )) * U256::from(max_fee_per_gas)
}

pub async fn quote_public_action_gas_fee(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    http: &HttpContext,
) -> Result<PublicActionGasFeeQuote> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    public_action_gas_fee_quote_from_rpc_pool(&query_rpc_pool, http.network_mode(), chain_id).await
}

pub async fn estimate_public_advanced_transaction(
    request: PublicAdvancedTransactionEstimateRequest,
    http: &HttpContext,
) -> Result<PublicAdvancedTransactionEstimate> {
    validate_public_transaction_intent(&request.intent)?;
    if !matches!(request.intent, PublicTransactionIntent::Raw { .. }) {
        return Err(eyre!(
            "advanced gas estimation requires a raw transaction intent"
        ));
    }
    let chain = public_chain_runtime_config(request.chain_id, request.effective_chain.as_ref())?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    let quote = public_action_gas_fee_quote_from_rpc_pool(
        &query_rpc_pool,
        http.network_mode(),
        request.chain_id,
    )
    .await
    .wrap_err("fetch advanced public transaction gas price")?;
    let resolved = resolve_self_broadcast_gas_fee(request.gas_fee, quote)?;
    let tx_req = public_send_transaction_request(request.chain_id, request.from, &request.intent)?
        .with_max_fee_per_gas(resolved.max_fee_per_gas)
        .with_max_priority_fee_per_gas(resolved.max_priority_fee_per_gas);
    let mut last_error = None;
    for _ in 0..query_rpc_pool.len() {
        let Some(provider_handle) = query_rpc_pool.random_provider() else {
            break;
        };
        match provider_handle.provider.estimate_gas(tx_req.clone()).await {
            Ok(estimated_gas) => {
                let gas_limit =
                    buffered_advanced_gas_limit(estimated_gas, chain.gas.gas_limit_buffer);
                return Ok(PublicAdvancedTransactionEstimate {
                    payload_fingerprint: public_advanced_transaction_payload_fingerprint(
                        request.chain_id,
                        request.from,
                        &request.intent,
                        resolved.max_fee_per_gas,
                        resolved.max_priority_fee_per_gas,
                    ),
                    gas_limit,
                    max_fee_per_gas: resolved.max_fee_per_gas,
                    max_priority_fee_per_gas: resolved.max_priority_fee_per_gas,
                    max_gas_cost: U256::from(gas_limit) * U256::from(resolved.max_fee_per_gas),
                });
            }
            Err(error) => {
                tracing::warn!(%error, "advanced public transaction gas estimate failed");
                last_error = Some(error);
            }
        }
    }
    if let Some(error) = last_error {
        Err(eyre!(error)).wrap_err("all advanced public transaction query RPC attempts failed")
    } else {
        Err(eyre!("no healthy query RPC available"))
    }
}

pub(super) const fn buffered_advanced_gas_limit(estimated_gas: u64, buffer: u64) -> u64 {
    estimated_gas.saturating_add(buffer)
}

pub(super) fn public_advanced_transaction_payload_fingerprint(
    chain_id: u64,
    from: alloy::primitives::Address,
    intent: &PublicTransactionIntent,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
) -> B256 {
    let mut encoded = b"railoxide:public-advanced-transaction:v1".to_vec();
    encoded.extend_from_slice(&chain_id.to_be_bytes());
    encoded.extend_from_slice(from.as_slice());
    match intent {
        PublicTransactionIntent::Transfer {
            asset,
            amount,
            recipient,
        } => {
            encoded.push(0);
            match asset {
                PublicAssetId::Native => encoded.push(0),
                PublicAssetId::Erc20(token) => {
                    encoded.push(1);
                    encoded.extend_from_slice(token.as_slice());
                }
            }
            encoded.extend_from_slice(&amount.to_be_bytes::<32>());
            encoded.extend_from_slice(recipient.as_slice());
        }
        PublicTransactionIntent::Raw { to, value, data } => {
            encoded.push(1);
            match to {
                Some(to) => {
                    encoded.push(1);
                    encoded.extend_from_slice(to.as_slice());
                }
                None => encoded.push(0),
            }
            encoded.extend_from_slice(&value.to_be_bytes::<32>());
            encoded.extend_from_slice(&(data.len() as u64).to_be_bytes());
            encoded.extend_from_slice(data);
        }
    }
    encoded.extend_from_slice(&max_fee_per_gas.to_be_bytes());
    encoded.extend_from_slice(&max_priority_fee_per_gas.to_be_bytes());
    keccak256(encoded)
}

pub async fn estimate_public_native_action_gas_reserve(
    chain_id: u64,
    steps: &[PublicActionProgressStep],
    effective_chain: Option<&EffectiveChainConfig>,
    gas_fee: PublicActionGasFeeSelection,
    http: &HttpContext,
) -> Result<U256> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    let quote =
        public_action_gas_fee_quote_from_rpc_pool(&query_rpc_pool, http.network_mode(), chain_id)
            .await
            .wrap_err("fetch public action gas price")?;
    let gas = resolve_self_broadcast_gas_fee(gas_fee, quote)?;
    Ok(public_native_action_gas_reserve_with_buffer(
        gas.max_fee_per_gas,
        steps,
        chain.gas.gas_limit_buffer,
    ))
}

pub(super) async fn public_action_gas_fee_quote_from_rpc_pool(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
) -> Result<PublicActionGasFeeQuote> {
    self_broadcast_gas_fee_quote_from_rpc_pool_with_tip_fallback(
        query_rpc_pool,
        network_mode,
        public_action_tip_fallback(chain_id),
    )
    .await
}

pub(super) const fn public_action_tip_fallback(chain_id: u64) -> SelfBroadcastTipFallback {
    if chain_id == PUBLIC_ACTION_BNB_CHAIN_ID {
        SelfBroadcastTipFallback::RpcGasPrice
    } else {
        SelfBroadcastTipFallback::Minimum
    }
}

const fn public_native_step_gas_units(step: PublicActionProgressStep) -> u64 {
    match step {
        PublicActionProgressStep::ShieldKey => 0,
        PublicActionProgressStep::Send => PUBLIC_NATIVE_SEND_GAS_UNITS,
        PublicActionProgressStep::Wrap => PUBLIC_NATIVE_WRAP_GAS_UNITS,
        PublicActionProgressStep::Approve => PUBLIC_NATIVE_APPROVE_GAS_UNITS,
        PublicActionProgressStep::Shield => PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS,
    }
}
