use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::network::TransactionBuilder as _;
use alloy::primitives::{Address, FixedBytes, U256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use broadcaster_core::query_rpc_pool::{ProviderHandle, QueryRpcPool};
use eyre::{Result, WrapErr, eyre};

use super::gas::public_action_gas_fee_quote_from_rpc_pool;
use super::signer::VaultedPublicSigner;
use super::types::{
    PublicActionAttemptInfo, PublicActionCommand, PublicActionCommandReceiver,
    PublicActionGasFeeQuote, PublicActionGasFeeSelection, PublicActionProgressStatus,
    PublicActionProgressStep, PublicActionProgressUpdate, PublicActionSessionEvent,
    PublicActionSessionEventSender,
};
use crate::settings::EffectiveChainGasSettings;
use crate::{
    SelfBroadcastResolvedGasFee, TxReceiptOutput, report_chain_string,
    resolve_self_broadcast_gas_fee, self_broadcast_replacement_bumped_fee,
    self_broadcast_send_raw_transaction_to_rpc_pool, tx_receipt_output,
};

pub(super) struct PublicActionStepOutcome {
    pub(super) receipt: TxReceiptOutput,
    pub(super) next_nonce: u64,
    pub(super) gas_fee: PublicActionGasFeeSelection,
}

pub(super) struct PublicActionPreflight {
    tx_req: TransactionRequest,
    nonce: u64,
    gas_limit: u64,
    rpc_gas_price: u128,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    estimated_native_gas_cost: U256,
    live_native_balance: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublicActionPreflightMode {
    Managed,
    PreserveRequestFields,
}

impl PublicActionPreflightMode {
    const fn needs_fee_quote(self, tx_req: &TransactionRequest) -> bool {
        match self {
            Self::Managed => true,
            Self::PreserveRequestFields => {
                tx_req.gas_price.is_none()
                    && (tx_req.max_fee_per_gas.is_none()
                        || tx_req.max_priority_fee_per_gas.is_none())
            }
        }
    }
}

pub(super) struct SubmittedPublicActionAttempt {
    provider_handles: Vec<ProviderHandle>,
    tx_hash: FixedBytes<32>,
    pub(super) info: PublicActionAttemptInfo,
    rpc_gas_price: u128,
    estimated_native_gas_cost: U256,
    live_native_balance: U256,
}

struct PublicActionSentTx {
    tx_hash: FixedBytes<32>,
    tx_hash_string: String,
    provider_handles: Vec<ProviderHandle>,
}

pub(super) async fn submit_public_action_step_session(
    step: PublicActionProgressStep,
    base_tx_req: TransactionRequest,
    signer: &VaultedPublicSigner,
    label: &str,
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
    from_address: Address,
    gas: &EffectiveChainGasSettings,
    authorized_gas_limit: Option<u64>,
    mut nonce: Option<u64>,
    gas_fee: PublicActionGasFeeSelection,
    command_rx: &mut Option<PublicActionCommandReceiver>,
    event_tx: Option<&PublicActionSessionEventSender>,
    progress: &mut (impl FnMut(PublicActionProgressUpdate) + Send),
) -> Result<PublicActionStepOutcome> {
    let mut next_gas_fee = gas_fee;
    let authorized_gas_fee = authorized_gas_limit.map(|_| gas_fee);
    let mut submitted_attempts = Vec::new();

    loop {
        progress(public_action_progress_update(
            step,
            PublicActionProgressStatus::Pending,
            None,
            None,
        ));

        let preflight = match public_action_preflight_from_rpc_pool(
            query_rpc_pool,
            network_mode,
            chain_id,
            from_address,
            base_tx_req.clone(),
            next_gas_fee,
            gas,
            authorized_gas_limit,
            nonce,
            None,
        )
        .await
        {
            Ok(preflight) => preflight,
            Err(error) => {
                let message = report_chain_string(&error);
                progress(public_action_progress_update(
                    step,
                    PublicActionProgressStatus::Error,
                    None,
                    Some(message.clone()),
                ));
                emit_public_action_event(
                    event_tx,
                    PublicActionSessionEvent::StepFailed { step, message },
                );
                if authorized_gas_limit.is_some() {
                    return Err(error).wrap_err(
                        "advanced transaction requires a refreshed estimate and authorization",
                    );
                }
                let Some(command) = recv_public_action_command(command_rx).await else {
                    return Err(error);
                };
                next_gas_fee = command.gas_fee;
                continue;
            }
        };
        nonce = Some(preflight.nonce);

        emit_public_action_event(event_tx, PublicActionSessionEvent::AttemptHandoff { step });
        let attempt = match submit_public_action_attempt(
            step,
            preflight,
            query_rpc_pool,
            network_mode,
            signer,
            label,
            event_tx,
            None,
        )
        .await
        {
            Ok(attempt) => attempt,
            Err(
                PublicActionAttemptError::Signing(error) | PublicActionAttemptError::Sending(error),
            ) => {
                let message = report_chain_string(&error);
                progress(public_action_progress_update(
                    step,
                    PublicActionProgressStatus::Error,
                    None,
                    Some(message.clone()),
                ));
                emit_public_action_event(
                    event_tx,
                    PublicActionSessionEvent::StepFailed { step, message },
                );
                let Some(command) = recv_public_action_command(command_rx).await else {
                    return Err(error);
                };
                ensure_public_action_command_gas_fee_authorized(
                    authorized_gas_fee,
                    command.gas_fee,
                )?;
                next_gas_fee = command.gas_fee;
                continue;
            }
        };
        progress(public_action_progress_update(
            step,
            PublicActionProgressStatus::Pending,
            Some(attempt.info.tx_hash.clone()),
            None,
        ));
        submitted_attempts.push(attempt);

        loop {
            let receipt = if command_rx.is_some() {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(3)) => {
                        poll_public_action_attempt_receipts(&submitted_attempts).await?
                    }
                    command = recv_public_action_command(command_rx) => {
                        let Some(command) = command else {
                            *command_rx = None;
                            continue;
                        };
                        if let Err(error) = ensure_public_action_command_gas_fee_authorized(
                            authorized_gas_fee,
                            command.gas_fee,
                        ) {
                            emit_public_action_event(
                                event_tx,
                                PublicActionSessionEvent::AttemptRejected {
                                    step,
                                    message: report_chain_string(&error),
                                },
                            );
                            continue;
                        }
                        let Some(nonce) = nonce else {
                            next_gas_fee = command.gas_fee;
                            break;
                        };
                        let gas_limit = submitted_attempts
                            .last()
                            .map_or(0, |attempt| attempt.info.gas_limit);
                        let replacement = match public_action_preflight_from_rpc_pool(
                            query_rpc_pool,
                            network_mode,
                            chain_id,
                            from_address,
                            base_tx_req.clone(),
                            command.gas_fee,
                            gas,
                            authorized_gas_limit,
                            Some(nonce),
                            Some(gas_limit),
                        )
                        .await
                        {
                            Ok(preflight) => preflight,
                            Err(error) => {
                                emit_public_action_event(
                                    event_tx,
                                    PublicActionSessionEvent::AttemptRejected {
                                        step,
                                        message: report_chain_string(&error),
                                    },
                                );
                                continue;
                            }
                        };
                        emit_public_action_event(
                            event_tx,
                            PublicActionSessionEvent::AttemptHandoff { step },
                        );
                        match submit_public_action_attempt(
                            step,
                            replacement,
                            query_rpc_pool,
                            network_mode,
                            signer,
                            label,
                            event_tx,
                            None,
                        )
                        .await
                        {
                            Ok(attempt) => {
                                progress(public_action_progress_update(
                                    step,
                                    PublicActionProgressStatus::Pending,
                                    Some(attempt.info.tx_hash.clone()),
                                    None,
                                ));
                                submitted_attempts.push(attempt);
                            }
                            Err(error) => emit_public_action_event(
                                event_tx,
                                PublicActionSessionEvent::AttemptRejected {
                                    step,
                                    message: error.message(),
                                },
                            ),
                        }
                        continue;
                    }
                }
            } else {
                tokio::time::sleep(Duration::from_secs(3)).await;
                poll_public_action_attempt_receipts(&submitted_attempts).await?
            };

            if let Some((winner_index, receipt)) = receipt {
                let winner = &submitted_attempts[winner_index];
                tracing::info!(
                    step = ?step,
                    tx_hash = %receipt.tx_hash,
                    rpc_gas_price = winner.rpc_gas_price,
                    estimated_native_gas_cost = %winner.estimated_native_gas_cost,
                    live_native_balance = %winner.live_native_balance,
                    "public action receipt confirmed from submitted attempts"
                );
                if receipt.status {
                    progress(public_action_progress_update(
                        step,
                        PublicActionProgressStatus::Done,
                        Some(receipt.tx_hash.clone()),
                        None,
                    ));
                } else {
                    let message = "Transaction reverted".to_string();
                    progress(public_action_progress_update(
                        step,
                        PublicActionProgressStatus::Error,
                        Some(receipt.tx_hash.clone()),
                        Some(message.clone()),
                    ));
                    emit_public_action_event(
                        event_tx,
                        PublicActionSessionEvent::StepFailed { step, message },
                    );
                    let gas_fee = PublicActionGasFeeSelection::Custom {
                        max_fee_per_gas: winner.info.max_fee_per_gas,
                        max_priority_fee_per_gas: winner.info.max_priority_fee_per_gas,
                    };
                    let Some(command) = recv_public_action_command(command_rx).await else {
                        return Ok(PublicActionStepOutcome {
                            receipt,
                            next_nonce: winner.info.nonce.saturating_add(1),
                            gas_fee,
                        });
                    };
                    ensure_public_action_command_gas_fee_authorized(
                        authorized_gas_fee,
                        command.gas_fee,
                    )?;
                    nonce = Some(winner.info.nonce.saturating_add(1));
                    next_gas_fee = command.gas_fee;
                    submitted_attempts.clear();
                    break;
                }
                let gas_fee = PublicActionGasFeeSelection::Custom {
                    max_fee_per_gas: winner.info.max_fee_per_gas,
                    max_priority_fee_per_gas: winner.info.max_priority_fee_per_gas,
                };
                return Ok(PublicActionStepOutcome {
                    receipt,
                    next_nonce: winner.info.nonce.saturating_add(1),
                    gas_fee,
                });
            }
        }
    }
}

