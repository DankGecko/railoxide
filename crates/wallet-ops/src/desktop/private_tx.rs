use super::*;
use eyre::eyre;

pub(super) async fn prepare_desktop_unshield_plan_without_broadcaster_fee(
    request: DesktopUnshieldPlanRequest<'_>,
    http: &HttpContext,
) -> Result<PreparedDesktopUnshieldPlan> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }
    let chain = effective_desktop_chain_config(request.chain_id, request.effective_chain)?;
    if request.unwrap && !is_effective_wrapped_native_token(request.chain_id, request.token, &chain)
    {
        return Err(eyre!("selected token does not support unwrap-to-native"));
    }

    let artifact_source = artifact_source(http, request.session.db.as_ref());
    let prover = ProverService::new_with_db(&artifact_source, &request.session.db);
    let chain_handle = request
        .session
        .sync_manager
        .chain_handle(&request.session.chain_key)
        .await
        .ok_or_else(|| eyre!("chain handle not found for chain {}", request.chain_id))?;
    let mut forest = chain_handle.forest.read().await.clone();
    forest.compute_roots();

    let utxos = request.session.unspent_utxos();
    let mode = if request.unwrap {
        UnshieldMode::UnwrapBase
    } else {
        UnshieldMode::Token
    };
    let receiver_amount = unshield_receiver_amount_for_fee_mode(request.amount, request.fee_mode)?;
    let unshield_request = RailgunUnshieldRequest {
        token_address: request.token,
        amount: receiver_amount,
        recipient: request.recipient,
        mode,
        verify_proof: request.verify_proof,
        spend_up_to: false,
        broadcaster_fee: None,
        min_gas_price: 0,
    };
    update_transaction_generation_stage(
        request.progress_tx,
        TransactionGenerationStage::SelectingPrivateNotes,
    );
    let selection_info = unshield_selection_info(&utxos, request.token, receiver_amount, false)
        .wrap_err("select POI-verified unshield notes")?;
    let native_top_up = request
        .native_top_up
        .as_ref()
        .map(|top_up| {
            desktop_native_top_up_plan_from_request(
                &request,
                &chain,
                top_up,
                receiver_amount,
                &utxos,
            )
        })
        .transpose()?;

    let signer = request.spend_authorization.into_signer(
        request.vault_store,
        request.view_session.wallet_id(),
        "unshield",
    )?;

    let tx_builder = TransactionBuilder {
        chain_type: 0,
        chain_id: request.chain_id,
        railgun_contract: chain.railgun_contract,
        relay_adapt_contract: chain.relay_adapt_contract,
    };

    update_transaction_generation_stage(
        request.progress_tx,
        TransactionGenerationStage::ProvingTransaction,
    );
    if let Some(native_top_up) = native_top_up {
        let composite_request = native_top_up_composite_unshield_request(
            request.token,
            receiver_amount,
            request.recipient,
            request.unwrap,
            request.verify_proof,
            &native_top_up,
        )?;
        let plan = tx_builder
            .build_composite_unshield_plan_with_signer(
                &request.view_session.scan_keys(),
                &signer,
                &forest,
                &utxos,
                composite_request,
                &prover,
            )
            .await
            .wrap_err("build desktop composite unshield calldata")?;

        return Ok(PreparedDesktopUnshieldPlan {
            plan: DesktopUnshieldPreparedPlan::Composite(plan),
            max_spendable: selection_info.max_spendable,
            prover,
            native_top_up: Some(native_top_up),
        });
    }

    let plan = tx_builder
        .build_unshield_plan_with_signer(
            &request.view_session.scan_keys(),
            &signer,
            &forest,
            &utxos,
            unshield_request,
            &prover,
        )
        .await
        .wrap_err("build desktop unshield calldata")?;

    Ok(PreparedDesktopUnshieldPlan {
        plan: DesktopUnshieldPreparedPlan::Single(plan),
        max_spendable: selection_info.max_spendable,
        prover,
        native_top_up: None,
    })
}

fn desktop_native_top_up_plan_from_request(
    request: &DesktopUnshieldPlanRequest<'_>,
    chain: &EffectiveDesktopChainConfig,
    top_up: &DesktopNativeTopUpRequest,
    receiver_amount: U256,
    utxos: &[Utxo],
) -> Result<DesktopNativeTopUpPlan> {
    desktop_native_top_up_plan_from_unshield_fields(
        request.chain_id,
        chain,
        request.view_session,
        request.vault_store,
        request.token,
        request.recipient,
        request.unwrap,
        top_up,
        receiver_amount,
        None,
        U256::ZERO,
        utxos,
    )
}

pub(super) fn desktop_native_top_up_plan_from_unshield_fields(
    chain_id: u64,
    chain: &EffectiveDesktopChainConfig,
    view_session: &vault::DesktopViewSession,
    vault_store: &vault::DesktopVaultStore,
    token: Address,
    recipient: Address,
    unwrap: bool,
    top_up: &DesktopNativeTopUpRequest,
    receiver_amount: U256,
    broadcaster_fee_token: Option<Address>,
    broadcaster_fee_amount: U256,
    utxos: &[Utxo],
) -> Result<DesktopNativeTopUpPlan> {
    let policy = native_top_up_policy_for_chain(chain_id)
        .ok_or_else(|| eyre!("selected chain does not support native top-up"))?;
    let wrapped_native_token = chain
        .wrapped_native_token
        .ok_or_else(|| eyre!("selected chain has no wrapped native token for native top-up"))?;
    let accounts = vault_store
        .list_active_public_accounts_for_session(view_session)
        .wrap_err("load active public accounts for native top-up")?;
    let account = accounts
        .iter()
        .find(|account| account.public_account_uuid == top_up.public_account_uuid)
        .ok_or_else(|| eyre!("native top-up Public account is not visible"))?;
    if account.address != recipient {
        return Err(eyre!(
            "native top-up Public account does not match unshield recipient"
        ));
    }
    if top_up.native_balance >= policy.offer_threshold {
        return Err(eyre!(
            "native top-up recipient already has enough native gas"
        ));
    }
    if unwrap {
        return Err(eyre!(
            "native top-up cannot be combined with unwrap-to-native output"
        ));
    }
    let wrapped_native_amount = native_top_up_wrapped_native_amount(policy.top_up_amount);
    let mut required_wrapped_native = native_top_up_required_wrapped_native_amount(
        token,
        wrapped_native_token,
        receiver_amount,
        policy.top_up_amount,
    );
    if broadcaster_fee_token == Some(wrapped_native_token) {
        required_wrapped_native = required_wrapped_native.saturating_add(broadcaster_fee_amount);
    }
    let max_wrapped_native = max_unshield_spendable(utxos, wrapped_native_token);
    if max_wrapped_native < required_wrapped_native {
        return Err(eyre!(
            "native top-up wrapped-native max spendable: {max_wrapped_native}; required: {required_wrapped_native}"
        ));
    }

    Ok(DesktopNativeTopUpPlan {
        public_account_uuid: account.public_account_uuid.clone(),
        recipient,
        wrapped_native_token,
        native_amount: policy.top_up_amount,
        wrapped_native_amount,
        native_balance_before: top_up.native_balance,
    })
}

