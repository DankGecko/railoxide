use alloy::network::TransactionBuilder as _;
use alloy::primitives::{B256, U256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::BlockNumberOrTag;
use broadcaster_core::query_rpc_pool::QueryRpcPool;
use eyre::{Result, WrapErr, eyre};

use super::actions::{public_send_transaction_request, validate_public_transaction_intent};
use super::runtime::public_chain_runtime_config;
use super::types::{
    PublicActionGasFeeQuote, PublicActionGasFeeQuoteBundle, PublicActionGasFeeSelection,
    PublicActionKind, PublicActionProgressStep, PublicAdvancedTransactionEstimate,
    PublicAdvancedTransactionEstimateRequest, PublicAssetId, PublicShieldTransactionProfile,
    PublicTransactionIntent,
};
use crate::settings::EffectiveChainConfig;
use crate::{
    Eip1559GasCostProjection, GAS_LIMIT_BUFFER, HttpContext, RAILGUN_PROTOCOL_FEE_BPS,
    SelfBroadcastGasFeeQuote, SelfBroadcastResolvedGasFee, SelfBroadcastTipFallback,
    eip1559_gas_cost_projection, expected_eip1559_fee_per_gas, query_rpc_pool_with_http_client,
    railgun_protocol_fee_amount, resolve_self_broadcast_gas_fee,
    self_broadcast_gas_fee_quote_from_rpc_pool_with_tip_fallback,
};

pub(super) const PUBLIC_NATIVE_SEND_GAS_UNITS: u64 = 21_000;
const PUBLIC_ERC20_SEND_GAS_UNITS: u64 = 65_000;
pub(super) const PUBLIC_NATIVE_WRAP_GAS_UNITS: u64 = 50_000;
pub(super) const PUBLIC_NATIVE_APPROVE_GAS_UNITS: u64 = 65_000;
pub(super) const PUBLIC_NATIVE_SHIELD_GAS_UNITS: u64 = 650_000;
pub(super) const PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS: u64 = 900_000;
pub(super) const PUBLIC_RAILWAY_NATIVE_SHIELD_GAS_UNITS: u64 = 6_000_000;
const PUBLIC_ACTION_BNB_CHAIN_ID: u64 = 56;
const RAILWAY_FEE_HISTORY_BLOCKS: u64 = 10;
const RAILWAY_FEE_HISTORY_REWARD_PERCENTILES: [f64; 4] = [40.0, 60.0, 80.0, 95.0];
const RAILWAY_BNB_GAS_PRICE_CAP: u128 = 50_000_000;

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
) -> Result<Eip1559GasCostProjection> {
    estimate_public_action_gas_cost_with_profile(
        chain_id,
        effective_chain,
        kind,
        asset,
        PublicShieldTransactionProfile::Railoxide,
        gas_fee,
        quote,
    )
}

pub fn estimate_public_action_gas_cost_with_profile(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    kind: PublicActionKind,
    asset: PublicAssetId,
    profile: PublicShieldTransactionProfile,
    gas_fee: PublicActionGasFeeSelection,
    quote: Option<PublicActionGasFeeQuote>,
) -> Result<Eip1559GasCostProjection> {
    estimate_public_action_gas_cost_with_profile_and_ceiling(
        chain_id,
        effective_chain,
        kind,
        asset,
        profile,
        gas_fee,
        quote,
        None,
    )
}

pub fn estimate_public_action_gas_cost_with_profile_and_ceiling(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    kind: PublicActionKind,
    asset: PublicAssetId,
    profile: PublicShieldTransactionProfile,
    gas_fee: PublicActionGasFeeSelection,
    quote: Option<PublicActionGasFeeQuote>,
    authorization_ceiling: Option<PublicActionGasFeeSelection>,
) -> Result<Eip1559GasCostProjection> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let resolved = resolve_public_action_gas_fee(chain_id, profile, gas_fee, quote)?;
    let maximum_resolved = authorization_ceiling.map_or(Ok(resolved), |ceiling| {
        resolve_public_action_gas_fee(chain_id, profile, ceiling, None)
    })?;
    let expected_gas_units = public_action_estimated_gas_usage_units(kind, asset);
    let maximum_gas_units = public_action_estimated_gas_units_with_buffer(
        kind,
        asset,
        profile,
        chain.gas.gas_limit_buffer,
    );
    if profile.uses_legacy_envelope(chain_id) {
        return Ok(legacy_gas_cost_projection(
            expected_gas_units,
            maximum_gas_units,
            resolved.max_fee_per_gas,
            maximum_resolved.max_fee_per_gas,
        ));
    }
    let quote = quote
        .unwrap_or_else(|| SelfBroadcastGasFeeQuote::from_rpc_gas_price(resolved.max_fee_per_gas));
    Ok(public_action_eip1559_gas_cost_projection(
        expected_gas_units,
        maximum_gas_units,
        quote,
        resolved.max_fee_per_gas,
        resolved.max_priority_fee_per_gas,
        maximum_resolved.max_fee_per_gas,
    ))
}