pub(super) fn ensure_public_action_command_gas_fee_authorized(
    authorized_gas_fee: Option<PublicActionGasFeeSelection>,
    requested_gas_fee: PublicActionGasFeeSelection,
) -> Result<()> {
    if authorized_gas_fee.is_some_and(|authorized| authorized != requested_gas_fee) {
        return Err(eyre!(
            "advanced transaction fee changed after authorization; refresh the estimate and authorize again"
        ));
    }
    Ok(())
}

pub(super) async fn submit_public_action_attempt(
    step: PublicActionProgressStep,
    preflight: PublicActionPreflight,
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    signer: &VaultedPublicSigner,
    label: &str,
    event_tx: Option<&PublicActionSessionEventSender>,
    expiry_timestamp: Option<u64>,
) -> Result<SubmittedPublicActionAttempt, PublicActionAttemptError> {
    let sent = sign_send_public_action_transaction(
        query_rpc_pool,
        network_mode,
        signer,
        preflight.tx_req,
        label,
        event_tx,
        expiry_timestamp,
    )
    .await?;
    let info = PublicActionAttemptInfo {
        tx_hash: sent.tx_hash_string,
        nonce: preflight.nonce,
        gas_limit: preflight.gas_limit,
        max_fee_per_gas: preflight.max_fee_per_gas,
        max_priority_fee_per_gas: preflight.max_priority_fee_per_gas,
    };
    emit_public_action_event(
        event_tx,
        PublicActionSessionEvent::AttemptSubmitted {
            step,
            attempt: info.clone(),
        },
    );
    Ok(SubmittedPublicActionAttempt {
        provider_handles: sent.provider_handles,
        tx_hash: sent.tx_hash,
        info,
        rpc_gas_price: preflight.rpc_gas_price,
        estimated_native_gas_cost: preflight.estimated_native_gas_cost,
        live_native_balance: preflight.live_native_balance,
    })
}