pub(super) fn desktop_native_top_up_plan_for_estimate(
    chain_id: u64,
    chain: &EffectiveDesktopChainConfig,
    _token: Address,
    recipient: Address,
    unwrap: bool,
    top_up: &DesktopNativeTopUpRequest,
    _receiver_amount: U256,
) -> Result<DesktopNativeTopUpPlan> {
    let policy = native_top_up_policy_for_chain(chain_id)
        .ok_or_else(|| eyre!("selected chain does not support native top-up"))?;
    let wrapped_native_token = chain
        .wrapped_native_token
        .ok_or_else(|| eyre!("selected chain has no wrapped native token for native top-up"))?;
    if top_up.native_balance >= policy.offer_threshold {
        return Err(eyre!(
            "native top-up recipient already has enough native gas"
        ));
    }
    if unwrap {
        return Err(eyre!(
            "native top-up cannot be combined with unwrap-to-native output"
        ));
    }
    let wrapped_native_amount = native_top_up_wrapped_native_amount(policy.top_up_amount);

    Ok(DesktopNativeTopUpPlan {
        public_account_uuid: top_up.public_account_uuid.clone(),
        recipient,
        wrapped_native_token,
        native_amount: policy.top_up_amount,
        wrapped_native_amount,
        native_balance_before: top_up.native_balance,
    })
}

pub(crate) fn native_top_up_composite_unshield_request(
    token: Address,
    receiver_amount: U256,
    recipient: Address,
    unwrap: bool,
    verify_proof: bool,
    native_top_up: &DesktopNativeTopUpPlan,
) -> Result<CompositeUnshieldRequest> {
    if unwrap {
        return Err(eyre!(
            "native top-up cannot be combined with unwrap-to-native output"
        ));
    }

    let wrapped_native = native_top_up.wrapped_native_token;
    let native_amount = native_top_up.native_amount;
    let wrapped_native_amount = native_top_up.wrapped_native_amount;
    let mut calls = vec![
        CompositeRelayAction::UnwrapBase {
            amount: native_amount,
        },
        CompositeRelayAction::Transfer {
            token: CompositeRelayActionToken::BaseNative,
            recipient,
            amount: native_amount,
        },
    ];
    let legs = if token == wrapped_native {
        let combined_wrapped_native_amount = native_top_up_required_wrapped_native_amount(
            token,
            wrapped_native,
            receiver_amount,
            native_amount,
        );
        let wrapped_output_amount =
            native_top_up_net_after_protocol_fee(combined_wrapped_native_amount) - native_amount;
        calls.push(CompositeRelayAction::Transfer {
            token: CompositeRelayActionToken::Erc20(wrapped_native),
            recipient,
            amount: wrapped_output_amount,
        });
        vec![CompositeUnshieldLeg {
            token_address: wrapped_native,
            amount: combined_wrapped_native_amount,
            recipient: CompositeUnshieldRecipient::RelayAdapt,
            role: CompositeUnshieldLegRole::WrappedNativeOutput,
        }]
    } else {
        vec![
            CompositeUnshieldLeg {
                token_address: token,
                amount: receiver_amount,
                recipient: CompositeUnshieldRecipient::Public(recipient),
                role: CompositeUnshieldLegRole::Primary,
            },
            CompositeUnshieldLeg {
                token_address: wrapped_native,
                amount: wrapped_native_amount,
                recipient: CompositeUnshieldRecipient::RelayAdapt,
                role: CompositeUnshieldLegRole::NativeTopUp,
            },
        ]
    };

    Ok(CompositeUnshieldRequest {
        legs,
        relay_actions: Some(CompositeRelayActions {
            min_gas_limit: U256::ZERO,
            calls,
        }),
        broadcaster_fee: None,
        min_gas_price: 0,
        verify_proof,
        spend_up_to: false,
    })
}

pub async fn prepare_blocked_shield_rescue_preview(
    request: BlockedShieldRescuePreviewRequest,
    http: &HttpContext,
) -> Result<BlockedShieldRescuePreview> {
    let utxo = selected_blocked_shield_rescue_utxo(&request.session, &request.utxo_id)?;
    let eligibility = resolve_blocked_shield_rescue_eligibility(
        BlockedShieldRescueEligibilityRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain,
            view_session: request.view_session,
            session: request.session,
            vault_store: request.vault_store,
            utxo_id: request.utxo_id,
        },
        http,
    )
    .await?;
    let origin_address = eligibility
        .origin_address
        .ok_or_else(|| eyre!("blocked Shield refund origin is unresolved"))?;
    let public_account_uuid = eligibility
        .public_account_uuid
        .ok_or_else(|| eyre!("blocked Shield refund origin Public account is unavailable"))?;
    if !eligibility.eligible {
        return Err(eyre!(
            "blocked Shield refund is unavailable: {}",
            eligibility
                .disabled_reason
                .as_deref()
                .unwrap_or("eligibility check failed")
        ));
    }

    Ok(BlockedShieldRescuePreview {
        chain_id: request.chain_id,
        utxo_id: request.utxo_id,
        token: utxo.token_address(),
        amount: utxo.note.value,
        source_tx_hash: utxo.source.tx_hash,
        origin_address,
        public_account_uuid,
        public_account_label: eligibility.public_account_label,
    })
}

