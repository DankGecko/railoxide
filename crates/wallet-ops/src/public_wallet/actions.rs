use alloy::network::TransactionBuilder as _;
use alloy::primitives::{Address, Bytes, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use eyre::{Result, WrapErr, eyre};

use super::contracts::{PublicErc20, PublicRelayAdapt, RelayAdaptCall};
use super::runtime::{public_chain_runtime_config, public_shield_token};
use super::signer::vaulted_public_signer;
use super::submission::{
    emit_public_action_event, emit_refreshed_public_action_hardware_session,
    public_action_progress_update, recv_public_action_command, submit_public_action_step_session,
};
use super::types::{
    PublicActionProgressStatus, PublicActionProgressStep, PublicActionProgressUpdate,
    PublicActionSessionEvent, PublicAssetId, PublicSendRequest, PublicSendResult,
    PublicShieldRequest,
};
use crate::{HttpContext, ShieldSendOutput, query_rpc_pool_with_http_client, report_chain_string};

pub async fn submit_public_send(
    request: PublicSendRequest,
    http: &HttpContext,
) -> Result<PublicSendResult> {
    submit_public_send_with_progress(request, http, |_| {}).await
}

pub async fn submit_public_send_with_progress(
    request: PublicSendRequest,
    http: &HttpContext,
    mut progress: impl FnMut(PublicActionProgressUpdate) + Send,
) -> Result<PublicSendResult> {
    if request.amount.is_zero() {
        return Err(eyre!("amount is required"));
    }
    let chain = public_chain_runtime_config(request.chain_id, request.effective_chain.as_ref())?;
    let signer = vaulted_public_signer(
        &request.vault_store,
        &request.view_session,
        Some(request.vault_password.as_str()),
        &request.public_account_uuid,
        request.trezor_app_passphrase,
        request.trezor_pin_matrix_provider,
    )?;
    let from_address = signer.address();
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);
    let tx_req = public_send_transaction_request(
        request.chain_id,
        from_address,
        request.asset,
        request.amount,
        request.recipient,
    );
    let mut command_rx = request.command_rx;
    let tx = submit_public_action_step_session(
        PublicActionProgressStep::Send,
        tx_req,
        &signer,
        "public-send",
        &query_rpc_pool,
        http.network_mode(),
        request.chain_id,
        from_address,
        &chain.gas,
        None,
        request.gas_fee,
        &mut command_rx,
        request.event_tx.as_ref(),
        &mut progress,
    )
    .await?
    .receipt;
    if !tx.status {
        return Err(eyre!("public send transaction reverted ({})", tx.tx_hash));
    }
    Ok(PublicSendResult { tx })
}

pub async fn submit_public_shield(
    request: PublicShieldRequest,
    http: &HttpContext,
) -> Result<ShieldSendOutput> {
    submit_public_shield_with_progress(request, http, |_| {}).await
}