pub(super) enum PublicActionAttemptError {
    Signing(eyre::Report),
    Sending(eyre::Report),
}

impl PublicActionAttemptError {
    pub(super) fn message(&self) -> String {
        match self {
            Self::Signing(error) | Self::Sending(error) => report_chain_string(error),
        }
    }
}

async fn public_action_preflight_from_rpc_pool(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
    from: Address,
    base_tx_req: TransactionRequest,
    gas_fee: PublicActionGasFeeSelection,
    gas: &EffectiveChainGasSettings,
    authorized_gas_limit: Option<u64>,
    nonce: Option<u64>,
    gas_limit: Option<u64>,
) -> Result<PublicActionPreflight> {
    public_action_preflight_from_rpc_pool_with_mode(
        query_rpc_pool,
        network_mode,
        chain_id,
        from,
        base_tx_req,
        gas_fee,
        gas,
        authorized_gas_limit,
        nonce,
        gas_limit,
        PublicActionPreflightMode::Managed,
    )
    .await
}

pub(super) async fn public_action_preflight_from_rpc_pool_with_mode(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    chain_id: u64,
    from: Address,
    base_tx_req: TransactionRequest,
    gas_fee: PublicActionGasFeeSelection,
    gas: &EffectiveChainGasSettings,
    authorized_gas_limit: Option<u64>,
    nonce: Option<u64>,
    gas_limit: Option<u64>,
    mode: PublicActionPreflightMode,
) -> Result<PublicActionPreflight> {
    let quote = if mode.needs_fee_quote(&base_tx_req) {
        Some(
            public_action_gas_fee_quote_from_rpc_pool(query_rpc_pool, network_mode, chain_id)
                .await
                .wrap_err("fetch public action gas price")?,
        )
    } else {
        None
    };
    let mut last_error = None;
    for _ in 0..query_rpc_pool.len() {
        let Some(provider_handle) = query_rpc_pool.random_provider() else {
            break;
        };
        match public_action_preflight(
            provider_handle,
            chain_id,
            from,
            base_tx_req.clone(),
            gas_fee,
            quote,
            gas,
            authorized_gas_limit,
            nonce,
            gas_limit,
            mode,
        )
        .await
        {
            Ok(preflight) => return Ok(preflight),
            Err(error) => {
                tracing::warn!(%error, "public action preflight failed");
                last_error = Some(error);
            }
        }
    }
    if let Some(error) = last_error {
        Err(error).wrap_err("all public action query RPC attempts failed")
    } else {
        Err(eyre!("no healthy query RPC available"))
    }
}