pub(super) async fn prepare_blocked_shield_rescue_plan(
    request: &BlockedShieldRescueSelfBroadcastRequest,
    http: &HttpContext,
) -> Result<PreparedBlockedShieldRescuePlan> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }
    let chain = effective_desktop_chain_config(request.chain_id, request.effective_chain.as_ref())?;
    let utxo = selected_blocked_shield_rescue_utxo(&request.session, &request.utxo_id)?;
    let token = utxo.token_address();
    let amount = utxo.note.value;
    let eligibility = resolve_blocked_shield_rescue_eligibility(
        BlockedShieldRescueEligibilityRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain.clone(),
            view_session: Arc::clone(&request.view_session),
            session: Arc::clone(&request.session),
            vault_store: Arc::clone(&request.vault_store),
            utxo_id: request.utxo_id,
        },
        http,
    )
    .await?;
    if !eligibility.eligible {
        return Err(eyre!(
            "blocked Shield refund is unavailable: {}",
            eligibility
                .disabled_reason
                .as_deref()
                .unwrap_or("eligibility check failed")
        ));
    }
    let origin_address = eligibility
        .origin_address
        .ok_or_else(|| eyre!("blocked Shield refund origin is unresolved"))?;
    let public_account_uuid = matched_blocked_shield_rescue_public_account_uuid(
        eligibility.public_account_uuid.as_deref(),
        request.requested_public_account_uuid.as_deref(),
    )?;

    let artifact_source = artifact_source(http, request.session.db.as_ref());
    let prover = ProverService::new_with_db(&artifact_source, &request.session.db);
    let chain_handle = request
        .session
        .sync_manager
        .chain_handle(&request.session.chain_key)
        .await
        .ok_or_else(|| eyre!("chain handle not found for chain {}", request.chain_id))?;
    let mut forest = chain_handle.forest.read().await.clone();
    forest.compute_roots();

    let rescue_utxos = vec![utxo.clone()];
    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::SelectingPrivateNotes,
    );
    let selection_info = unshield_selection_info(&rescue_utxos, token, amount, false)
        .wrap_err("select blocked Shield refund note")?;
    if selection_info.input_count != 1 || selection_info.max_spendable != amount {
        return Err(eyre!(
            "blocked Shield refund must select exactly the chosen UTXO"
        ));
    }

    let signer = request.spend_authorization.signer(
        request.vault_store.as_ref(),
        request.view_session.wallet_id(),
        "blocked Shield refund",
    )?;
    let tx_builder = TransactionBuilder {
        chain_type: 0,
        chain_id: request.chain_id,
        railgun_contract: chain.railgun_contract,
        relay_adapt_contract: chain.relay_adapt_contract,
    };
    let unshield_request = RailgunUnshieldRequest {
        token_address: token,
        amount,
        recipient: origin_address,
        mode: UnshieldMode::Token,
        verify_proof: request.verify_proof,
        spend_up_to: false,
        broadcaster_fee: None,
        min_gas_price: 0,
    };

    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::ProvingTransaction,
    );
    let plan = tx_builder
        .build_unshield_plan_with_signer(
            &request.view_session.scan_keys(),
            &signer,
            &forest,
            &rescue_utxos,
            unshield_request,
            &prover,
        )
        .await
        .wrap_err("build blocked Shield refund calldata")?;
    validate_blocked_shield_rescue_plan(&plan, &request.utxo_id, token, amount, origin_address)?;

    Ok(PreparedBlockedShieldRescuePlan {
        plan,
        public_account_uuid,
    })
}

pub(super) fn selected_blocked_shield_rescue_utxo(
    session: &WalletSession,
    utxo_id: &BlockedShieldRescueUtxoId,
) -> Result<Utxo> {
    let snapshot = session
        .handle
        .current_snapshot()
        .ok_or_else(|| eyre!("wallet state is unavailable while synchronization resets"))?;
    blocked_shield_rescue_candidate_from_records(
        &snapshot.utxos,
        &snapshot.pending_overlay,
        utxo_id,
    )
    .ok_or_else(|| eyre!("selected UTXO is not an unspent blocked Shield that can be refunded"))
}

pub(crate) fn matched_blocked_shield_rescue_public_account_uuid(
    matched: Option<&str>,
    requested: Option<&str>,
) -> Result<String> {
    let matched =
        matched.ok_or_else(|| eyre!("blocked Shield refund origin account is unavailable"))?;
    if let Some(requested) = requested
        && requested != matched
    {
        return Err(eyre!(
            "blocked Shield refund gas payer must be the matched origin Public account"
        ));
    }
    Ok(matched.to_string())
}

pub(crate) fn validate_blocked_shield_rescue_plan(
    plan: &UnshieldPlan,
    utxo_id: &BlockedShieldRescueUtxoId,
    token: Address,
    amount: U256,
    origin_address: Address,
) -> Result<()> {
    if plan.inputs.len() != 1 {
        return Err(eyre!(
            "blocked Shield refund must spend exactly one private input"
        ));
    }
    let input = &plan.inputs[0].utxo;
    if !blocked_shield_rescue_utxo_matches(input, utxo_id) {
        return Err(eyre!("blocked Shield refund selected an unexpected UTXO"));
    }
    if input.note.value != amount || plan.unshield_note.value != amount {
        return Err(eyre!(
            "blocked Shield refund must spend the full UTXO value"
        ));
    }
    let expected_unshield = Note::new_unshield(origin_address, token, amount);
    if plan.unshield_note.token_hash != expected_unshield.token_hash
        || plan.unshield_note.npk != expected_unshield.npk
    {
        return Err(eyre!(
            "blocked Shield refund must unshield the exact token to the origin address"
        ));
    }
    if plan.unshield_notes.len() != 1 {
        return Err(eyre!(
            "blocked Shield refund must have exactly one public output"
        ));
    }
    if plan.broadcaster_fee_note.is_some() {
        return Err(eyre!(
            "blocked Shield refund cannot include a broadcaster fee note"
        ));
    }
    if plan.change_note.is_some() {
        return Err(eyre!("blocked Shield refund cannot create private change"));
    }
    for chunk in &plan.chunks {
        if chunk.private_output_count() != Some(0) {
            return Err(eyre!("blocked Shield refund cannot create private outputs"));
        }
    }
    Ok(())
}