#[must_use]
fn public_action_eip1559_gas_cost_projection(
    expected_gas_units: u64,
    maximum_gas_units: u64,
    quote: PublicActionGasFeeQuote,
    expected_max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    maximum_fee_per_gas: u128,
) -> Eip1559GasCostProjection {
    let expected_fee_per_gas =
        expected_eip1559_fee_per_gas(quote, expected_max_fee_per_gas, max_priority_fee_per_gas);
    Eip1559GasCostProjection {
        expected_fee_per_gas,
        maximum_fee_per_gas,
        expected_cost: U256::from(expected_gas_units) * U256::from(expected_fee_per_gas),
        maximum_cost: U256::from(maximum_gas_units) * U256::from(maximum_fee_per_gas),
    }
}

#[must_use]
fn legacy_gas_cost_projection(
    expected_gas_units: u64,
    maximum_gas_units: u64,
    expected_gas_price: u128,
    maximum_gas_price: u128,
) -> Eip1559GasCostProjection {
    Eip1559GasCostProjection {
        expected_fee_per_gas: expected_gas_price,
        maximum_fee_per_gas: maximum_gas_price,
        expected_cost: U256::from(expected_gas_units) * U256::from(expected_gas_price),
        maximum_cost: U256::from(maximum_gas_units) * U256::from(maximum_gas_price),
    }
}

pub(super) fn resolve_public_action_gas_fee(
    chain_id: u64,
    profile: PublicShieldTransactionProfile,
    gas_fee: PublicActionGasFeeSelection,
    quote: Option<PublicActionGasFeeQuote>,
) -> Result<SelfBroadcastResolvedGasFee> {
    let quote = match quote {
        Some(quote) => quote,
        None => match gas_fee {
            PublicActionGasFeeSelection::Custom {
                max_fee_per_gas, ..
            } => SelfBroadcastGasFeeQuote::from_rpc_gas_price(max_fee_per_gas),
            PublicActionGasFeeSelection::Auto => {
                return Err(eyre!("public action gas fee quote is not ready"));
            }
        },
    };
    let resolved = resolve_self_broadcast_gas_fee(gas_fee, quote)?;
    if !profile.uses_legacy_envelope(chain_id) {
        return Ok(resolved);
    }

    let gas_price = match gas_fee {
        PublicActionGasFeeSelection::Auto => quote.rpc_gas_price,
        PublicActionGasFeeSelection::Custom {
            max_fee_per_gas, ..
        } => max_fee_per_gas,
    };
    if gas_price == 0 {
        return Err(eyre!("legacy gas price must be greater than zero"));
    }
    Ok(SelfBroadcastResolvedGasFee {
        rpc_gas_price: quote.rpc_gas_price,
        max_fee_per_gas: gas_price,
        max_priority_fee_per_gas: 0,
    })
}

#[must_use]
pub fn public_shield_protocol_fee_amount(amount: U256) -> U256 {
    railgun_protocol_fee_amount(amount, RAILGUN_PROTOCOL_FEE_BPS)
}

fn public_action_estimated_gas_units_with_buffer(
    kind: PublicActionKind,
    asset: PublicAssetId,
    profile: PublicShieldTransactionProfile,
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
            PublicAssetId::Native => match profile {
                PublicShieldTransactionProfile::Railway => PUBLIC_RAILWAY_NATIVE_SHIELD_GAS_UNITS,
                PublicShieldTransactionProfile::Railoxide => {
                    PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS.saturating_add(gas_limit_buffer)
                }
            },
            PublicAssetId::Erc20(_) => match profile {
                PublicShieldTransactionProfile::Railway => {
                    railway_gas_limit(PUBLIC_NATIVE_APPROVE_GAS_UNITS)
                        .saturating_add(railway_gas_limit(PUBLIC_NATIVE_SHIELD_GAS_UNITS))
                }
                PublicShieldTransactionProfile::Railoxide => PUBLIC_NATIVE_APPROVE_GAS_UNITS
                    .saturating_add(gas_limit_buffer)
                    .saturating_add(PUBLIC_NATIVE_SHIELD_GAS_UNITS)
                    .saturating_add(gas_limit_buffer),
            },
        },
    }
}