async fn public_action_preflight(
    provider_handle: ProviderHandle,
    chain_id: u64,
    from: Address,
    base_tx_req: TransactionRequest,
    gas_fee: PublicActionGasFeeSelection,
    quote: Option<PublicActionGasFeeQuote>,
    gas: &EffectiveChainGasSettings,
    authorized_gas_limit: Option<u64>,
    nonce: Option<u64>,
    gas_limit: Option<u64>,
    mode: PublicActionPreflightMode,
) -> Result<PublicActionPreflight> {
    let provider = &provider_handle.provider;
    let resolved = match quote {
        Some(quote) => resolve_self_broadcast_gas_fee(gas_fee, quote)?,
        None => walletconnect_resolved_gas_fee_from_request(&base_tx_req)?,
    };
    let requested_nonce = match mode {
        PublicActionPreflightMode::Managed => nonce,
        PublicActionPreflightMode::PreserveRequestFields => base_tx_req.nonce.or(nonce),
    };
    let requested_gas_limit = match mode {
        PublicActionPreflightMode::Managed => gas_limit,
        PublicActionPreflightMode::PreserveRequestFields => base_tx_req.gas.or(gas_limit),
    };
    let nonce = if let Some(nonce) = requested_nonce {
        nonce
    } else {
        provider
            .get_transaction_count(from)
            .await
            .wrap_err("fetch public action nonce")?
    };
    let tx_req = match mode {
        PublicActionPreflightMode::Managed => public_action_eip1559_transaction_request(
            base_tx_req,
            chain_id,
            from,
            resolved.max_fee_per_gas,
            resolved.max_priority_fee_per_gas,
            nonce,
        ),
        PublicActionPreflightMode::PreserveRequestFields => {
            public_action_fill_walletconnect_transaction_request(
                base_tx_req,
                chain_id,
                from,
                resolved.max_fee_per_gas,
                resolved.max_priority_fee_per_gas,
                nonce,
            )?
        }
    };
    let max_fee_per_gas = tx_req
        .max_fee_per_gas
        .or(tx_req.gas_price)
        .unwrap_or(resolved.max_fee_per_gas);
    let max_priority_fee_per_gas = tx_req.max_priority_fee_per_gas.unwrap_or_else(|| {
        if tx_req.gas_price.is_some() {
            0
        } else {
            resolved.max_priority_fee_per_gas
        }
    });
    let gas_limit = if let Some(authorized_gas_limit) = authorized_gas_limit {
        let estimated_gas = provider
            .estimate_gas(tx_req.clone())
            .await
            .wrap_err("re-estimate authorized advanced public transaction gas")?;
        ensure_advanced_gas_estimate_authorized(estimated_gas, authorized_gas_limit)?;
        authorized_gas_limit
    } else if let Some(gas_limit) = requested_gas_limit {
        gas_limit
    } else {
        provider
            .estimate_gas(tx_req.clone())
            .await
            .wrap_err("estimate public action gas")?
            .saturating_add(gas.gas_limit_buffer)
    };
    let estimated_native_gas_cost =
        public_action_native_exposure(tx_req.value.unwrap_or_default(), gas_limit, max_fee_per_gas);
    let live_native_balance = provider
        .get_balance(from)
        .await
        .wrap_err("fetch public action native balance")?;
    if live_native_balance < estimated_native_gas_cost {
        let action = if authorized_gas_limit.is_some() {
            "advanced public transaction"
        } else {
            "public action"
        };
        return Err(eyre!(
            "insufficient native balance for {action}: live balance {live_native_balance}, required value plus maximum gas cost {estimated_native_gas_cost}"
        ));
    }
    Ok(PublicActionPreflight {
        tx_req: tx_req.with_gas_limit(gas_limit),
        nonce,
        gas_limit,
        rpc_gas_price: resolved.rpc_gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        estimated_native_gas_cost,
        live_native_balance,
    })
}