pub(super) async fn prepare_desktop_send_plan_without_broadcaster_fee(
    request: DesktopSendPlanRequest<'_>,
    http: &HttpContext,
) -> Result<PreparedPrivatePlan<SendPlan>> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }

    let recipient = request.recipient.trim();
    let recipient_data = parse_railgun_recipient(recipient)?;
    let chain = effective_desktop_chain_config(request.chain_id, request.effective_chain)?;
    let artifact_source = artifact_source(http, request.session.db.as_ref());
    let prover = ProverService::new_with_db(&artifact_source, &request.session.db);
    let chain_handle = request
        .session
        .sync_manager
        .chain_handle(&request.session.chain_key)
        .await
        .ok_or_else(|| eyre!("chain handle not found for chain {}", request.chain_id))?;
    let mut forest = chain_handle.forest.read().await.clone();
    forest.compute_roots();

    let utxos = request.session.unspent_utxos();
    let send_request = RailgunSendRequest {
        token_address: request.token,
        amount: request.amount,
        recipient: recipient_data,
        verify_proof: request.verify_proof,
        spend_up_to: false,
        broadcaster_fee: None,
        min_gas_price: 0,
    };
    update_transaction_generation_stage(
        request.progress_tx,
        TransactionGenerationStage::SelectingPrivateNotes,
    );
    let selection_info = send_selection_info(&utxos, request.token, request.amount, false)
        .wrap_err("select POI-verified send notes")?;

    let signer = request.spend_authorization.into_signer(
        request.vault_store,
        request.view_session.wallet_id(),
        "send",
    )?;

    let tx_builder = TransactionBuilder {
        chain_type: 0,
        chain_id: request.chain_id,
        railgun_contract: chain.railgun_contract,
        relay_adapt_contract: chain.relay_adapt_contract,
    };

    update_transaction_generation_stage(
        request.progress_tx,
        TransactionGenerationStage::ProvingTransaction,
    );
    let plan = tx_builder
        .build_send_plan_with_signer(
            &request.view_session.scan_keys(),
            &signer,
            &forest,
            &utxos,
            send_request,
            &prover,
        )
        .await
        .wrap_err("build desktop send calldata")?;

    Ok(PreparedPrivatePlan {
        plan,
        max_spendable: selection_info.max_spendable,
        prover,
    })
}

pub(super) async fn persist_manual_unshield_pending_pois(
    plan: &DesktopUnshieldPreparedPlan,
    session: &WalletSession,
    chain_id: u64,
    _wallet_id: &str,
    prover: &ProverService,
    verify_proof: bool,
    http: &HttpContext,
    operation_label: &'static str,
) -> Result<()> {
    let (pending_poi_list_keys, pending_pois) = active_list_pre_transaction_pois(
        plan.chunks(),
        session,
        chain_id,
        prover,
        verify_proof,
        http,
        operation_label,
    )
    .await?;
    match plan {
        DesktopUnshieldPreparedPlan::Single(plan) => {
            persist_pending_unshield_output_poi_contexts(
                session,
                &plan.chunks,
                &pending_pois,
                &pending_poi_list_keys,
                false,
                false,
            )
            .await?;
        }
        DesktopUnshieldPreparedPlan::Composite(plan) => {
            persist_pending_composite_unshield_output_poi_contexts(
                session,
                &plan.chunks,
                &plan.private_output_roles,
                &pending_pois,
                &pending_poi_list_keys,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn persist_manual_send_pending_pois(
    plan: &SendPlan,
    session: &WalletSession,
    chain_id: u64,
    _wallet_id: &str,
    prover: &ProverService,
    verify_proof: bool,
    http: &HttpContext,
    operation_label: &'static str,
) -> Result<()> {
    let (pending_poi_list_keys, pending_pois) = active_list_pre_transaction_pois(
        &plan.chunks,
        session,
        chain_id,
        prover,
        verify_proof,
        http,
        operation_label,
    )
    .await?;
    persist_pending_send_output_poi_contexts(
        session,
        &plan.chunks,
        &pending_pois,
        &pending_poi_list_keys,
        false,
        false,
    )
    .await?;
    Ok(())
}

pub(crate) fn unshield_chunks_require_pending_output_pois(chunks: &[TransactionPlanChunk]) -> bool {
    chunks
        .iter()
        .any(|chunk| chunk.private_output_count().is_none_or(|count| count > 0))
}

pub(super) fn prepared_unshield_call_from_plan(
    chain_id: u64,
    token: Address,
    amount: U256,
    fee_mode: FeeHandlingMode,
    recipient: Address,
    unwrap: bool,
    max_spendable: U256,
    plan: &DesktopUnshieldPreparedPlan,
    native_top_up: Option<DesktopNativeTopUpPlan>,
) -> PreparedUnshieldCall {
    PreparedUnshieldCall {
        chain_id,
        token,
        amount,
        fee_mode,
        recipient,
        unwrap,
        max_spendable,
        transaction_count: plan.transaction_count(),
        input_count: plan.input_count(),
        private_output_count: plan.private_output_count(),
        public_output_count: plan.public_output_count(),
        to: plan.call_to(),
        data: hex::encode_prefixed(plan.call_data()),
        native_top_up,
    }
}

pub(super) fn prepared_send_call_from_plan(
    chain_id: u64,
    token: Address,
    amount: U256,
    recipient: String,
    max_spendable: U256,
    plan: &SendPlan,
) -> PreparedSendCall {
    PreparedSendCall {
        chain_id,
        token,
        amount,
        recipient,
        max_spendable,
        transaction_count: plan.transaction_count(),
        input_count: plan.input_count(),
        private_output_count: plan.private_output_count(),
        public_output_count: plan.public_output_count(),
        to: plan.call.to,
        data: hex::encode_prefixed(&plan.call.data),
    }
}

pub async fn prepare_desktop_unshield_calldata(
    request: DesktopUnshieldCalldataRequest,
    http: &HttpContext,
) -> Result<PreparedUnshieldCall> {
    let prepared = prepare_desktop_unshield_plan_without_broadcaster_fee(
        DesktopUnshieldPlanRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain.as_ref(),
            view_session: request.view_session.as_ref(),
            session: request.session.as_ref(),
            vault_store: request.vault_store.as_ref(),
            spend_authorization: request.spend_authorization,
            token: request.token,
            amount: request.amount,
            fee_mode: request.fee_mode,
            recipient: request.recipient,
            unwrap: request.unwrap,
            native_top_up: request.native_top_up,
            verify_proof: request.verify_proof,
            progress_tx: request.progress_tx.as_ref(),
        },
        http,
    )
    .await?;

    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::GeneratingPoiProofs,
    );
    persist_manual_unshield_pending_pois(
        &prepared.plan,
        request.session.as_ref(),
        request.chain_id,
        request.view_session.wallet_id(),
        &prepared.prover,
        request.verify_proof,
        http,
        "generate manual unshield pending output pre-transaction POI",
    )
    .await?;

    Ok(prepared_unshield_call_from_plan(
        request.chain_id,
        request.token,
        request.amount,
        request.fee_mode,
        request.recipient,
        request.unwrap,
        prepared.max_spendable,
        &prepared.plan,
        prepared.native_top_up,
    ))
}

pub async fn prepare_desktop_send_calldata(
    request: DesktopSendCalldataRequest,
    http: &HttpContext,
) -> Result<PreparedSendCall> {
    let recipient = request.recipient.trim().to_string();
    let prepared = prepare_desktop_send_plan_without_broadcaster_fee(
        DesktopSendPlanRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain.as_ref(),
            view_session: request.view_session.as_ref(),
            session: request.session.as_ref(),
            vault_store: request.vault_store.as_ref(),
            spend_authorization: request.spend_authorization,
            token: request.token,
            amount: request.amount,
            recipient: &recipient,
            verify_proof: request.verify_proof,
            progress_tx: request.progress_tx.as_ref(),
        },
        http,
    )
    .await?;

    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::GeneratingPoiProofs,
    );
    persist_manual_send_pending_pois(
        &prepared.plan,
        request.session.as_ref(),
        request.chain_id,
        request.view_session.wallet_id(),
        &prepared.prover,
        request.verify_proof,
        http,
        "generate manual send pending output pre-transaction POI",
    )
    .await?;

    Ok(prepared_send_call_from_plan(
        request.chain_id,
        request.token,
        request.amount,
        recipient,
        prepared.max_spendable,
        &prepared.plan,
    ))
}