pub async fn submit_public_shield_with_progress(
    request: PublicShieldRequest,
    http: &HttpContext,
    mut progress: impl FnMut(PublicActionProgressUpdate) + Send,
) -> Result<ShieldSendOutput> {
    if request.amount.is_zero() {
        return Err(eyre!("amount is required"));
    }
    let chain = public_chain_runtime_config(request.chain_id, request.effective_chain.as_ref())?;
    let token = public_shield_token(request.asset, &chain)?;
    let recipient = request
        .view_session
        .receive_address()
        .wrap_err("derive selected private wallet receive address")?;
    let railgun_addr = broadcaster_core::crypto::railgun::Address::from(recipient.as_str());
    let addr_data = broadcaster_core::crypto::railgun::AddressData::try_from(&railgun_addr)
        .wrap_err("invalid selected private wallet receive address")?;
    let signer = vaulted_public_signer(
        &request.vault_store,
        &request.view_session,
        Some(request.vault_password.as_str()),
        &request.public_account_uuid,
        request.trezor_app_passphrase,
        request.trezor_pin_matrix_provider,
    )?;
    let mut nonce = None;
    let mut gas_fee = request.gas_fee;
    let mut command_rx = request.command_rx;
    let event_tx = request.event_tx;
    let shield_private_key = if signer.requires_device_approval() {
        loop {
            progress(public_action_progress_update(
                PublicActionProgressStep::ShieldKey,
                PublicActionProgressStatus::Pending,
                None,
                None,
            ));
            match signer.derive_shield_private_key().await {
                Ok(shield_private_key) => {
                    progress(public_action_progress_update(
                        PublicActionProgressStep::ShieldKey,
                        PublicActionProgressStatus::Done,
                        None,
                        None,
                    ));
                    break shield_private_key;
                }
                Err(error) => {
                    let message = report_chain_string(&error);
                    progress(public_action_progress_update(
                        PublicActionProgressStep::ShieldKey,
                        PublicActionProgressStatus::Error,
                        None,
                        Some(message.clone()),
                    ));
                    emit_public_action_event(
                        event_tx.as_ref(),
                        PublicActionSessionEvent::StepFailed {
                            step: PublicActionProgressStep::ShieldKey,
                            message,
                        },
                    );
                    let Some(command) = recv_public_action_command(&mut command_rx).await else {
                        return Err(error);
                    };
                    gas_fee = command.gas_fee;
                }
            }
        }
    } else {
        signer.derive_shield_private_key().await?
    };
    emit_refreshed_public_action_hardware_session(event_tx.as_ref(), &signer);
    let shield_data = broadcaster_core::contracts::shield::build_shield_calldata(
        addr_data.master_public_key,
        &addr_data.viewing_public_key,
        token,
        request.amount,
        &shield_private_key,
    )
    .wrap_err("build public shield calldata")?;

    let from_address = signer.address();
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls, http);

    let approve_receipt = if request.asset == PublicAssetId::Native {
        None
    } else {
        let approve_data = broadcaster_core::contracts::shield::build_approve_calldata(
            chain.railgun_contract,
            request.amount,
        );
        let approve_tx = TransactionRequest::default()
            .with_chain_id(request.chain_id)
            .with_from(from_address)
            .with_to(token)
            .with_input(approve_data)
            .with_nonce(0);
        let approve_outcome = submit_public_action_step_session(
            PublicActionProgressStep::Approve,
            approve_tx,
            &signer,
            "public-shield-approve",
            &query_rpc_pool,
            http.network_mode(),
            request.chain_id,
            from_address,
            &chain.gas,
            nonce,
            gas_fee,
            &mut command_rx,
            event_tx.as_ref(),
            &mut progress,
        )
        .await?;
        let receipt = approve_outcome.receipt;
        if !receipt.status {
            return Err(eyre!(
                "public shield approve transaction reverted ({})",
                receipt.tx_hash
            ));
        }
        nonce = Some(approve_outcome.next_nonce);
        gas_fee = approve_outcome.gas_fee;
        Some(receipt)
    };

    let shield_tx = if request.asset == PublicAssetId::Native {
        public_native_shield_transaction_request(
            request.chain_id,
            from_address,
            chain.relay_adapt_contract,
            request.amount,
            shield_data,
        )
    } else {
        TransactionRequest::default()
            .with_chain_id(request.chain_id)
            .with_from(from_address)
            .with_to(chain.railgun_contract)
            .with_input(shield_data)
            .with_nonce(0)
    };
    let shield_receipt = submit_public_action_step_session(
        PublicActionProgressStep::Shield,
        shield_tx,
        &signer,
        "public-shield",
        &query_rpc_pool,
        http.network_mode(),
        request.chain_id,
        from_address,
        &chain.gas,
        nonce,
        gas_fee,
        &mut command_rx,
        event_tx.as_ref(),
        &mut progress,
    )
    .await?
    .receipt;
    if !shield_receipt.status {
        return Err(eyre!(
            "public shield transaction reverted ({})",
            shield_receipt.tx_hash
        ));
    }

    Ok(ShieldSendOutput {
        wrap: None,
        approve: approve_receipt,
        shield: shield_receipt,
    })
}

pub(super) fn public_native_shield_transaction_request(
    chain_id: u64,
    from: Address,
    relay_adapt: Address,
    amount: U256,
    shield_data: Vec<u8>,
) -> TransactionRequest {
    let calls = vec![
        RelayAdaptCall {
            to: relay_adapt,
            data: PublicRelayAdapt::wrapBaseCall { _amount: amount }
                .abi_encode()
                .into(),
            value: U256::ZERO,
        },
        RelayAdaptCall {
            to: relay_adapt,
            data: Bytes::from(shield_data),
            value: U256::ZERO,
        },
    ];
    TransactionRequest::default()
        .with_chain_id(chain_id)
        .with_from(from)
        .with_to(relay_adapt)
        .with_input(
            PublicRelayAdapt::multicallCall {
                _requireSuccess: true,
                _calls: calls,
            }
            .abi_encode(),
        )
        .with_value(amount)
        .with_nonce(0)
}

pub(super) fn public_send_transaction_request(
    chain_id: u64,
    from: Address,
    asset: PublicAssetId,
    amount: U256,
    recipient: Address,
) -> TransactionRequest {
    let mut tx_req = TransactionRequest::default()
        .with_chain_id(chain_id)
        .with_from(from);
    match asset {
        PublicAssetId::Native => {
            tx_req = tx_req.with_to(recipient).with_value(amount);
        }
        PublicAssetId::Erc20(token) => {
            tx_req = tx_req
                .with_to(token)
                .with_input(PublicErc20::transferCall { recipient, amount }.abi_encode());
        }
    }
    tx_req
}