pub(super) const fn public_action_estimated_gas_usage_units(
    kind: PublicActionKind,
    asset: PublicAssetId,
) -> u64 {
    match kind {
        PublicActionKind::Send => match asset {
            PublicAssetId::Native => PUBLIC_NATIVE_SEND_GAS_UNITS,
            PublicAssetId::Erc20(_) => PUBLIC_ERC20_SEND_GAS_UNITS,
        },
        PublicActionKind::Shield => match asset {
            PublicAssetId::Native => PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS,
            PublicAssetId::Erc20(_) => {
                PUBLIC_NATIVE_APPROVE_GAS_UNITS + PUBLIC_NATIVE_SHIELD_GAS_UNITS
            }
        },
    }
}

#[must_use]
fn public_native_action_gas_reserve_with_buffer(
    max_fee_per_gas: u128,
    steps: &[PublicActionProgressStep],
    gas_limit_buffer: u64,
) -> U256 {
    public_native_action_gas_reserve_with_profile(
        max_fee_per_gas,
        steps,
        PublicShieldTransactionProfile::Railoxide,
        gas_limit_buffer,
    )
}

#[must_use]
pub(super) fn public_native_action_gas_reserve_with_profile(
    max_fee_per_gas: u128,
    steps: &[PublicActionProgressStep],
    profile: PublicShieldTransactionProfile,
    gas_limit_buffer: u64,
) -> U256 {
    U256::from(public_native_action_gas_units_with_profile(
        steps,
        profile,
        gas_limit_buffer,
    )) * U256::from(max_fee_per_gas)
}

#[must_use]
fn public_native_action_gas_units_with_profile(
    steps: &[PublicActionProgressStep],
    profile: PublicShieldTransactionProfile,
    gas_limit_buffer: u64,
) -> u64 {
    steps.iter().fold(0_u64, |total, step| {
        let gas_units = if profile == PublicShieldTransactionProfile::Railway
            && *step == PublicActionProgressStep::Shield
        {
            PUBLIC_RAILWAY_NATIVE_SHIELD_GAS_UNITS
        } else {
            public_native_step_gas_units(*step)
        };
        if gas_units == 0 {
            total
        } else if profile == PublicShieldTransactionProfile::Railway
            && *step == PublicActionProgressStep::Shield
        {
            total.saturating_add(gas_units)
        } else {
            total.saturating_add(gas_units.saturating_add(gas_limit_buffer))
        }
    })
}

#[must_use]
pub(super) fn railway_gas_limit(estimated_gas: u64) -> u64 {
    let multiplied = u128::from(estimated_gas) * 120 / 100;
    multiplied.min(u128::from(u64::MAX)) as u64
}

pub async fn quote_public_action_gas_fee(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    http: &HttpContext,
) -> Result<PublicActionGasFeeQuote> {
    quote_public_action_gas_fee_with_profile(
        chain_id,
        effective_chain,
        PublicShieldTransactionProfile::Railoxide,
        http,
    )
    .await
}

pub async fn quote_public_action_gas_fee_with_profile(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    profile: PublicShieldTransactionProfile,
    http: &HttpContext,
) -> Result<PublicActionGasFeeQuote> {
    Ok(
        quote_public_action_gas_fee_bundle_with_profile(chain_id, effective_chain, profile, http)
            .await?
            .standard,
    )
}

pub async fn quote_public_action_gas_fee_bundle_with_profile(
    chain_id: u64,
    effective_chain: Option<&EffectiveChainConfig>,
    profile: PublicShieldTransactionProfile,
    http: &HttpContext,
) -> Result<PublicActionGasFeeQuoteBundle> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    public_action_gas_fee_quote_bundle_from_rpc_pool_with_profile(
        &query_rpc_pool,
        http.network_mode(),
        chain_id,
        profile,
    )
    .await
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
    let resolved = resolve_public_action_gas_fee(
        request.chain_id,
        PublicShieldTransactionProfile::Railoxide,
        request.gas_fee,
        Some(quote),
    )?;
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
                let gas_cost = eip1559_gas_cost_projection(
                    gas_limit,
                    quote,
                    resolved.max_fee_per_gas,
                    resolved.max_priority_fee_per_gas,
                );
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
                    expected_fee_per_gas: gas_cost.expected_fee_per_gas,
                    expected_gas_cost: gas_cost.expected_cost,
                    max_gas_cost: gas_cost.maximum_cost,
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
    estimate_public_native_action_gas_reserve_with_profile_and_ceiling(
        chain_id,
        steps,
        PublicShieldTransactionProfile::Railoxide,
        effective_chain,
        gas_fee,
        http,
        None,
    )
    .await
}