pub(super) fn public_action_eip1559_transaction_request(
    tx_req: TransactionRequest,
    chain_id: u64,
    from: Address,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    nonce: u64,
) -> TransactionRequest {
    tx_req
        .with_chain_id(chain_id)
        .with_from(from)
        .with_max_fee_per_gas(max_fee_per_gas)
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas)
        .with_nonce(nonce)
}

pub(super) fn public_action_fill_walletconnect_transaction_request(
    mut tx_req: TransactionRequest,
    chain_id: u64,
    from: Address,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    nonce: u64,
) -> Result<TransactionRequest> {
    tx_req = tx_req
        .with_chain_id(chain_id)
        .with_from(from)
        .with_nonce(nonce);
    if tx_req.gas_price.is_some() {
        return Ok(tx_req);
    }
    if tx_req.max_fee_per_gas.is_none() {
        tx_req = tx_req.with_max_fee_per_gas(max_fee_per_gas);
    }
    if tx_req.max_priority_fee_per_gas.is_none() {
        tx_req = tx_req.with_max_priority_fee_per_gas(max_priority_fee_per_gas);
    }
    if let (Some(max_fee), Some(priority_fee)) =
        (tx_req.max_fee_per_gas, tx_req.max_priority_fee_per_gas)
        && priority_fee > max_fee
    {
        return Err(eyre!(
            "WalletConnect max priority fee per gas cannot exceed max fee per gas"
        ));
    }
    Ok(tx_req)
}

