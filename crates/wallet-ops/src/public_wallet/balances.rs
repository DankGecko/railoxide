use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::SystemTime;

use alloy::primitives::{Address, U256};
use alloy::providers::{CallItem, Provider};
use alloy::sol_types::SolCall;
use eyre::{Result, eyre};
use railgun_ui::{chain_name, known_tokens_for_chain};

use super::contracts::{Multicall3Balance, PublicErc20};
use super::runtime::public_chain_runtime_config;
use super::types::{
    PlannedPublicBalanceCall, PublicAccountBalance, PublicAssetId, PublicBalanceAmount,
    PublicBalanceAsset, PublicBalanceEntry, PublicBalanceSnapshot,
};
use crate::http::redact_url_for_display;
use crate::settings::{EffectiveChainConfig, EffectiveTokenRegistry};
use crate::vault::PublicAccountMetadata;
use crate::{HttpContext, query_rpc_pool_with_http_client};

const PUBLIC_BALANCE_REFRESH_INTERVAL_SECS: u64 = 60;

#[must_use]
pub const fn public_balance_refresh_interval_secs() -> u64 {
    PUBLIC_BALANCE_REFRESH_INTERVAL_SECS
}

#[must_use]
pub fn public_balance_assets_for_chain(chain_id: u64) -> Vec<PublicBalanceAsset> {
    public_balance_assets_for_chain_with_registry(chain_id, None)
}

#[must_use]
pub(super) fn public_balance_assets_for_chain_with_registry(
    chain_id: u64,
    token_registry: Option<&EffectiveTokenRegistry>,
) -> Vec<PublicBalanceAsset> {
    let mut assets = Vec::new();
    if let Some(native) = native_asset_for_chain(chain_id) {
        assets.push(native);
    }
    if let Some(token_registry) = token_registry {
        assets.extend(
            token_registry
                .tokens
                .values()
                .filter(|token| token.chain_id == chain_id)
                .filter_map(|token| {
                    Address::from_str(&token.token_address)
                        .ok()
                        .map(|address| PublicBalanceAsset {
                            id: PublicAssetId::Erc20(address),
                            symbol: token.symbol.clone(),
                            decimals: token.decimals,
                        })
                }),
        );
    } else {
        assets.extend(
            known_tokens_for_chain(chain_id).map(|token| PublicBalanceAsset {
                id: PublicAssetId::Erc20(token.token),
                symbol: token.symbol.to_string(),
                decimals: token.decimals,
            }),
        );
    }
    assets
}

pub(super) fn plan_public_balance_calls(
    chain_id: u64,
    multicall_addr: Address,
    accounts: &[PublicAccountMetadata],
    token_registry: Option<&EffectiveTokenRegistry>,
) -> Vec<PlannedPublicBalanceCall> {
    let assets = public_balance_assets_for_chain_with_registry(chain_id, token_registry);
    let mut calls = Vec::with_capacity(accounts.len().saturating_mul(assets.len()));
    for account in accounts {
        for asset in &assets {
            let (target, data) = match asset.id {
                PublicAssetId::Native => (
                    multicall_addr,
                    Multicall3Balance::getEthBalanceCall {
                        addr: account.address,
                    }
                    .abi_encode(),
                ),
                PublicAssetId::Erc20(token) => (
                    token,
                    PublicErc20::balanceOfCall {
                        account: account.address,
                    }
                    .abi_encode(),
                ),
            };
            calls.push(PlannedPublicBalanceCall {
                public_account_uuid: account.public_account_uuid.clone(),
                account: account.address,
                asset: asset.clone(),
                target,
                data,
            });
        }
    }
    calls
}