pub async fn estimate_public_native_action_gas_reserve_with_profile_and_ceiling(
    chain_id: u64,
    steps: &[PublicActionProgressStep],
    profile: PublicShieldTransactionProfile,
    effective_chain: Option<&EffectiveChainConfig>,
    gas_fee: PublicActionGasFeeSelection,
    http: &HttpContext,
    authorization_ceiling: Option<PublicActionGasFeeSelection>,
) -> Result<U256> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    let quote_bundle = public_action_gas_fee_quote_bundle_from_rpc_pool_with_profile(
        &query_rpc_pool,
        http.network_mode(),
        chain_id,
        profile,
    )
    .await
    .wrap_err("fetch public action gas price")?;
    let quote = quote_bundle.standard;
    let gas = resolve_public_action_gas_fee(chain_id, profile, gas_fee, Some(quote))?;
    let maximum_gas = authorization_ceiling.map_or(Ok(gas), |ceiling| {
        resolve_public_action_gas_fee(chain_id, profile, ceiling, None)
    })?;
    Ok(public_native_action_gas_reserve_with_profile(
        maximum_gas.max_fee_per_gas,
        steps,
        profile,
        chain.gas.gas_limit_buffer,
    ))
}

pub async fn estimate_public_native_action_gas_reserve_with_profile(
    chain_id: u64,
    steps: &[PublicActionProgressStep],
    profile: PublicShieldTransactionProfile,
    effective_chain: Option<&EffectiveChainConfig>,
    gas_fee: PublicActionGasFeeSelection,
    http: &HttpContext,
) -> Result<U256> {
    estimate_public_native_action_gas_reserve_with_profile_and_ceiling(
        chain_id,
        steps,
        profile,
        effective_chain,
        gas_fee,
        http,
        None,
    )
    .await
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

pub(super) async fn public_action_gas_fee_quote_from_rpc_pool_with_profile(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
    profile: PublicShieldTransactionProfile,
) -> Result<PublicActionGasFeeQuote> {
    Ok(
        public_action_gas_fee_quote_bundle_from_rpc_pool_with_profile(
            query_rpc_pool,
            network_mode,
            chain_id,
            profile,
        )
        .await?
        .standard,
    )
}

pub(super) async fn public_action_gas_fee_quote_bundle_from_rpc_pool_with_profile(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
    profile: PublicShieldTransactionProfile,
) -> Result<PublicActionGasFeeQuoteBundle> {
    if profile == PublicShieldTransactionProfile::Railoxide {
        let standard =
            public_action_gas_fee_quote_from_rpc_pool(query_rpc_pool, network_mode, chain_id)
                .await?;
        return Ok(public_action_gas_fee_quote_bundle_from_standard(standard));
    }
    let providers = query_rpc_pool.available_providers();
    if providers.is_empty() {
        return Err(eyre!("no healthy query RPC available"));
    }
    for provider_handle in providers {
        match railway_gas_fee_quote_from_provider(&provider_handle.provider, chain_id).await {
            Ok(quote) => return Ok(quote),
            Err(_) => {
                tracing::debug!("Railway gas fee quote provider attempt failed");
            }
        }
    }
    Err(eyre!("all Railway gas quote RPC attempts failed"))
}

async fn railway_gas_fee_quote_from_provider(
    provider: &impl Provider,
    chain_id: u64,
) -> Result<PublicActionGasFeeQuoteBundle> {
    if chain_id == PUBLIC_ACTION_BNB_CHAIN_ID {
        let provider_gas_price = provider
            .get_gas_price()
            .await
            .wrap_err("fetch Railway BNB gas price")?;
        return Ok(railway_bnb_gas_fee_quote_bundle(provider_gas_price));
    }
    let fee_history = provider
        .get_fee_history(
            RAILWAY_FEE_HISTORY_BLOCKS,
            BlockNumberOrTag::Latest,
            &RAILWAY_FEE_HISTORY_REWARD_PERCENTILES,
        )
        .await
        .wrap_err("fetch Railway fee history")?;
    railway_standard_gas_fee_quote_bundle(
        &fee_history.base_fee_per_gas,
        fee_history.reward.as_deref(),
    )
}

#[cfg(test)]
pub(super) fn railway_standard_gas_fee_quote(
    base_fee_per_gas: &[u128],
    rewards: Option<&[Vec<u128>]>,
) -> Result<PublicActionGasFeeQuote> {
    Ok(railway_standard_gas_fee_quote_bundle(base_fee_per_gas, rewards)?.standard)
}

pub(super) fn railway_standard_gas_fee_quote_bundle(
    base_fee_per_gas: &[u128],
    rewards: Option<&[Vec<u128>]>,
) -> Result<PublicActionGasFeeQuoteBundle> {
    let next_base_fee_per_gas = base_fee_per_gas
        .last()
        .copied()
        .ok_or_else(|| eyre!("Railway fee history returned no base fee"))?;
    let rewards = rewards.ok_or_else(|| eyre!("Railway fee history returned no rewards"))?;
    let mut priority_fees = rewards
        .iter()
        .map(|reward| reward.get(1).copied())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| eyre!("Railway fee history reward columns are incomplete"))?;
    let max_priority_fee_per_gas = railway_lower_median(&mut priority_fees)
        .ok_or_else(|| eyre!("Railway fee history returned no priority fees"))?;
    let mut aggressive_priority_fees = rewards
        .iter()
        .map(|reward| reward.get(3).copied())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| eyre!("Railway fee history reward columns are incomplete"))?;
    let aggressive_priority_fee_per_gas = railway_lower_median(&mut aggressive_priority_fees)
        .ok_or_else(|| eyre!("Railway fee history returned no priority fees"))?;
    let max_base_fee_per_gas = next_base_fee_per_gas
        .checked_mul(110)
        .ok_or_else(|| eyre!("Railway base fee overflow"))?
        / 100;
    let max_fee_per_gas = max_base_fee_per_gas
        .checked_add(max_priority_fee_per_gas)
        .ok_or_else(|| eyre!("Railway max fee overflow"))?;
    let aggressive_max_base_fee_per_gas = next_base_fee_per_gas
        .checked_mul(140)
        .ok_or_else(|| eyre!("Railway aggressive base fee overflow"))?
        / 100;
    let aggressive_max_fee_per_gas = aggressive_max_base_fee_per_gas
        .checked_add(aggressive_priority_fee_per_gas)
        .ok_or_else(|| eyre!("Railway aggressive max fee overflow"))?;
    let standard = PublicActionGasFeeQuote {
        rpc_gas_price: max_fee_per_gas,
        current_base_fee_per_gas: base_fee_per_gas
            .len()
            .checked_sub(2)
            .and_then(|index| base_fee_per_gas.get(index))
            .copied(),
        suggested_max_fee_per_gas: max_fee_per_gas,
        suggested_max_priority_fee_per_gas: max_priority_fee_per_gas,
    };
    Ok(PublicActionGasFeeQuoteBundle {
        standard,
        authorization_ceiling: PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: aggressive_max_fee_per_gas,
            max_priority_fee_per_gas: aggressive_priority_fee_per_gas,
        },
    })
}