pub async fn estimate_desktop_unshield_public_broadcaster_cost(
    request: DesktopUnshieldPublicBroadcasterEstimateRequest,
    http: &HttpContext,
) -> Result<PublicBroadcasterCostEstimate> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }
    let chain = effective_desktop_chain_config(request.chain_id, request.effective_chain.as_ref())?;
    if request.unwrap && !is_effective_wrapped_native_token(request.chain_id, request.token, &chain)
    {
        return Err(eyre!("selected token does not support unwrap-to-native"));
    }

    let policy = request.fee_policy;
    let anchor_rate = public_broadcaster_anchor_rate_for_policy(
        request.anchor_cache.as_ref(),
        request.chain_id,
        request.fee_token,
    );
    let candidates = public_broadcaster_candidates(
        &request.fee_rows,
        request.chain_id,
        request.fee_token,
        if request.unwrap || request.native_top_up.is_some() {
            Some(chain.relay_adapt_contract)
        } else {
            None
        },
        SystemTime::now(),
        policy,
        anchor_rate,
    );
    let broadcaster = select_public_broadcaster_with_policy_and_trust(
        &candidates,
        &request.selection,
        policy,
        &request.trust_filter,
    )?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls.clone(), http);
    let min_gas_price = buffered_gas_price_from_rpc_pool(&query_rpc_pool, &chain.gas).await?;
    let utxos = request.session.unspent_utxos();
    let same_token_fee = request.fee_token == request.token;
    let native_top_up = request
        .native_top_up
        .as_ref()
        .map(|top_up| {
            desktop_native_top_up_plan_for_estimate(
                request.chain_id,
                &chain,
                request.token,
                request.recipient,
                request.unwrap,
                top_up,
                request.amount,
            )
        })
        .transpose()?;
    let initial_fee_amount =
        initial_public_broadcaster_fee_amount(&broadcaster, min_gas_price, same_token_fee, || {
            let seed_split = public_broadcaster_amount_split_for_tokens_and_protocol(
                request.amount,
                U256::ZERO,
                request.fee_mode,
                same_token_fee,
                RAILGUN_PROTOCOL_FEE_BPS,
            )?;
            if let Some(native_top_up) = &native_top_up {
                return native_top_up_approximate_shape(
                    &utxos,
                    request.token,
                    request.fee_token,
                    seed_split.receiver_amount,
                    U256::ZERO,
                    native_top_up,
                );
            }
            let selection = unshield_selection_info_with_separate_broadcaster_fee_seed(
                &utxos,
                request.token,
                request.fee_token,
                seed_split.receiver_amount,
                false,
            )
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    U256::ZERO,
                    seed_split.fee_mode,
                    same_token_fee,
                    RAILGUN_PROTOCOL_FEE_BPS,
                )
            })?;
            Ok(unshield_approximate_shape(
                &selection,
                selection.max_spendable,
                request.unwrap,
            ))
        })?;

    let mut estimate = approximate_public_broadcaster_cost(
        broadcaster,
        request.token,
        request.fee_token,
        request.amount,
        request.fee_mode,
        RAILGUN_PROTOCOL_FEE_BPS,
        min_gas_price,
        initial_fee_amount,
        |split| {
            if let Some(native_top_up) = &native_top_up {
                return native_top_up_approximate_shape(
                    &utxos,
                    request.token,
                    request.fee_token,
                    split.receiver_amount,
                    split.fee_amount,
                    native_top_up,
                );
            }
            let selection = unshield_selection_info_with_broadcaster_fee_token(
                &utxos,
                request.token,
                request.fee_token,
                split.receiver_amount,
                split.fee_amount,
                false,
            )
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    split.fee_amount,
                    split.fee_mode,
                    same_token_fee,
                    RAILGUN_PROTOCOL_FEE_BPS,
                )
            })?;
            Ok(unshield_approximate_shape(
                &selection,
                selection.max_spendable,
                request.unwrap,
            ))
        },
    )?;
    let reported_amounts = public_broadcaster_reported_amounts(
        request.token,
        request.fee_token,
        PublicBroadcasterAmountSplit {
            entered_amount: estimate.entered_amount,
            receiver_amount: estimate.receiver_amount,
            total_private_spend: estimate.total_private_spend,
            fee_amount: estimate.fee_amount,
            fee_mode: estimate.fee_mode,
        },
        RAILGUN_PROTOCOL_FEE_BPS,
        native_top_up.as_ref(),
    );
    estimate.recipient_amount = reported_amounts.recipient_amount;
    estimate.total_private_spend = reported_amounts.total_private_spend;
    estimate.protocol_fee_amount = reported_amounts.protocol_fee_amount;
    estimate.native_top_up = native_top_up;
    Ok(estimate)
}