fn walletconnect_resolved_gas_fee_from_request(
    tx_req: &TransactionRequest,
) -> Result<SelfBroadcastResolvedGasFee> {
    if let Some(gas_price) = tx_req.gas_price {
        if gas_price == 0 {
            return Err(eyre!("WalletConnect gasPrice must be greater than zero"));
        }
        return Ok(SelfBroadcastResolvedGasFee {
            rpc_gas_price: gas_price,
            max_fee_per_gas: gas_price,
            max_priority_fee_per_gas: tx_req.max_priority_fee_per_gas.unwrap_or(0),
        });
    }
    let max_fee_per_gas = tx_req
        .max_fee_per_gas
        .ok_or_else(|| eyre!("WalletConnect maxFeePerGas is required"))?;
    let max_priority_fee_per_gas = tx_req
        .max_priority_fee_per_gas
        .ok_or_else(|| eyre!("WalletConnect maxPriorityFeePerGas is required"))?;
    if max_fee_per_gas == 0 {
        return Err(eyre!(
            "WalletConnect maxFeePerGas must be greater than zero"
        ));
    }
    if max_priority_fee_per_gas > max_fee_per_gas {
        return Err(eyre!(
            "WalletConnect max priority fee per gas cannot exceed max fee per gas"
        ));
    }
    Ok(SelfBroadcastResolvedGasFee {
        rpc_gas_price: max_fee_per_gas,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    })
}

pub(super) fn public_action_native_exposure(
    value: U256,
    gas_limit: u64,
    max_fee_per_gas: u128,
) -> U256 {
    value + (U256::from(gas_limit) * U256::from(max_fee_per_gas))
}

pub(super) fn ensure_advanced_gas_estimate_authorized(
    estimated_gas: u64,
    authorized_gas_limit: u64,
) -> Result<()> {
    if estimated_gas > authorized_gas_limit {
        return Err(eyre!(
            "advanced transaction gas estimate {estimated_gas} exceeds authorized limit {authorized_gas_limit}; refresh the estimate and authorize again"
        ));
    }
    Ok(())
}

async fn sign_send_public_action_transaction(
    query_rpc_pool: &QueryRpcPool,
    network_mode: crate::WalletNetworkMode,
    signer: &VaultedPublicSigner,
    tx_req: TransactionRequest,
    label: &str,
    event_tx: Option<&PublicActionSessionEventSender>,
    expiry_timestamp: Option<u64>,
) -> Result<PublicActionSentTx, PublicActionAttemptError> {
    tracing::info!(
        from = %tx_req.from.unwrap_or_default(),
        to = ?tx_req.to,
        gas = ?tx_req.gas,
        label,
        "signing and sending public action transaction",
    );
    let signed_tx = signer
        .sign_transaction_request(tx_req, label)
        .await
        .map_err(PublicActionAttemptError::Signing)?;
    emit_refreshed_public_action_hardware_session(event_tx, signer);
    // Stop/abort requested during synchronous hardware approval is observed here before RPC broadcast.
    public_action_before_raw_broadcast_checkpoint().await;
    ensure_public_action_broadcast_not_expired(expiry_timestamp, label)
        .map_err(PublicActionAttemptError::Sending)?;
    let tx_hash = keccak256(&signed_tx);
    let provider_handles = self_broadcast_send_raw_transaction_to_rpc_pool(
        query_rpc_pool,
        network_mode,
        signed_tx,
        tx_hash,
    )
    .await
    .wrap_err_with(|| format!("{label}: send"))
    .map_err(PublicActionAttemptError::Sending)?;
    let tx_hash_string = alloy::hex::encode_prefixed(tx_hash);
    tracing::info!(%tx_hash, providers = provider_handles.len(), label, "sent public action transaction");
    Ok(PublicActionSentTx {
        tx_hash,
        tx_hash_string,
        provider_handles,
    })
}