#[cfg(test)]
pub(super) fn railway_bnb_gas_fee_quote(provider_gas_price: u128) -> PublicActionGasFeeQuote {
    railway_bnb_gas_fee_quote_bundle(provider_gas_price).standard
}

pub(super) fn railway_bnb_gas_fee_quote_bundle(
    provider_gas_price: u128,
) -> PublicActionGasFeeQuoteBundle {
    let capped_gas_price = provider_gas_price.min(RAILWAY_BNB_GAS_PRICE_CAP);
    let standard_gas_price = capped_gas_price * 110 / 100;
    let aggressive_gas_price = capped_gas_price * 140 / 100;
    let standard = PublicActionGasFeeQuote {
        rpc_gas_price: standard_gas_price,
        current_base_fee_per_gas: None,
        suggested_max_fee_per_gas: standard_gas_price,
        suggested_max_priority_fee_per_gas: 0,
    };
    PublicActionGasFeeQuoteBundle {
        standard,
        authorization_ceiling: PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: aggressive_gas_price,
            max_priority_fee_per_gas: 0,
        },
    }
}

const fn public_action_gas_fee_quote_bundle_from_standard(
    standard: PublicActionGasFeeQuote,
) -> PublicActionGasFeeQuoteBundle {
    PublicActionGasFeeQuoteBundle {
        authorization_ceiling: PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: standard.suggested_max_fee_per_gas,
            max_priority_fee_per_gas: standard.suggested_max_priority_fee_per_gas,
        },
        standard,
    }
}

pub(super) fn railway_lower_median(values: &mut [u128]) -> Option<u128> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    values.get((values.len() - 1) / 2).copied()
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