pub async fn estimate_desktop_send_public_broadcaster_cost(
    request: DesktopSendPublicBroadcasterEstimateRequest,
    http: &HttpContext,
) -> Result<PublicBroadcasterCostEstimate> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }
    parse_railgun_recipient(&request.recipient)?;

    let chain = effective_desktop_chain_config(request.chain_id, request.effective_chain.as_ref())?;
    let policy = request.fee_policy;
    let anchor_rate = public_broadcaster_anchor_rate_for_policy(
        request.anchor_cache.as_ref(),
        request.chain_id,
        request.fee_token,
    );
    let candidates = public_broadcaster_candidates(
        &request.fee_rows,
        request.chain_id,
        request.fee_token,
        None,
        SystemTime::now(),
        policy,
        anchor_rate,
    );
    let broadcaster = select_public_broadcaster_with_policy_and_trust(
        &candidates,
        &request.selection,
        policy,
        &request.trust_filter,
    )?;
    let query_rpc_pool = query_rpc_pool_with_http_client(chain.rpc_urls.clone(), http);
    let min_gas_price = buffered_gas_price_from_rpc_pool(&query_rpc_pool, &chain.gas).await?;
    let utxos = request.session.unspent_utxos();
    let same_token_fee = request.fee_token == request.token;
    let initial_fee_amount =
        initial_public_broadcaster_fee_amount(&broadcaster, min_gas_price, same_token_fee, || {
            let selection = send_selection_info_with_separate_broadcaster_fee_seed(
                &utxos,
                request.token,
                request.fee_token,
                request.amount,
                false,
            )
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    U256::ZERO,
                    FeeHandlingMode::AddToAmount,
                    same_token_fee,
                    U256::ZERO,
                )
            })?;
            Ok(send_approximate_shape(&selection, selection.max_spendable))
        })?;

    approximate_public_broadcaster_cost(
        broadcaster,
        request.token,
        request.fee_token,
        request.amount,
        request.fee_mode,
        U256::ZERO,
        min_gas_price,
        initial_fee_amount,
        |split| {
            let selection = send_selection_info_with_broadcaster_fee_token(
                &utxos,
                request.token,
                request.fee_token,
                split.receiver_amount,
                split.fee_amount,
                false,
            )
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    split.fee_amount,
                    split.fee_mode,
                    same_token_fee,
                    U256::ZERO,
                )
            })?;
            Ok(send_approximate_shape(&selection, selection.max_spendable))
        },
    )
}

pub async fn submit_desktop_unshield_public_broadcaster(
    request: DesktopUnshieldPublicBroadcasterRequest,
    http: &HttpContext,
) -> Result<PublicBroadcasterSubmissionResult> {
    let waku = Arc::clone(&request.waku);
    let timeout = request.response_timeout;
    let republish_interval = request.republish_interval;
    let progress_tx = request.progress_tx.clone();
    let session = Arc::clone(&request.session);
    let prepared = prepare_desktop_unshield_public_broadcaster(request, http).await?;
    let pending_spent_inputs = prepared.plan.input_utxos();
    let result = submit_public_broadcaster_plan(
        waku,
        prepared.plan.call_to(),
        prepared.plan.call_data(),
        prepared.pre_transaction_pois_per_txid_leaf_per_list,
        prepared.broadcaster,
        prepared.action_token,
        prepared.fee_token,
        prepared.entered_amount,
        prepared.receiver_amount,
        prepared.recipient_amount,
        prepared.total_private_spend,
        prepared.fee_amount,
        prepared.protocol_fee_amount,
        prepared.protocol_fee_bps,
        prepared.fee_mode,
        prepared.gas_limit,
        prepared.min_gas_price,
        prepared.bound_min_gas_price,
        prepared.transaction_count,
        prepared.input_count,
        prepared.private_output_count,
        prepared.public_output_count,
        prepared.relay_call_count,
        prepared.uses_relay_adapt,
        prepared.native_top_up,
        progress_tx,
        timeout,
        republish_interval,
    )
    .await?;
    mark_submitted_inputs_pending_spent(&session, &pending_spent_inputs, &result).await;
    Ok(result)
}

pub async fn submit_desktop_send_public_broadcaster(
    request: DesktopSendPublicBroadcasterRequest,
    http: &HttpContext,
) -> Result<PublicBroadcasterSubmissionResult> {
    let waku = Arc::clone(&request.waku);
    let timeout = request.response_timeout;
    let republish_interval = request.republish_interval;
    let progress_tx = request.progress_tx.clone();
    let session = Arc::clone(&request.session);
    let prepared = prepare_desktop_send_public_broadcaster(request, http).await?;
    let pending_spent_inputs = prepared
        .plan
        .inputs
        .iter()
        .map(|input| input.utxo.clone())
        .collect::<Vec<_>>();
    let result = submit_public_broadcaster_plan(
        waku,
        prepared.plan.call.to,
        prepared.plan.call.data,
        prepared.pre_transaction_pois_per_txid_leaf_per_list,
        prepared.broadcaster,
        prepared.action_token,
        prepared.fee_token,
        prepared.entered_amount,
        prepared.receiver_amount,
        prepared.recipient_amount,
        prepared.total_private_spend,
        prepared.fee_amount,
        prepared.protocol_fee_amount,
        prepared.protocol_fee_bps,
        prepared.fee_mode,
        prepared.gas_limit,
        prepared.min_gas_price,
        prepared.bound_min_gas_price,
        prepared.transaction_count,
        prepared.input_count,
        prepared.private_output_count,
        prepared.public_output_count,
        prepared.relay_call_count,
        prepared.uses_relay_adapt,
        prepared.native_top_up,
        progress_tx,
        timeout,
        republish_interval,
    )
    .await?;
    mark_submitted_inputs_pending_spent(&session, &pending_spent_inputs, &result).await;
    Ok(result)
}