pub(super) fn ensure_public_action_broadcast_not_expired(
    expiry_timestamp: Option<u64>,
    label: &str,
) -> Result<()> {
    let Some(expiry_timestamp) = expiry_timestamp else {
        return Ok(());
    };
    if public_action_current_unix_seconds() >= expiry_timestamp {
        return Err(eyre!(
            "{label}: request expired before transaction broadcast"
        ));
    }
    Ok(())
}

pub(super) fn public_action_current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) async fn public_action_before_raw_broadcast_checkpoint() {
    tokio::task::yield_now().await;
}

async fn poll_public_action_attempt_receipts(
    attempts: &[SubmittedPublicActionAttempt],
) -> Result<Option<(usize, TxReceiptOutput)>> {
    let mut queried_provider_count = 0;
    let mut pending_response_count = 0;
    let mut last_error = None;
    for (index, attempt) in attempts.iter().enumerate() {
        for provider_handle in &attempt.provider_handles {
            match provider_handle
                .provider
                .get_transaction_receipt(attempt.tx_hash)
                .await
            {
                Ok(Some(receipt)) => {
                    return Ok(Some((index, tx_receipt_output(attempt.tx_hash, &receipt))));
                }
                Ok(None) => {
                    queried_provider_count += 1;
                    pending_response_count += 1;
                }
                Err(error) => {
                    queried_provider_count += 1;
                    last_error = Some(format!("{}: {error}", provider_handle.url));
                    tracing::warn!(
                        url = %provider_handle.url,
                        %error,
                        "public action receipt fetch failed"
                    );
                }
            }
        }
    }
    if let Some(message) = public_action_receipt_poll_error_message(
        queried_provider_count,
        pending_response_count,
        last_error,
    ) {
        return Err(eyre!("{message}"));
    }
    Ok(None)
}

#[must_use]
pub(super) fn public_action_receipt_poll_error_message(
    queried_provider_count: usize,
    pending_response_count: usize,
    last_error: Option<String>,
) -> Option<String> {
    if queried_provider_count == 0 || pending_response_count > 0 {
        return None;
    }
    last_error.map(|error| {
        format!(
            "public action receipt fetch failed for all accepted RPC providers ({queried_provider_count} checked): {error}"
        )
    })
}

pub(super) fn emit_public_action_event(
    event_tx: Option<&PublicActionSessionEventSender>,
    event: PublicActionSessionEvent,
) {
    if let Some(event_tx) = event_tx {
        let _ = event_tx.send(event);
    }
}

pub(super) fn emit_refreshed_public_action_hardware_session(
    event_tx: Option<&PublicActionSessionEventSender>,
    signer: &VaultedPublicSigner,
) {
    match signer.refreshed_hardware_session() {
        Ok(Some(session)) => emit_public_action_event(
            event_tx,
            PublicActionSessionEvent::HardwareProfileSessionRefreshed { session },
        ),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "failed to read refreshed hardware public signer session");
        }
    }
}

pub(super) async fn recv_public_action_command(
    command_rx: &mut Option<PublicActionCommandReceiver>,
) -> Option<PublicActionCommand> {
    let command_rx = command_rx.as_mut()?;
    command_rx.recv().await
}

#[must_use]
pub const fn public_action_replacement_bumped_fee(value: u128) -> u128 {
    self_broadcast_replacement_bumped_fee(value)
}

pub(super) const fn public_action_progress_update(
    step: PublicActionProgressStep,
    status: PublicActionProgressStatus,
    tx_hash: Option<String>,
    message: Option<String>,
) -> PublicActionProgressUpdate {
    PublicActionProgressUpdate {
        step,
        status,
        tx_hash,
        message,
    }
}