pub async fn refresh_public_balances(
    chain_id: u64,
    accounts: &[PublicAccountMetadata],
    effective_chain: Option<&EffectiveChainConfig>,
    token_registry: Option<&EffectiveTokenRegistry>,
    http: &HttpContext,
) -> Result<PublicBalanceSnapshot> {
    let chain = public_chain_runtime_config(chain_id, effective_chain)?;
    let chain_label =
        chain_name(chain_id).map_or_else(|| format!("chain {chain_id}"), str::to_string);
    let multicall_contract = chain.multicall_contract;
    let planned_calls =
        plan_public_balance_calls(chain_id, multicall_contract, accounts, token_registry);
    if planned_calls.is_empty() {
        return Ok(empty_public_balance_snapshot(chain_id, accounts));
    }

    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    let mut last_error = None;
    let mut results = None;
    for _ in 0..query_rpc_pool.len() {
        let Some(provider_handle) = query_rpc_pool.random_provider() else {
            break;
        };
        let mut multicall = provider_handle
            .provider
            .multicall()
            .dynamic::<PublicErc20::balanceOfCall>()
            .address(multicall_contract);
        for call in &planned_calls {
            multicall =
                multicall.add_call_dynamic(CallItem::new(call.target, call.data.clone().into()));
        }

        match multicall.try_aggregate(false).await {
            Ok(values) => {
                results = Some(values);
                break;
            }
            Err(error) => {
                let rpc = redact_url_for_display(&provider_handle.url);
                tracing::warn!(%error, %rpc, "refresh public balances multicall failed");
                query_rpc_pool.mark_bad_provider(&provider_handle);
                last_error = Some(eyre!("{error}"));
            }
        }
    }
    let results = results.ok_or_else(|| {
        let account_suffix = if accounts.len() == 1 { "" } else { "s" };
        let call_suffix = if planned_calls.len() == 1 { "" } else { "s" };
        let detail = last_error.map_or_else(
            || "no healthy query RPC available".to_string(),
            |error| error.to_string(),
        );
        eyre!(
            "could not refresh public balances on {chain_label}: Multicall3 request to configured RPCs ({multicall_contract:#x}) failed for {} account{account_suffix} and {} balance call{call_suffix}: {detail}",
            accounts.len(),
            planned_calls.len(),
        )
    })?;
    Ok(public_balance_snapshot_from_results(
        chain_id,
        accounts,
        &planned_calls,
        results.into_iter().map(std::result::Result::ok).collect(),
    ))
}

pub(super) fn public_balance_snapshot_from_results(
    chain_id: u64,
    accounts: &[PublicAccountMetadata],
    planned_calls: &[PlannedPublicBalanceCall],
    results: Vec<Option<U256>>,
) -> PublicBalanceSnapshot {
    let mut by_account: BTreeMap<String, Vec<PublicBalanceEntry>> = BTreeMap::new();
    for (call, result) in planned_calls.iter().zip(results) {
        by_account
            .entry(call.public_account_uuid.clone())
            .or_default()
            .push(PublicBalanceEntry {
                asset: call.asset.clone(),
                amount: result.map_or(
                    PublicBalanceAmount::Unavailable,
                    PublicBalanceAmount::Available,
                ),
            });
    }

    PublicBalanceSnapshot {
        chain_id,
        refreshed_at: SystemTime::now(),
        accounts: accounts
            .iter()
            .cloned()
            .map(|account| PublicAccountBalance {
                balances: by_account
                    .remove(&account.public_account_uuid)
                    .unwrap_or_default(),
                account,
            })
            .collect(),
    }
}

fn empty_public_balance_snapshot(
    chain_id: u64,
    accounts: &[PublicAccountMetadata],
) -> PublicBalanceSnapshot {
    PublicBalanceSnapshot {
        chain_id,
        refreshed_at: SystemTime::now(),
        accounts: accounts
            .iter()
            .cloned()
            .map(|account| PublicAccountBalance {
                account,
                balances: Vec::new(),
            })
            .collect(),
    }
}

fn native_asset_for_chain(chain_id: u64) -> Option<PublicBalanceAsset> {
    let symbol = match chain_id {
        1 | 42161 => "ETH",
        56 => "BNB",
        137 => "MATIC",
        _ => return None,
    };
    Some(PublicBalanceAsset {
        id: PublicAssetId::Native,
        symbol: symbol.to_string(),
        decimals: 18,
    })
}