pub async fn submit_desktop_unshield_self_broadcast(
    request: DesktopUnshieldSelfBroadcastRequest,
    http: &HttpContext,
) -> Result<DesktopSelfBroadcastResult> {
    let prepared = prepare_desktop_unshield_plan_without_broadcaster_fee(
        DesktopUnshieldPlanRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain.as_ref(),
            view_session: request.view_session.as_ref(),
            session: request.session.as_ref(),
            vault_store: request.vault_store.as_ref(),
            spend_authorization: request.spend_authorization,
            token: request.token,
            amount: request.amount,
            fee_mode: request.fee_mode,
            recipient: request.recipient,
            unwrap: request.unwrap,
            native_top_up: request.native_top_up,
            verify_proof: request.verify_proof,
            progress_tx: request.progress_tx.as_ref(),
        },
        http,
    )
    .await?;
    let pending_output_pois_required =
        unshield_chunks_require_pending_output_pois(prepared.plan.chunks());
    emit_self_broadcast_event(
        request.event_tx.as_ref(),
        SelfBroadcastSessionEvent::PendingOutputPoiProofsRequired {
            required: pending_output_pois_required,
        },
    );
    if pending_output_pois_required {
        update_transaction_generation_stage(
            request.progress_tx.as_ref(),
            TransactionGenerationStage::GeneratingPoiProofs,
        );
        persist_manual_unshield_pending_pois(
            &prepared.plan,
            request.session.as_ref(),
            request.chain_id,
            request.view_session.wallet_id(),
            &prepared.prover,
            request.verify_proof,
            http,
            "generate self-broadcast unshield pending output pre-transaction POI",
        )
        .await?;
    }
    let pending_spent_inputs = prepared.plan.input_utxos();
    let mut result = submit_self_broadcast_plan(
        request.chain_id,
        request.effective_chain.as_ref(),
        request.view_session.as_ref(),
        request.vault_store.as_ref(),
        request
            .vault_password
            .as_ref()
            .map(|password| password.as_str()),
        request.trezor_pin_matrix_provider,
        request.public_account_uuid,
        Arc::clone(&request.session),
        prepared.plan.call_to(),
        prepared.plan.call_data(),
        pending_spent_inputs,
        prepared.native_top_up.is_some(),
        request.gas_fee,
        request.progress_tx,
        request.command_rx,
        request.event_tx,
        http,
    )
    .await?;
    result.native_top_up = prepared.native_top_up;
    Ok(result)
}

pub async fn submit_blocked_shield_rescue_self_broadcast(
    request: BlockedShieldRescueSelfBroadcastRequest,
    http: &HttpContext,
) -> Result<DesktopSelfBroadcastResult> {
    let prepared = prepare_blocked_shield_rescue_plan(&request, http).await?;
    let pending_output_pois_required =
        unshield_chunks_require_pending_output_pois(&prepared.plan.chunks);
    emit_self_broadcast_event(
        request.event_tx.as_ref(),
        SelfBroadcastSessionEvent::PendingOutputPoiProofsRequired {
            required: pending_output_pois_required,
        },
    );
    if pending_output_pois_required {
        return Err(eyre!(
            "blocked Shield refund plan unexpectedly requires private output POI proofs"
        ));
    }
    let pending_spent_inputs = prepared
        .plan
        .inputs
        .iter()
        .map(|input| input.utxo.clone())
        .collect::<Vec<_>>();
    submit_self_broadcast_plan(
        request.chain_id,
        request.effective_chain.as_ref(),
        request.view_session.as_ref(),
        request.vault_store.as_ref(),
        Some(request.vault_password.as_str()),
        request.trezor_pin_matrix_provider,
        prepared.public_account_uuid,
        Arc::clone(&request.session),
        prepared.plan.call.to,
        prepared.plan.call.data,
        pending_spent_inputs,
        false,
        request.gas_fee,
        request.progress_tx,
        request.command_rx,
        request.event_tx,
        http,
    )
    .await
}

