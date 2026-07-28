use super::*;
use eyre::eyre;

pub(super) async fn prepare_desktop_unshield_public_broadcaster(
    request: DesktopUnshieldPublicBroadcasterRequest,
    http: &HttpContext,
) -> Result<PreparedPublicBroadcasterPlan<DesktopUnshieldPreparedPlan>> {
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

    let PublicBroadcasterSetup {
        chain,
        broadcaster,
        query_rpc_pool,
        min_gas_price,
        prover,
        forest,
        utxos,
    } = public_broadcaster_setup(
        &request.session,
        request.chain_id,
        request.effective_chain.as_ref(),
        request.fee_token,
        &request.fee_rows,
        &request.selection,
        request.unwrap || request.native_top_up.is_some(),
        request.fee_policy,
        &request.trust_filter,
        request.anchor_cache.as_ref(),
        http,
    )
    .await?;
    let bound_min_gas_price =
        public_broadcaster_bound_min_gas_price(request.chain_id, min_gas_price);
    let same_token_fee = request.fee_token == request.token;
    let initial_split = public_broadcaster_amount_split_for_tokens_and_protocol(
        request.amount,
        U256::ZERO,
        request.fee_mode,
        same_token_fee,
        RAILGUN_PROTOCOL_FEE_BPS,
    )?;
    let initial_native_top_up = request
        .native_top_up
        .as_ref()
        .map(|top_up| {
            desktop_native_top_up_plan_from_unshield_fields(
                request.chain_id,
                &chain,
                request.view_session.as_ref(),
                request.vault_store.as_ref(),
                request.token,
                request.recipient,
                request.unwrap,
                top_up,
                initial_split.receiver_amount,
                Some(request.fee_token),
                U256::ZERO,
                &utxos,
            )
        })
        .transpose()?;
    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::SelectingPrivateNotes,
    );
    let seeded_fee_amount =
        initial_public_broadcaster_fee_amount(&broadcaster, min_gas_price, same_token_fee, || {
            if let Some(native_top_up) = &initial_native_top_up {
                return native_top_up_approximate_shape(
                    &utxos,
                    request.token,
                    request.fee_token,
                    initial_split.receiver_amount,
                    U256::ZERO,
                    native_top_up,
                );
            }
            let selection = unshield_selection_info_with_separate_broadcaster_fee_seed(
                &utxos,
                request.token,
                request.fee_token,
                initial_split.receiver_amount,
                false,
            )
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    U256::ZERO,
                    initial_split.fee_mode,
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
    let initial_fee_estimate = match approximate_public_broadcaster_cost(
        broadcaster.clone(),
        request.token,
        request.fee_token,
        request.amount,
        request.fee_mode,
        RAILGUN_PROTOCOL_FEE_BPS,
        min_gas_price,
        seeded_fee_amount,
        |split| {
            if let Some(native_top_up) = &initial_native_top_up {
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
    ) {
        Ok(estimate) => {
            tracing::info!(
                fee_amount = %estimate.fee_amount,
                gas_limit = estimate.gas_limit,
                min_gas_price,
                bound_min_gas_price,
                transaction_count = estimate.transaction_count,
                input_count = estimate.input_count,
                private_output_count = estimate.private_output_count,
                public_output_count = estimate.public_output_count,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "using approximate public broadcaster unshield fee for first proof"
            );
            Some(estimate)
        }
        Err(err) => {
            if !same_token_fee {
                return Err(err).wrap_err("estimate initial public broadcaster unshield fee");
            }
            tracing::warn!(
                ?err,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "failed to estimate initial same-token public broadcaster unshield fee; starting at zero"
            );
            None
        }
    };
    let initial_fee_amount = initial_fee_estimate
        .as_ref()
        .map_or(U256::ZERO, |estimate| estimate.fee_amount);

    let signer = request.spend_authorization.into_signer(
        request.vault_store.as_ref(),
        request.view_session.wallet_id(),
        "public broadcaster unshield",
    )?;

    let mode = if request.unwrap {
        UnshieldMode::UnwrapBase
    } else {
        UnshieldMode::Token
    };
    let tx_builder = TransactionBuilder {
        chain_type: 0,
        chain_id: request.chain_id,
        railgun_contract: chain.railgun_contract,
        relay_adapt_contract: chain.relay_adapt_contract,
    };

    let mut fee_amount = initial_fee_amount;
    for attempt in 1..=PUBLIC_BROADCASTER_FEE_ATTEMPTS {
        let split = public_broadcaster_amount_split_for_tokens_and_protocol(
            request.amount,
            fee_amount,
            request.fee_mode,
            same_token_fee,
            RAILGUN_PROTOCOL_FEE_BPS,
        )?;
        let native_top_up = request
            .native_top_up
            .as_ref()
            .map(|top_up| {
                desktop_native_top_up_plan_from_unshield_fields(
                    request.chain_id,
                    &chain,
                    request.view_session.as_ref(),
                    request.vault_store.as_ref(),
                    request.token,
                    request.recipient,
                    request.unwrap,
                    top_up,
                    split.receiver_amount,
                    Some(request.fee_token),
                    fee_amount,
                    &utxos,
                )
            })
            .transpose()?;
        update_transaction_generation_stage(
            request.progress_tx.as_ref(),
            TransactionGenerationStage::ProvingTransaction,
        );
        let proof_started = Instant::now();
        let plan = if let Some(native_top_up) = &native_top_up {
            let mut composite_request = native_top_up_composite_unshield_request(
                request.token,
                split.receiver_amount,
                request.recipient,
                request.unwrap,
                request.verify_proof,
                native_top_up,
            )?;
            composite_request.broadcaster_fee = Some(BroadcasterFeeOutput {
                recipient: broadcaster.address_data,
                token_address: request.fee_token,
                amount: fee_amount,
            });
            composite_request.min_gas_price = bound_min_gas_price;
            DesktopUnshieldPreparedPlan::Composite(
                tx_builder
                    .build_composite_unshield_plan_with_signer(
                        &request.view_session.scan_keys(),
                        &signer,
                        &forest,
                        &utxos,
                        composite_request,
                        &prover,
                    )
                    .await
                    .map_err(|error| {
                        public_broadcaster_build_error(
                            error,
                            fee_amount,
                            split.fee_mode,
                            same_token_fee,
                            RAILGUN_PROTOCOL_FEE_BPS,
                        )
                    })
                    .wrap_err("build public broadcaster composite unshield proof")?,
            )
        } else {
            let unshield_request = RailgunUnshieldRequest {
                token_address: request.token,
                amount: split.receiver_amount,
                recipient: request.recipient,
                mode,
                verify_proof: request.verify_proof,
                spend_up_to: false,
                broadcaster_fee: Some(BroadcasterFeeOutput {
                    recipient: broadcaster.address_data,
                    token_address: request.fee_token,
                    amount: fee_amount,
                }),
                min_gas_price: bound_min_gas_price,
            };
            DesktopUnshieldPreparedPlan::Single(
                tx_builder
                    .build_unshield_plan_with_signer(
                        &request.view_session.scan_keys(),
                        &signer,
                        &forest,
                        &utxos,
                        unshield_request,
                        &prover,
                    )
                    .await
                    .map_err(|error| {
                        public_broadcaster_build_error(
                            error,
                            fee_amount,
                            split.fee_mode,
                            same_token_fee,
                            RAILGUN_PROTOCOL_FEE_BPS,
                        )
                    })
                    .wrap_err("build public broadcaster unshield proof")?,
            )
        };
        tracing::info!(
            attempt,
            fee_amount = %fee_amount,
            elapsed_ms = proof_started.elapsed().as_millis(),
            transaction_count = plan.transaction_count(),
            input_count = plan.input_count(),
            private_output_count = plan.private_output_count(),
            public_output_count = plan.public_output_count(),
            native_top_up = native_top_up.is_some(),
            broadcaster = %broadcaster.railgun_address,
            fees_id = %broadcaster.fees_id,
            "built public broadcaster unshield proof"
        );
        update_transaction_generation_stage(
            request.progress_tx.as_ref(),
            TransactionGenerationStage::EstimatingBroadcasterFee,
        );
        let gas_started = Instant::now();
        let call_to = plan.call_to();
        let call_data = plan.call_data();
        let (gas_limit, computed_fee) = estimate_public_broadcaster_fee_from_rpc_pool(
            &query_rpc_pool,
            request.chain_id,
            call_to,
            &call_data,
            broadcaster.fee,
            min_gas_price,
            chain.gas.gas_limit_buffer,
        )
        .await?;
        let gas_elapsed_ms = gas_started.elapsed().as_millis();
        tracing::info!(
            attempt,
            available_fee = %fee_amount,
            computed_fee = %computed_fee,
            gas_limit,
            min_gas_price,
            bound_min_gas_price,
            gas_elapsed_ms,
            broadcaster = %broadcaster.railgun_address,
            fees_id = %broadcaster.fees_id,
            "estimated public broadcaster unshield fee"
        );
        if broadcaster_fee_covers(fee_amount, computed_fee) {
            let reported_amounts = public_broadcaster_reported_amounts(
                request.token,
                request.fee_token,
                split,
                RAILGUN_PROTOCOL_FEE_BPS,
                native_top_up.as_ref(),
            );
            tracing::info!(
                attempt,
                fee_amount = %fee_amount,
                computed_fee = %computed_fee,
                gas_limit,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "public broadcaster unshield fee stabilized"
            );
            update_transaction_generation_stage(
                request.progress_tx.as_ref(),
                TransactionGenerationStage::GeneratingPoiProofs,
            );
            let pre_transaction_pois = public_broadcaster_pre_transaction_pois(
                plan.chunks(),
                &broadcaster,
                request.session.as_ref(),
                request.chain_id,
                &prover,
                request.verify_proof,
                http,
            )
            .await?;
            let pending_persist_started = Instant::now();
            let pending_contexts = match &plan {
                DesktopUnshieldPreparedPlan::Single(plan) => {
                    persist_pending_unshield_output_poi_contexts(
                        request.session.as_ref(),
                        &plan.chunks,
                        &pre_transaction_pois.pending_pois,
                        &pre_transaction_pois.pending_poi_list_keys,
                        true,
                        !same_token_fee,
                    )
                    .await?
                }
                DesktopUnshieldPreparedPlan::Composite(plan) => {
                    persist_pending_composite_unshield_output_poi_contexts(
                        request.session.as_ref(),
                        &plan.chunks,
                        &plan.private_output_roles,
                        &pre_transaction_pois.pending_pois,
                        &pre_transaction_pois.pending_poi_list_keys,
                    )
                    .await?
                }
            };
            tracing::info!(
                chain_id = request.chain_id,
                pending_contexts,
                elapsed_ms = pending_persist_started.elapsed().as_millis(),
                "persisted public broadcaster unshield pending output POI contexts"
            );
            let relay_call_count = match &plan {
                DesktopUnshieldPreparedPlan::Single(_) => usize::from(request.unwrap),
                DesktopUnshieldPreparedPlan::Composite(plan) => plan.shape.relay_call_count,
            };
            let uses_relay_adapt = match &plan {
                DesktopUnshieldPreparedPlan::Single(_) => request.unwrap,
                DesktopUnshieldPreparedPlan::Composite(plan) => plan.shape.uses_relay_adapt,
            };
            return Ok(PreparedPublicBroadcasterPlan {
                transaction_count: plan.transaction_count(),
                input_count: plan.input_count(),
                private_output_count: plan.private_output_count(),
                public_output_count: plan.public_output_count(),
                relay_call_count,
                uses_relay_adapt,
                plan,
                pre_transaction_pois_per_txid_leaf_per_list: pre_transaction_pois.request_pois,
                broadcaster,
                action_token: request.token,
                fee_token: request.fee_token,
                entered_amount: split.entered_amount,
                receiver_amount: split.receiver_amount,
                recipient_amount: reported_amounts.recipient_amount,
                total_private_spend: reported_amounts.total_private_spend,
                fee_amount,
                protocol_fee_amount: reported_amounts.protocol_fee_amount,
                protocol_fee_bps: RAILGUN_PROTOCOL_FEE_BPS,
                fee_mode: split.fee_mode,
                gas_limit,
                min_gas_price,
                bound_min_gas_price,
                native_top_up,
            });
        }
        let next_fee = buffered_public_broadcaster_fee(computed_fee);
        log_public_broadcaster_fee_prediction_failure(
            "unshield",
            attempt,
            fee_amount,
            computed_fee,
            gas_limit,
            initial_fee_estimate.as_ref(),
            plan.transaction_count(),
            plan.input_count(),
            plan.private_output_count(),
            plan.public_output_count(),
            &broadcaster,
        );
        tracing::info!(
            attempt,
            previous_fee = %fee_amount,
            computed_fee = %computed_fee,
            next_fee = %next_fee,
            "retrying public broadcaster unshield proof with buffered fee"
        );
        fee_amount = next_fee;
    }

    Err(eyre!(
        "public broadcaster fee did not stabilize after bounded retries"
    ))
}

pub(super) async fn prepare_desktop_send_public_broadcaster(
    request: DesktopSendPublicBroadcasterRequest,
    http: &HttpContext,
) -> Result<PreparedPublicBroadcasterPlan<SendPlan>> {
    if request.session.chain_id != request.chain_id {
        return Err(eyre!(
            "selected wallet session is for chain {}, not {}",
            request.session.chain_id,
            request.chain_id
        ));
    }

    let recipient = parse_railgun_recipient(&request.recipient)?;
    let PublicBroadcasterSetup {
        chain,
        broadcaster,
        query_rpc_pool,
        min_gas_price,
        prover,
        forest,
        utxos,
    } = public_broadcaster_setup(
        &request.session,
        request.chain_id,
        request.effective_chain.as_ref(),
        request.fee_token,
        &request.fee_rows,
        &request.selection,
        false,
        request.fee_policy,
        &request.trust_filter,
        request.anchor_cache.as_ref(),
        http,
    )
    .await?;
    let bound_min_gas_price =
        public_broadcaster_bound_min_gas_price(request.chain_id, min_gas_price);
    let same_token_fee = request.fee_token == request.token;
    update_transaction_generation_stage(
        request.progress_tx.as_ref(),
        TransactionGenerationStage::SelectingPrivateNotes,
    );
    let seeded_fee_amount =
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
    let initial_fee_estimate = match approximate_public_broadcaster_cost(
        broadcaster.clone(),
        request.token,
        request.fee_token,
        request.amount,
        request.fee_mode,
        U256::ZERO,
        min_gas_price,
        seeded_fee_amount,
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
    ) {
        Ok(estimate) => {
            tracing::info!(
                fee_amount = %estimate.fee_amount,
                gas_limit = estimate.gas_limit,
                min_gas_price,
                bound_min_gas_price,
                transaction_count = estimate.transaction_count,
                input_count = estimate.input_count,
                private_output_count = estimate.private_output_count,
                public_output_count = estimate.public_output_count,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "using approximate public broadcaster send fee for first proof"
            );
            Some(estimate)
        }
        Err(err) => {
            if !same_token_fee {
                return Err(err).wrap_err("estimate initial public broadcaster send fee");
            }
            tracing::warn!(
                ?err,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "failed to estimate initial same-token public broadcaster send fee; starting at zero"
            );
            None
        }
    };
    let initial_fee_amount = initial_fee_estimate
        .as_ref()
        .map_or(U256::ZERO, |estimate| estimate.fee_amount);

    let signer = request.spend_authorization.into_signer(
        request.vault_store.as_ref(),
        request.view_session.wallet_id(),
        "public broadcaster send",
    )?;

    let tx_builder = TransactionBuilder {
        chain_type: 0,
        chain_id: request.chain_id,
        railgun_contract: chain.railgun_contract,
        relay_adapt_contract: chain.relay_adapt_contract,
    };

    let mut fee_amount = initial_fee_amount;
    for attempt in 1..=PUBLIC_BROADCASTER_FEE_ATTEMPTS {
        let split = public_broadcaster_amount_split_for_tokens(
            request.amount,
            fee_amount,
            request.fee_mode,
            same_token_fee,
        )?;
        let send_request = RailgunSendRequest {
            token_address: request.token,
            amount: split.receiver_amount,
            recipient,
            verify_proof: request.verify_proof,
            spend_up_to: false,
            broadcaster_fee: Some(BroadcasterFeeOutput {
                recipient: broadcaster.address_data,
                token_address: request.fee_token,
                amount: fee_amount,
            }),
            min_gas_price: bound_min_gas_price,
        };
        update_transaction_generation_stage(
            request.progress_tx.as_ref(),
            TransactionGenerationStage::ProvingTransaction,
        );
        let proof_started = Instant::now();
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
            .map_err(|error| {
                public_broadcaster_build_error(
                    error,
                    fee_amount,
                    split.fee_mode,
                    same_token_fee,
                    U256::ZERO,
                )
            })
            .wrap_err("build public broadcaster send proof")?;
        let chunk_input_counts = plan
            .chunks
            .iter()
            .map(|chunk| chunk.inputs.len())
            .collect::<Vec<_>>();
        let chunk_output_counts = plan
            .chunks
            .iter()
            .map(|chunk| chunk.outputs.len())
            .collect::<Vec<_>>();
        let chunk_tree_numbers = plan
            .chunks
            .iter()
            .map(|chunk| chunk.tree_number)
            .collect::<Vec<_>>();
        tracing::info!(
            attempt,
            fee_amount = %fee_amount,
            elapsed_ms = proof_started.elapsed().as_millis(),
            transaction_count = plan.transaction_count(),
            input_count = plan.input_count(),
            private_output_count = plan.private_output_count(),
            public_output_count = plan.public_output_count(),
            same_token_fee,
            ?chunk_input_counts,
            ?chunk_output_counts,
            ?chunk_tree_numbers,
            broadcaster = %broadcaster.railgun_address,
            fees_id = %broadcaster.fees_id,
            "built public broadcaster send proof"
        );
        update_transaction_generation_stage(
            request.progress_tx.as_ref(),
            TransactionGenerationStage::EstimatingBroadcasterFee,
        );
        let gas_started = Instant::now();
        let (gas_limit, computed_fee) = estimate_public_broadcaster_fee_from_rpc_pool(
            &query_rpc_pool,
            request.chain_id,
            plan.call.to,
            &plan.call.data,
            broadcaster.fee,
            min_gas_price,
            chain.gas.gas_limit_buffer,
        )
        .await?;
        let gas_elapsed_ms = gas_started.elapsed().as_millis();
        tracing::info!(
            attempt,
            available_fee = %fee_amount,
            computed_fee = %computed_fee,
            gas_limit,
            min_gas_price,
            bound_min_gas_price,
            gas_elapsed_ms,
            broadcaster = %broadcaster.railgun_address,
            fees_id = %broadcaster.fees_id,
            "estimated public broadcaster send fee"
        );
        if broadcaster_fee_covers(fee_amount, computed_fee) {
            let protocol_fee_amount = U256::ZERO;
            tracing::info!(
                attempt,
                fee_amount = %fee_amount,
                computed_fee = %computed_fee,
                gas_limit,
                broadcaster = %broadcaster.railgun_address,
                fees_id = %broadcaster.fees_id,
                "public broadcaster send fee stabilized"
            );
            update_transaction_generation_stage(
                request.progress_tx.as_ref(),
                TransactionGenerationStage::GeneratingPoiProofs,
            );
            let pre_transaction_pois = public_broadcaster_pre_transaction_pois(
                &plan.chunks,
                &broadcaster,
                request.session.as_ref(),
                request.chain_id,
                &prover,
                request.verify_proof,
                http,
            )
            .await?;
            let pending_persist_started = Instant::now();
            let pending_contexts = persist_pending_send_output_poi_contexts(
                request.session.as_ref(),
                &plan.chunks,
                &pre_transaction_pois.pending_pois,
                &pre_transaction_pois.pending_poi_list_keys,
                true,
                !same_token_fee,
            )
            .await?;
            tracing::info!(
                chain_id = request.chain_id,
                pending_contexts,
                elapsed_ms = pending_persist_started.elapsed().as_millis(),
                "persisted public broadcaster send pending output POI contexts"
            );
            return Ok(PreparedPublicBroadcasterPlan {
                transaction_count: plan.transaction_count(),
                input_count: plan.input_count(),
                private_output_count: plan.private_output_count(),
                public_output_count: plan.public_output_count(),
                relay_call_count: 0,
                uses_relay_adapt: false,
                plan,
                pre_transaction_pois_per_txid_leaf_per_list: pre_transaction_pois.request_pois,
                broadcaster,
                action_token: request.token,
                fee_token: request.fee_token,
                entered_amount: split.entered_amount,
                receiver_amount: split.receiver_amount,
                recipient_amount: recipient_amount_after_protocol_fee(
                    split.receiver_amount,
                    protocol_fee_amount,
                ),
                total_private_spend: split.total_private_spend,
                fee_amount,
                protocol_fee_amount,
                protocol_fee_bps: U256::ZERO,
                fee_mode: split.fee_mode,
                gas_limit,
                min_gas_price,
                bound_min_gas_price,
                native_top_up: None,
            });
        }
        let next_fee = buffered_public_broadcaster_fee(computed_fee);
        log_public_broadcaster_fee_prediction_failure(
            "send",
            attempt,
            fee_amount,
            computed_fee,
            gas_limit,
            initial_fee_estimate.as_ref(),
            plan.transaction_count(),
            plan.input_count(),
            plan.private_output_count(),
            plan.public_output_count(),
            &broadcaster,
        );
        tracing::info!(
            attempt,
            previous_fee = %fee_amount,
            computed_fee = %computed_fee,
            next_fee = %next_fee,
            "retrying public broadcaster send proof with buffered fee"
        );
        fee_amount = next_fee;
    }

    Err(eyre!(
        "public broadcaster fee did not stabilize after bounded retries"
    ))
}

pub(super) async fn estimate_public_broadcaster_fee(
    provider: &(impl Provider + Clone),
    chain_id: u64,
    to: Address,
    data: &Bytes,
    token_fee_per_unit_gas: U256,
    min_gas_price: u128,
    gas_limit_buffer: u64,
) -> Result<(u64, U256)> {
    let tx_req = TransactionRequest::default()
        .with_chain_id(chain_id)
        .with_to(to)
        .with_input(data.clone())
        .with_gas_price(min_gas_price);
    let estimated_gas = provider
        .estimate_gas(tx_req)
        .await
        .wrap_err("estimate public broadcaster gas")?;
    let gas_limit = public_broadcaster_gas_limit_with_buffer(estimated_gas, gas_limit_buffer);
    let service_gas_price = public_broadcaster_service_gas_price(min_gas_price);
    Ok((
        gas_limit,
        broadcaster_fee_amount(token_fee_per_unit_gas, gas_limit, service_gas_price),
    ))
}

pub(super) async fn estimate_public_broadcaster_fee_from_rpc_pool(
    query_rpc_pool: &QueryRpcPool,
    chain_id: u64,
    to: Address,
    data: &Bytes,
    token_fee_per_unit_gas: U256,
    min_gas_price: u128,
    gas_limit_buffer: u64,
) -> Result<(u64, U256)> {
    let mut last_error = None;
    for _ in 0..query_rpc_pool.len() {
        let Some(provider_handle) = query_rpc_pool.random_provider() else {
            break;
        };
        match estimate_public_broadcaster_fee(
            &provider_handle.provider,
            chain_id,
            to,
            data,
            token_fee_per_unit_gas,
            min_gas_price,
            gas_limit_buffer,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(error) => {
                let rpc = http::redact_url_for_display(&provider_handle.url);
                tracing::warn!(%error, %rpc, "estimate public broadcaster gas failed");
                query_rpc_pool.mark_bad_provider(&provider_handle);
                last_error = Some(error);
            }
        }
    }
    if let Some(error) = last_error {
        Err(error).wrap_err("all query RPC public broadcaster gas estimate attempts failed")
    } else {
        Err(eyre!("no healthy query RPC available"))
    }
}

pub(crate) const fn public_broadcaster_gas_limit_with_buffer(
    estimated_gas: u64,
    gas_limit_buffer: u64,
) -> u64 {
    estimated_gas.saturating_add(gas_limit_buffer)
}

pub(crate) fn public_broadcaster_transact_params(
    broadcaster: &PublicBroadcasterCandidate,
    to: Address,
    data: Bytes,
    min_gas_price: u128,
    pre_transaction_pois_per_txid_leaf_per_list: PreTransactionPoiMap,
) -> BroadcasterRawParamsTransact {
    BroadcasterRawParamsTransact {
        chain_type: 0,
        chain_id: broadcaster.chain_id,
        transact_type: None,
        min_gas_price: Some(U256::from(min_gas_price)),
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        authorization: None,
        fees_id: Some(broadcaster.fees_id.clone()),
        to,
        data,
        broadcaster_viewing_key: FixedBytes::from(broadcaster.viewing_public_key),
        txid_version: Some(DEFAULT_TXID_VERSION.to_string()),
        pre_transaction_pois_per_txid_leaf_per_list,
    }
}

pub(super) async fn publish_public_broadcaster_payload(
    waku: &WakuClient,
    pubsub_path: &str,
    transact_topic: &str,
    payload: &[u8],
    attempt: usize,
) -> Result<()> {
    tracing::info!(
        pubsub_path = %pubsub_path,
        transact_topic = %transact_topic,
        payload_len = payload.len(),
        attempt,
        "publishing public broadcaster transact request"
    );
    let publish_started = Instant::now();
    waku.publish(transact_topic, payload)
        .await
        .wrap_err("publish public broadcaster transact request")?;
    tracing::info!(
        pubsub_path = %pubsub_path,
        transact_topic = %transact_topic,
        elapsed_ms = publish_started.elapsed().as_millis(),
        attempt,
        "published public broadcaster transact request"
    );
    Ok(())
}

pub(crate) async fn public_broadcaster_republish_loop<F, Fut>(
    mut stop_rx: oneshot::Receiver<()>,
    republish_interval: Duration,
    mut publish: F,
) where
    F: FnMut(usize) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send,
{
    let mut attempt = 1usize;
    loop {
        tokio::select! {
            _ = &mut stop_rx => break,
            () = tokio::time::sleep(republish_interval) => {
                attempt = attempt.saturating_add(1);
                if let Err(error) = publish(attempt).await {
                    tracing::warn!(%error, attempt, "republish public broadcaster transact request failed");
                }
            }
        }
    }
}

pub(super) async fn submit_public_broadcaster_plan(
    waku: Arc<WakuClient>,
    to: Address,
    data: Bytes,
    pre_transaction_pois_per_txid_leaf_per_list: PreTransactionPoiMap,
    broadcaster: PublicBroadcasterCandidate,
    action_token: Address,
    fee_token: Address,
    entered_amount: U256,
    receiver_amount: U256,
    recipient_amount: U256,
    total_private_spend: U256,
    fee_amount: U256,
    protocol_fee_amount: U256,
    protocol_fee_bps: U256,
    fee_mode: FeeHandlingMode,
    gas_limit: u64,
    min_gas_price: u128,
    bound_min_gas_price: u128,
    transaction_count: usize,
    input_count: usize,
    private_output_count: usize,
    public_output_count: usize,
    relay_call_count: usize,
    uses_relay_adapt: bool,
    native_top_up: Option<DesktopNativeTopUpPlan>,
    progress_tx: Option<TransactionGenerationProgressSender>,
    timeout: Duration,
    republish_interval: Duration,
) -> Result<PublicBroadcasterSubmissionResult> {
    let transact_topic = transact_topic(broadcaster.chain_id);
    let response_topic = transact_response_topic(broadcaster.chain_id);
    tracing::info!(
        chain_id = broadcaster.chain_id,
        broadcaster = %broadcaster.railgun_address,
        broadcaster_identifier = ?broadcaster.identifier.as_deref(),
        fees_id = %broadcaster.fees_id,
        token = ?broadcaster.token,
        to = ?to,
        fee_amount = %fee_amount,
        gas_limit,
        min_gas_price,
        bound_min_gas_price,
        data_len = data.len(),
        transact_topic = %transact_topic,
        response_topic = %response_topic,
        "preparing public broadcaster transact request"
    );
    update_transaction_generation_stage(
        progress_tx.as_ref(),
        TransactionGenerationStage::PublishingToBroadcaster,
    );
    let params = public_broadcaster_transact_params(
        &broadcaster,
        to,
        data,
        bound_min_gas_price,
        pre_transaction_pois_per_txid_leaf_per_list,
    );
    let encrypt_started = Instant::now();
    let encrypted = EncryptedTransactRequest::encrypt(broadcaster.viewing_public_key, &params)
        .wrap_err("encrypt public broadcaster transact request")?;
    let payload = encrypted
        .to_transact_payload()
        .wrap_err("serialize public broadcaster transact request")?;
    tracing::info!(
        chain_id = broadcaster.chain_id,
        broadcaster = %broadcaster.railgun_address,
        fees_id = %broadcaster.fees_id,
        payload_len = payload.len(),
        elapsed_ms = encrypt_started.elapsed().as_millis(),
        "built public broadcaster encrypted Waku payload"
    );
    let pubsub_path = waku.pubsub_path().to_string();
    tracing::info!(
        pubsub_path = %pubsub_path,
        response_topic = %response_topic,
        "subscribing to public broadcaster response topic"
    );
    let subscribe_started = Instant::now();
    let mut response_rx = waku
        .subscribe(vec![response_topic.clone()])
        .await
        .wrap_err("subscribe to public broadcaster response topic")?;
    tracing::info!(
        response_topic = %response_topic,
        elapsed_ms = subscribe_started.elapsed().as_millis(),
        "subscribed to public broadcaster response topic"
    );
    publish_public_broadcaster_payload(&waku, &pubsub_path, &transact_topic, &payload, 1)
        .await
        .wrap_err("publish initial public broadcaster transact request")?;
    update_transaction_generation_stage(
        progress_tx.as_ref(),
        TransactionGenerationStage::WaitingForBroadcasterResponse,
    );

    let (republish_stop_tx, republish_stop_rx) = oneshot::channel();
    let republish_waku = Arc::clone(&waku);
    let republish_pubsub_path = pubsub_path.clone();
    let republish_transact_topic = transact_topic.clone();
    let republish_payload = payload.clone();
    let republish_handle = tokio::spawn(public_broadcaster_republish_loop(
        republish_stop_rx,
        republish_interval,
        move |attempt| {
            let waku = Arc::clone(&republish_waku);
            let pubsub_path = republish_pubsub_path.clone();
            let transact_topic = republish_transact_topic.clone();
            let payload = republish_payload.clone();
            async move {
                publish_public_broadcaster_payload(
                    &waku,
                    &pubsub_path,
                    &transact_topic,
                    &payload,
                    attempt,
                )
                .await
            }
        },
    ));

    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    let result = loop {
        tokio::select! {
            () = &mut sleep => {
                tracing::warn!(
                    chain_id = broadcaster.chain_id,
                    broadcaster = %broadcaster.railgun_address,
                    fees_id = %broadcaster.fees_id,
                    response_topic = %response_topic,
                    timeout_ms = timeout.as_millis(),
                    "timed out waiting for public broadcaster response"
                );
                break PublicBroadcasterResultKind::TimedOut;
            },
            msg = response_rx.recv() => {
                let Some(msg) = msg else {
                    tracing::warn!(response_topic = %response_topic, "public broadcaster response channel closed");
                    break PublicBroadcasterResultKind::TimedOut;
                };
                tracing::info!(
                    content_topic = %msg.content_topic,
                    payload_len = msg.payload.len(),
                    "received public broadcaster response candidate"
                );
                match decode_public_broadcaster_response(&encrypted.shared_key, &msg.payload) {
                    Ok(Some(result)) => {
                        tracing::info!(?result, "decrypted public broadcaster response");
                        break result;
                    }
                    Ok(None) => tracing::debug!("public broadcaster response was not decryptable with request key"),
                    Err(error) => tracing::debug!(%error, "ignoring undecryptable public broadcaster response"),
                }
            }
        }
    };
    let _ = republish_stop_tx.send(());
    republish_handle.abort();

    Ok(PublicBroadcasterSubmissionResult {
        broadcaster,
        action_token,
        fee_token,
        entered_amount,
        receiver_amount,
        recipient_amount,
        total_private_spend,
        fee_amount,
        protocol_fee_amount,
        protocol_fee_bps,
        fee_mode,
        gas_limit,
        min_gas_price,
        transaction_count,
        input_count,
        private_output_count,
        public_output_count,
        relay_call_count,
        uses_relay_adapt,
        result,
        native_top_up,
    })
}