pub async fn submit_desktop_send_self_broadcast(
    request: DesktopSendSelfBroadcastRequest,
    http: &HttpContext,
) -> Result<DesktopSelfBroadcastResult> {
    let recipient = request.recipient.trim().to_string();
    let prepared = prepare_desktop_send_plan_without_broadcaster_fee(
        DesktopSendPlanRequest {
            chain_id: request.chain_id,
            effective_chain: request.effective_chain.as_ref(),
            view_session: request.view_session.as_ref(),
            session: request.session.as_ref(),
            vault_store: request.vault_store.as_ref(),
            spend_authorization: request.spend_authorization,
            token: request.token,
            amount: request.amount,
            recipient: &recipient,
            verify_proof: request.verify_proof,
            progress_tx: request.progress_tx.as_ref(),
        },
        http,
    )
    .await?;
    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::GeneratingPoiProofs,
    );
    persist_manual_send_pending_pois(
        &prepared.plan,
        request.session.as_ref(),
        request.chain_id,
        request.view_session.wallet_id(),
        &prepared.prover,
        request.verify_proof,
        http,
        "generate self-broadcast send pending output pre-transaction POI",
    )
    .await?;
    let pending_spent_inputs = prepared
        .plan
        .inputs
        .iter()
        .map(|input| input.utxo.clone())
        .collect::<Vec<_>>();
    submit_self_broadcast_plan(
        request.chain_id,
        request.effective_chain.as_ref(),
        request.view_session.as_ref(),
        request.vault_store.as_ref(),
        request
            .vault_password
            .as_ref()
            .map(|password| password.as_str()),
        request.trezor_pin_matrix_provider,
        request.public_account_uuid,
        Arc::clone(&request.session),
        prepared.plan.call.to,
        prepared.plan.call.data,
        pending_spent_inputs,
        false,
        request.gas_fee,
        request.progress_tx,
        request.command_rx,
        request.event_tx,
        http,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    use local_db::{DbConfig, DbStore};

    const TEST_PASSWORD: &str = "correct horse battery staple";
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn temp_db_root() -> PathBuf {
        let dir = std::env::temp_dir().join("railoxide-wallet-ops-private-tx-tests");
        fs::create_dir_all(&dir).expect("create temp db parent");
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        dir.join(format!("db-{pid}-{nanos}"))
    }

    fn test_utxo(token: Address, value: U256) -> Utxo {
        Utxo::new(
            Note::new_unshield(Address::ZERO, token, value),
            0,
            0,
            UtxoSource {
                tx_hash: FixedBytes::ZERO,
                block_number: 0,
                block_timestamp: 0,
            },
            UtxoCommitmentKind::Transact,
        )
    }

    fn test_chain_config(wrapped_native: Address) -> EffectiveDesktopChainConfig {
        EffectiveDesktopChainConfig {
            rpc_urls: Vec::new(),
            railgun_contract: Address::ZERO,
            relay_adapt_contract: Address::ZERO,
            wrapped_native_token: Some(wrapped_native),
            gas: settings::EffectiveChainGasSettings {
                gas_limit_buffer: GAS_LIMIT_BUFFER,
                gas_price_buffer_numerator: GAS_PRICE_BUFFER_NUMERATOR as u64,
                gas_price_buffer_denominator: GAS_PRICE_BUFFER_DENOMINATOR as u64,
            },
        }
    }

    #[test]
    fn native_top_up_estimate_rejects_unwrap_as_unsupported() {
        let wrapped_native = wrapped_native_token_for_chain(1).expect("ethereum wrapped native");
        let chain = test_chain_config(wrapped_native);
        let top_up = DesktopNativeTopUpRequest {
            public_account_uuid: "pub-1".to_string(),
            native_balance: U256::ZERO,
        };
        let policy = native_top_up_policy_for_chain(1).expect("ethereum native top-up policy");

        let error = desktop_native_top_up_plan_for_estimate(
            1,
            &chain,
            wrapped_native,
            Address::from([0x52; 20]),
            true,
            &top_up,
            policy.offer_threshold,
        )
        .expect_err("unwrap-to-native cannot be combined with native top-up");
        assert_eq!(
            error.to_string(),
            "native top-up cannot be combined with unwrap-to-native output"
        );
    }

    #[test]
    fn native_top_up_plan_validation_counts_wrapped_native_broadcaster_fee() {
        let root_dir = temp_db_root();
        let db = Arc::new(
            DbStore::open(DbConfig {
                root_dir: root_dir.clone(),
            })
            .expect("open db"),
        );
        let store = vault::DesktopVaultStore::from_db(Arc::clone(&db));
        store
            .create_vault_with_params(TEST_PASSWORD, vault::KdfParams::new(1024, 1, 1))
            .expect("create vault");
        let wallet_id = "wallet-1";
        let metadata = store
            .new_wallet_metadata(
                TEST_PASSWORD,
                wallet_id,
                0,
                vault::WalletSource::Generated,
                "Wallet",
            )
            .expect("wallet metadata");
        store
            .import_wallet_mnemonic_with_metadata(
                TEST_PASSWORD,
                wallet_id,
                0,
                "english",
                TEST_MNEMONIC,
                &metadata,
            )
            .expect("store wallet");
        let view_session = store
            .load_view_session(TEST_PASSWORD, wallet_id)
            .expect("load view session");
        let account = store
            .import_public_account(
                TEST_PASSWORD,
                &view_session,
                "0x0101010101010101010101010101010101010101010101010101010101010101",
                Some("Recipient"),
                true,
            )
            .expect("import public account");
        let wrapped_native = wrapped_native_token_for_chain(1).expect("ethereum wrapped native");
        let token = Address::from([0x51; 20]);
        let receiver_amount = U256::from(25_u64);
        let top_up = DesktopNativeTopUpRequest {
            public_account_uuid: account.public_account_uuid.clone(),
            native_balance: U256::ZERO,
        };
        let chain = test_chain_config(wrapped_native);
        let policy = native_top_up_policy_for_chain(1).expect("ethereum native top-up policy");
        let required_without_fee = native_top_up_required_wrapped_native_amount(
            token,
            wrapped_native,
            receiver_amount,
            policy.top_up_amount,
        );
        let utxos = vec![test_utxo(wrapped_native, required_without_fee)];

        let unwrap_error = desktop_native_top_up_plan_from_unshield_fields(
            1,
            &chain,
            &view_session,
            &store,
            wrapped_native,
            account.address,
            true,
            &top_up,
            policy.offer_threshold,
            None,
            U256::ZERO,
            &utxos,
        )
        .expect_err("unwrap-to-native cannot be combined with native top-up");
        assert_eq!(
            unwrap_error.to_string(),
            "native top-up cannot be combined with unwrap-to-native output"
        );

        desktop_native_top_up_plan_from_unshield_fields(
            1,
            &chain,
            &view_session,
            &store,
            token,
            account.address,
            false,
            &top_up,
            receiver_amount,
            Some(wrapped_native),
            U256::ZERO,
            &utxos,
        )
        .expect("zero wrapped-native broadcaster fee fits available balance");

        let fee_amount = U256::from(1_u64);
        let error = desktop_native_top_up_plan_from_unshield_fields(
            1,
            &chain,
            &view_session,
            &store,
            token,
            account.address,
            false,
            &top_up,
            receiver_amount,
            Some(wrapped_native),
            fee_amount,
            &utxos,
        )
        .expect_err("wrapped-native broadcaster fee should require additional balance");
        let expected_required = required_without_fee.saturating_add(fee_amount);
        let message = error.to_string();
        assert!(message.contains("native top-up wrapped-native max spendable"));
        assert!(message.contains(&format!("required: {expected_required}")));

        drop(store);
        drop(db);
        fs::remove_dir_all(root_dir).expect("remove temp db dir");
    }
}
