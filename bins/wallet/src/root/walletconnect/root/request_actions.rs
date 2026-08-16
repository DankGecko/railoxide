use super::super::helpers::{
    WALLETCONNECT_RAW_REQUEST_LABEL, WALLETCONNECT_TRANSACTION_DETAILS_LABEL,
    WalletConnectRequestExpiryStatus,
};
use super::super::intent::{
    WalletConnectAmount, WalletConnectHeroSummary, WalletConnectIntentContext,
    WalletConnectIntentView, WalletConnectParty, WalletConnectPartyRole, WalletConnectRisk,
    build_walletconnect_intent, walletconnect_approximate_usd_label,
    walletconnect_party_address_label, walletconnect_party_badge_label,
    walletconnect_selected_account_provenance_visible, walletconnect_should_render_token_contract,
    walletconnect_token_contract_recognition,
};
use super::*;
use alloy::primitives::Address;

impl WalletRoot {
    pub(in crate::root::walletconnect) fn render_walletconnect_request(
        &self,
        root: &Entity<Self>,
        request: &WalletConnectRequestUi,
        content_width: Pixels,
    ) -> gpui::Div {
        let approve_root = root.clone();
        let reject_root = root.clone();
        let request_key = Arc::<str>::from(request.key.as_str());
        let reject_key = Arc::clone(&request_key);
        let intent_context = match self.walletconnect_intent_context(request) {
            Ok(context) => context,
            Err(message) => {
                return walletconnect_notice(message, theme::DANGER, theme::DANGER_BG);
            }
        };
        let intent = build_walletconnect_intent(request, intent_context);
        let disclosure_state = self
            .walletconnect
            .request_disclosure_state(request.key.as_str());
        let hardware_request = request.account_source == PublicAccountSource::HardwareDerived;
        let in_flight = self
            .walletconnect
            .request_actions
            .contains(request.key.as_str());
        let hardware_typed_data_hash_fallback =
            walletconnect_request_uses_hardware_typed_data_hash_fallback(
                request,
                self.walletconnect_request_hardware_typed_data_mode(request),
            );
        let unlimited_allowance = intent
            .risks
            .iter()
            .any(|risk| matches!(risk, &WalletConnectRisk::UnlimitedAllowance { .. }));
        let approve_label = walletconnect_request_approve_label(
            in_flight,
            hardware_request,
            hardware_typed_data_hash_fallback,
            unlimited_allowance,
        );
        let mut content = div().w_full().min_w(px(0.0)).flex().flex_col().gap_2();
        for risk in &intent.risks {
            content = content.child(render_walletconnect_intent_risk(risk));
        }
        content = content
            .child(render_walletconnect_intent_card(
                &request.key,
                &intent,
                &request.item.chain_id,
                content_width,
            ))
            .child(render_walletconnect_request_provenance(request, &intent))
            .when_some(intent.transaction.as_ref(), |this, transaction| {
                this.child(render_walletconnect_request_disclosure(
                    root,
                    &request.key,
                    WalletConnectRequestDisclosure::TransactionDetails,
                    WALLETCONNECT_TRANSACTION_DETAILS_LABEL,
                    disclosure_state.transaction_details_open,
                    render_walletconnect_transaction_details(
                        transaction,
                        &request.key,
                        content_width,
                    ),
                ))
            })
            .child(render_walletconnect_request_disclosure(
                root,
                &request.key,
                WalletConnectRequestDisclosure::RawRequest,
                WALLETCONNECT_RAW_REQUEST_LABEL,
                disclosure_state.raw_request_open,
                walletconnect_raw_details(intent.raw_request),
            ));
        if hardware_typed_data_hash_fallback {
            content = content.child(walletconnect_notice(
                "This hardware session will use the device's EIP-712 hash-signing fallback. RailOxide computed the typed-data hashes from the validated request and will verify the signature before responding, but the device may show hashes instead of structured fields. Continue only if you accept this reduced device visibility.",
                theme::WARNING,
                theme::WARNING_BG,
            ));
        }
        if matches!(request.account_source, PublicAccountSource::HardwareDerived) {
            content = content.child(walletconnect_notice(
                hardware_walletconnect_notice(request.item.method),
                theme::WARNING,
                theme::WARNING_BG,
            ));
            #[cfg(feature = "hardware")]
            {
                if self.current_session_needs_trezor_app_passphrase() {
                    content = content.child(walletconnect_trezor_app_passphrase_input(
                        &self.trezor_app_passphrase_input,
                        in_flight,
                    ));
                }
            }
        }
        if let Some(progress) = self
            .walletconnect
            .request_approval_progress
            .get(request.key.as_str())
        {
            content = content.child(render_walletconnect_approval_stepper(progress));
        }
        if let Some(error) = self.walletconnect.error.as_ref() {
            content = content.child(
                Alert::error("walletconnect-request-dialog-error", error.to_string()).small(),
            );
        }
        let locally_expired = !walletconnect_request_approval_admitted(
            request.item.expiry_timestamp,
            current_unix_seconds(),
        );
        content.child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .child(
                    app_button(
                        SharedString::from(format!("walletconnect-request-reject-{}", request.key)),
                        "Reject",
                    )
                    .outline()
                    .small()
                    .disabled(in_flight)
                    .on_click(move |_event, window, cx| {
                        let key = Arc::clone(&reject_key);
                        reject_root.update(cx, |root, cx| {
                            root.reject_walletconnect_request(key.as_ref(), window, cx);
                        });
                    }),
                )
                .child({
                    let approve_button = app_button(
                        SharedString::from(format!(
                            "walletconnect-request-approve-{}",
                            request.key
                        )),
                        approve_label,
                    )
                    .primary()
                    .small()
                    .loading(in_flight)
                    .disabled(in_flight || locally_expired);
                    let approve_button = if unlimited_allowance
                        && !in_flight
                        && !hardware_request
                        && !hardware_typed_data_hash_fallback
                    {
                        approve_button.danger()
                    } else {
                        approve_button
                    };
                    approve_button.on_click(move |_event, window, cx| {
                        let key = Arc::clone(&request_key);
                        approve_root.update(cx, |root, cx| {
                            root.approve_walletconnect_request(key.as_ref(), window, cx);
                        });
                    })
                }),
        )
    }

    fn walletconnect_request_hardware_typed_data_mode(
        &self,
        request: &WalletConnectRequestUi,
    ) -> HardwareTypedDataSigningMode {
        walletconnect_hardware_typed_data_mode_for_request(
            request,
            &self.public_accounts,
            self.view_session.as_deref(),
        )
    }

    fn walletconnect_intent_context(
        &self,
        request: &WalletConnectRequestUi,
    ) -> Result<WalletConnectIntentContext<'_>, &'static str> {
        let chain_id = parse_caip2_chain_id(&request.item.chain_id)
            .ok_or("This request does not identify a supported EVM chain.")?;
        let chain = self
            .effective_chain_configs
            .get(&chain_id)
            .ok_or("The request chain is not available in the current wallet settings.")?;
        Ok(WalletConnectIntentContext {
            chain,
            token_registry: &self.effective_token_registry,
            anchor_rates: &self.public_broadcaster_anchor_cache,
            public_accounts: &self.public_accounts,
            public_address_book: &self.public_address_book,
        })
    }

    pub(in crate::root::walletconnect) fn approve_walletconnect_request(
        &mut self,
        request_key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.walletconnect.request_actions.contains(request_key) {
            return;
        }
        let Some(request) = self
            .walletconnect
            .pending_requests
            .get(request_key)
            .cloned()
        else {
            return;
        };
        if !walletconnect_request_approval_admitted(
            request.item.expiry_timestamp,
            current_unix_seconds(),
        ) {
            self.walletconnect.error = Some(Arc::from(
                "WalletConnect request expired before approval could start.",
            ));
            cx.notify();
            return;
        }
        tracing::info!(
            target: "wallet::root::walletconnect",
            request_key = %walletconnect_request_key_log_label(request_key),
            method = request.item.method.as_str(),
            chain_id = request.item.chain_id.as_str(),
            dapp = request.item.dapp_name.as_str(),
            hardware = request.account_source == PublicAccountSource::HardwareDerived,
            "walletconnect request approval selected"
        );
        if request.account_source == PublicAccountSource::HardwareDerived {
            self.submit_walletconnect_request_authorized(
                request_key,
                request.review_token,
                Zeroizing::new(String::new()),
                None,
                window,
                cx,
            );
        } else {
            let intent = SpendAuthorizationIntent::WalletConnectRequest {
                request_key: request_key.to_owned(),
                review_token: request.review_token,
            };
            let summary = {
                let context = match self.walletconnect_intent_context(&request) {
                    Ok(context) => context,
                    Err(message) => {
                        self.walletconnect.error = Some(Arc::from(message));
                        cx.notify();
                        return;
                    }
                };
                let intent = build_walletconnect_intent(&request, context);
                walletconnect_request_authorization_summary(&request, &intent)
            };
            self.request_spend_authorization(intent, summary, window, cx);
        }
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    pub(in crate::root) fn submit_walletconnect_request_authorized(
        &mut self,
        request_key: &str,
        review_token: u64,
        vault_password: Zeroizing<String>,
        protected_software_seed_session: Option<
            Arc<wallet_ops::vault::ProtectedSoftwareSeedSession>,
        >,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.walletconnect.request_actions.contains(request_key) {
            return;
        }
        let Some(request) = self
            .walletconnect
            .pending_requests
            .get(request_key)
            .cloned()
        else {
            return;
        };
        if !walletconnect_request_matches_review_token(&request, review_token) {
            self.walletconnect.error = Some(Arc::from(
                "WalletConnect request changed while authorization was open; review the current request before approving.",
            ));
            cx.notify();
            return;
        }
        let (Some(vault_store), Some(view_session)) =
            (self.vault_store.clone(), self.view_session.clone())
        else {
            self.walletconnect.error = Some(Arc::from("Unlock a wallet before approving requests"));
            cx.notify();
            return;
        };
        let effective_chain = parse_caip2_chain_id(&request.item.chain_id)
            .and_then(|chain_id| self.walletconnect_effective_chain_config(chain_id));
        let request = match self.revalidate_walletconnect_pending_request(
            &request,
            vault_store.as_ref(),
            view_session.as_ref(),
            current_unix_seconds(),
        ) {
            Ok(request) => request,
            Err(error) => {
                let context =
                    match self.walletconnect_client_context_for_session(&request.session, cx) {
                        Ok(context) => context,
                        Err(context_error) => {
                            self.walletconnect.error = Some(context_error);
                            cx.notify();
                            return;
                        }
                    };
                self.publish_invalid_walletconnect_pending_request(
                    request_key,
                    &request,
                    &error,
                    context,
                    window,
                    cx,
                );
                return;
            }
        };
        let context = match self.walletconnect_client_context_for_session(&request.session, cx) {
            Ok(context) => context,
            Err(error) => {
                self.walletconnect.error = Some(error);
                cx.notify();
                return;
            }
        };
        #[cfg(feature = "hardware")]
        let trezor_app_passphrase =
            if request.account_source == PublicAccountSource::HardwareDerived {
                view_session.hardware_profile_session().and_then(|session| {
                    self.read_trezor_app_passphrase_for_hardware_session(session, window, cx)
                })
            } else {
                None
            };
        #[cfg(not(feature = "hardware"))]
        let trezor_app_passphrase = None;
        #[cfg(feature = "hardware")]
        let trezor_pin_matrix_provider =
            if request.account_source == PublicAccountSource::HardwareDerived {
                Some(self.trezor_pin_matrix_provider_for_operation(window, cx))
            } else {
                None
            };
        #[cfg(not(feature = "hardware"))]
        let trezor_pin_matrix_provider = None;
        let http = self.http.clone();
        let hardware_request = request.account_source == PublicAccountSource::HardwareDerived;
        let hash_fallback_confirmed = walletconnect_request_uses_hardware_typed_data_hash_fallback(
            &request,
            self.walletconnect_request_hardware_typed_data_mode(&request),
        );
        let progress_generation = hardware_request.then(|| {
            self.walletconnect
                .start_request_approval_progress(request_key, &request)
        });
        let approval_event_tx = progress_generation.map(|generation| {
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            Self::spawn_walletconnect_approval_session_event_listener(
                request_key.to_owned(),
                generation,
                event_rx,
                cx,
            );
            event_tx
        });
        self.walletconnect
            .request_actions
            .insert(request_key.to_owned());
        self.walletconnect.error = None;
        tracing::info!(
            target: "wallet::root::walletconnect",
            request_key = %walletconnect_request_key_log_label(request_key),
            method = request.item.method.as_str(),
            chain_id = request.item.chain_id.as_str(),
            dapp = request.item.dapp_name.as_str(),
            "submitting authorized walletconnect request"
        );
        let request_key = request_key.to_owned();
        let join = self.runtime.spawn(async move {
            approve_walletconnect_request_task(
                request,
                vault_store,
                view_session,
                vault_password,
                protected_software_seed_session,
                trezor_app_passphrase,
                trezor_pin_matrix_provider,
                effective_chain,
                context,
                http,
                hash_fallback_confirmed,
                approval_event_tx,
            )
            .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                root.walletconnect.request_actions.remove(&request_key);
                match result {
                    Ok(Ok(outcome)) => {
                        tracing::info!(
                            target: "wallet::root::walletconnect",
                            request_key = %walletconnect_request_key_log_label(&request_key),
                            authorization_failed = outcome.authorization_failed,
                            response_published = outcome.response_published,
                            tx_submitted = outcome.submitted_tx_hash.is_some(),
                            "walletconnect request approval handled"
                        );
                        if outcome.hash_fallback_confirmation_required {
                            #[cfg(feature = "hardware")]
                            if let Some(session) = outcome.refreshed_hardware_session {
                                root.refresh_active_hardware_profile_session(session, cx);
                            }
                            root.walletconnect.request_approval_progress.remove(&request_key);
                            root.walletconnect.status = Some(Arc::from(
                                "Review the EIP-712 hash fallback warning, then continue if you still want to approve on device.",
                            ));
                            cx.notify();
                            return;
                        }
                        let completed_request = root.walletconnect.remove_pending_request(&request_key);
                        let show_completion = completed_request.is_some();
                        if let Some(request) = completed_request {
                            root.walletconnect.completed_request_dialogs.insert(
                                request_key.clone(),
                                WalletConnectCompletedRequestUi::from_outcome(request, &outcome),
                            );
                        }
                        if !show_completion {
                            root.stop_walletconnect_request_dialog_refresh();
                        }
                        if root.walletconnect.request_dialog_key.as_deref()
                            == Some(request_key.as_str())
                        {
                            root.clear_trezor_app_passphrase_input(window, cx);
                            if show_completion {
                                root.walletconnect.request_dialog_open = true;
                            } else {
                                root.clear_walletconnect_request_dialog_state(window, cx);
                                window.close_dialog(cx);
                            }
                        }
                        if let Some(error) = outcome.relay_error {
                            let status = outcome.submitted_tx_hash.as_ref().map_or_else(
                                || "WalletConnect request was handled locally, but the relay response publish failed.".to_owned(),
                                |tx_hash| format!(
                                    "WalletConnect transaction was submitted ({tx_hash}), but the relay response publish failed. The request was removed to avoid rebroadcasting."
                                ),
                            );
                            root.walletconnect.status = Some(Arc::from(status));
                            root.walletconnect.error = Some(Arc::from(error));
                        } else if outcome.authorization_failed {
                            root.clear_spend_authorization(cx);
                            root.walletconnect.status = Some(Arc::from(
                                "WalletConnect request was not authorized; error response published.",
                            ));
                        } else if outcome.expired {
                            root.walletconnect.status = Some(Arc::from(
                                "WalletConnect request expired before approval completed.",
                            ));
                        } else {
                            root.walletconnect.status = Some(Arc::from(
                                "WalletConnect request approved and response published.",
                            ));
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(
                            target: "wallet::root::walletconnect",
                            request_key = %walletconnect_request_key_log_label(&request_key),
                            error = %error,
                            "walletconnect request approval failed"
                        );
                        if let Some(generation) = progress_generation {
                            root.walletconnect.fail_request_approval_progress(
                                &request_key,
                                generation,
                                error.clone(),
                            );
                        }
                        root.walletconnect.error = Some(Arc::from(error));
                    }
                    Err(error) => {
                        let message = format!("WalletConnect approval task failed: {error}");
                        if let Some(generation) = progress_generation {
                            root.walletconnect.fail_request_approval_progress(
                                &request_key,
                                generation,
                                message.clone(),
                            );
                        }
                        root.walletconnect.error = Some(Arc::from(message));
                    }
                }
                root.sync_walletconnect_attention();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::root::walletconnect) fn spawn_walletconnect_approval_session_event_listener(
        request_key: String,
        generation: u64,
        mut event_rx: mpsc::UnboundedReceiver<PublicActionSessionEvent>,
        cx: &Context<'_, Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                let _ = this.update(cx, |root, cx| {
                    root.apply_walletconnect_approval_session_event(
                        &request_key,
                        generation,
                        event,
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    pub(in crate::root::walletconnect) fn apply_walletconnect_approval_session_event(
        &mut self,
        request_key: &str,
        generation: u64,
        event: PublicActionSessionEvent,
        cx: &mut Context<'_, Self>,
    ) {
        match event {
            PublicActionSessionEvent::AttemptHandoff { .. } => {
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::PrepareRequest,
                    PublicActionStepStatus::Done,
                    None,
                    None,
                );
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::ApproveOnDevice,
                    PublicActionStepStatus::Pending,
                    None,
                    None,
                );
            }
            PublicActionSessionEvent::AttemptSubmitted { attempt, .. } => {
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::ApproveOnDevice,
                    PublicActionStepStatus::Done,
                    None,
                    None,
                );
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::BroadcastTransaction,
                    PublicActionStepStatus::Done,
                    Some(attempt.tx_hash),
                    None,
                );
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::RespondToDapp,
                    PublicActionStepStatus::Pending,
                    None,
                    None,
                );
            }
            PublicActionSessionEvent::StepFailed { message, .. }
            | PublicActionSessionEvent::AttemptRejected { message, .. }
            | PublicActionSessionEvent::HardwareApprovalFailed { message } => {
                self.discard_active_trezor_session_if_stale(&message, cx);
                self.walletconnect
                    .fail_request_approval_progress(request_key, generation, message);
            }
            PublicActionSessionEvent::HardwareApprovalStarted => {
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::ApproveOnDevice,
                    PublicActionStepStatus::Pending,
                    None,
                    None,
                );
            }
            PublicActionSessionEvent::HardwareApprovalCompleted => {
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::ApproveOnDevice,
                    PublicActionStepStatus::Done,
                    None,
                    None,
                );
                self.walletconnect.apply_request_approval_progress_update(
                    request_key,
                    generation,
                    WalletConnectApprovalProgressStep::RespondToDapp,
                    PublicActionStepStatus::Pending,
                    None,
                    None,
                );
            }
            PublicActionSessionEvent::HardwareProfileSessionRefreshed { session } => {
                #[cfg(feature = "hardware")]
                self.refresh_active_hardware_profile_session(session, cx);
                #[cfg(not(feature = "hardware"))]
                let _ = session;
            }
            PublicActionSessionEvent::FeeAuthorizationRequired { .. } => {}
        }
        cx.notify();
    }

    pub(in crate::root::walletconnect) fn revalidate_walletconnect_pending_request(
        &self,
        request: &WalletConnectRequestUi,
        store: &DesktopVaultStore,
        view_session: &DesktopViewSession,
        now: u64,
    ) -> Result<WalletConnectRequestUi, WalletConnectSessionRequestFailure> {
        walletconnect_validate_pending_request_expiry(request.item.expiry_timestamp, now)?;
        let session = store
            .load_walletconnect_session(view_session, &request.session.session_uuid)
            .map_err(|error| WalletConnectSessionRequestFailure {
                kind: WalletConnectRequestErrorKind::Internal,
                message: format!("Could not reload WalletConnect session: {error}"),
            })?;
        let resolution = store
            .resolve_walletconnect_session_account(view_session, &session)
            .map_err(|error| WalletConnectSessionRequestFailure {
                kind: WalletConnectRequestErrorKind::Internal,
                message: format!("Could not resolve WalletConnect Public account: {error}"),
            })?;
        let account_source = match &resolution {
            WalletConnectSessionAccountResolution::Usable(account) => account.source,
            WalletConnectSessionAccountResolution::TemporarilyPausedWrongPrivateWallet {
                ..
            } => {
                return Err(WalletConnectSessionRequestFailure {
                    kind: WalletConnectRequestErrorKind::Unauthorized,
                    message:
                        "WalletConnect session is paused for a different selected Private wallet"
                            .to_owned(),
                });
            }
            WalletConnectSessionAccountResolution::InvalidPublicAccount => {
                return Err(WalletConnectSessionRequestFailure {
                    kind: WalletConnectRequestErrorKind::Unauthorized,
                    message: "WalletConnect session Public account is invalid".to_owned(),
                });
            }
        };
        let selected_account_support = match &resolution {
            WalletConnectSessionAccountResolution::Usable(account) => {
                walletconnect_namespace_account_support(account, Some(view_session))
            }
            WalletConnectSessionAccountResolution::TemporarilyPausedWrongPrivateWallet {
                ..
            }
            | WalletConnectSessionAccountResolution::InvalidPublicAccount => {
                WalletConnectNamespaceAccountSupport::for_account_source(
                    PublicAccountSource::Derived,
                )
            }
        };
        let validation = validate_walletconnect_session_request_with_account_support(
            &session,
            &resolution,
            selected_account_support,
            &request.item.topic,
            request.item.id,
            &request.item.chain_id,
            request.parsed.clone(),
            None,
            now,
        )
        .map_err(|error| walletconnect_session_request_failure_from_error(&error))?;
        self.ensure_walletconnect_chain_enabled(&request.item.chain_id)?;
        if let WalletConnectParsedRequest::WalletSwitchEthereumChain { chain_id } =
            &validation.request
        {
            self.ensure_walletconnect_chain_enabled(&format!("eip155:{chain_id}"))?;
        }
        let Some(mut item) = validation.approval_item else {
            return Err(WalletConnectSessionRequestFailure {
                kind: WalletConnectRequestErrorKind::Internal,
                message: "WalletConnect request no longer requires approval".to_owned(),
            });
        };
        item.expiry_timestamp = request.item.expiry_timestamp;
        Ok(WalletConnectRequestUi {
            key: request.key.clone(),
            review_token: request.review_token,
            session,
            parsed: request.parsed.clone(),
            item,
            account_source,
        })
    }

    pub(in crate::root::walletconnect) fn publish_invalid_walletconnect_pending_request(
        &mut self,
        request_key: &str,
        request: &WalletConnectRequestUi,
        failure: &WalletConnectSessionRequestFailure,
        context: WalletConnectClientContext,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let response = build_walletconnect_jsonrpc_error(
            request.item.id,
            failure.kind,
            failure.message.clone(),
        );
        let topic = request.session.session_topic.clone();
        let sym_key = request.session.keys.sym_key;
        let request_key = request_key.to_owned();
        self.walletconnect
            .request_actions
            .insert(request_key.clone());
        tracing::warn!(
            target: "wallet::root::walletconnect",
            request_key = %walletconnect_request_key_log_label(&request_key),
            error = %failure.message,
            "walletconnect pending request failed revalidation"
        );
        let join = self.runtime.spawn(async move {
            publish_walletconnect_session_response(context.worker, topic, sym_key, response).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                root.walletconnect.request_actions.remove(&request_key);
                root.walletconnect.remove_pending_request(&request_key);
                if root.walletconnect.request_dialog_key.as_deref() == Some(request_key.as_str()) {
                    root.clear_walletconnect_request_dialog_state(window, cx);
                    window.close_dialog(cx);
                }
                root.walletconnect.status = Some(Arc::from(
                    "WalletConnect request is no longer valid; error response published.",
                ));
                if let Ok(Err(error)) = result {
                    root.walletconnect.error = Some(Arc::from(format!(
                        "Request was removed locally, but relay error response failed: {error}"
                    )));
                }
                root.sync_walletconnect_attention();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::root::walletconnect) fn reject_walletconnect_request(
        &mut self,
        request_key: &str,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.walletconnect.request_actions.contains(request_key) {
            return;
        }
        let Some(request) = self
            .walletconnect
            .pending_requests
            .get(request_key)
            .cloned()
        else {
            return;
        };
        let context = match self.walletconnect_client_context_for_session(&request.session, cx) {
            Ok(context) => context,
            Err(error) => {
                self.walletconnect.error = Some(error);
                cx.notify();
                return;
            }
        };
        let response = build_walletconnect_jsonrpc_error(
            request.item.id,
            WalletConnectRequestErrorKind::UserRejected,
            "User rejected WalletConnect request",
        );
        let topic = request.session.session_topic.clone();
        let sym_key = request.session.keys.sym_key;
        let request_key = request_key.to_owned();
        self.walletconnect
            .request_actions
            .insert(request_key.clone());
        tracing::info!(
            target: "wallet::root::walletconnect",
            request_key = %walletconnect_request_key_log_label(&request_key),
            method = request.item.method.as_str(),
            chain_id = request.item.chain_id.as_str(),
            dapp = request.item.dapp_name.as_str(),
            "rejecting walletconnect request"
        );
        let join = self.runtime.spawn(async move {
            publish_walletconnect_session_response(context.worker, topic, sym_key, response).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                root.walletconnect.request_actions.remove(&request_key);
                tracing::info!(
                    target: "wallet::root::walletconnect",
                    request_key = %walletconnect_request_key_log_label(&request_key),
                    relay_failed = matches!(&result, Ok(Err(_))),
                    relay_not_sent = matches!(&result, Ok(Err(error)) if walletconnect_relay_request_was_not_sent(error)),
                    "walletconnect request rejection handled"
                );
                match result {
                    Ok(Ok(())) => {
                        root.walletconnect.remove_pending_request(&request_key);
                        if root.walletconnect.request_dialog_key.as_deref()
                            == Some(request_key.as_str())
                        {
                            root.clear_walletconnect_request_dialog_state(window, cx);
                            window.close_dialog(cx);
                        }
                        root.walletconnect.status = Some(Arc::from("WalletConnect request rejected."));
                    }
                    Ok(Err(error)) if walletconnect_relay_request_was_not_sent(&error) => {
                        root.walletconnect.status = Some(Arc::from(
                            "WalletConnect relay is reconnecting; rejection was not sent. The request remains pending so you can retry.",
                        ));
                        root.walletconnect.error = Some(Arc::from(error));
                    }
                    Ok(Err(error)) => {
                        root.walletconnect.remove_pending_request(&request_key);
                        if root.walletconnect.request_dialog_key.as_deref()
                            == Some(request_key.as_str())
                        {
                            root.clear_walletconnect_request_dialog_state(window, cx);
                            window.close_dialog(cx);
                        }
                        root.walletconnect.status = Some(Arc::from("WalletConnect request rejected."));
                        root.walletconnect.error = Some(Arc::from(format!(
                            "Request was removed locally, but relay rejection failed: {error}"
                        )));
                    }
                    Err(error) => {
                        root.walletconnect.status = Some(Arc::from(
                            "WalletConnect rejection task failed. The request remains pending so you can retry.",
                        ));
                        root.walletconnect.error = Some(Arc::from(format!(
                            "WalletConnect rejection task failed: {error}"
                        )));
                    }
                }
                root.sync_walletconnect_attention();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn render_walletconnect_intent_risk(risk: &WalletConnectRisk) -> gpui::Div {
    let (message, border, background) = match risk {
        WalletConnectRisk::UnlimitedAllowance { spender } => (
            format!(
                "Unlimited allowance: {} can keep spending this token until the allowance is revoked.",
                spender.to_checksum(None)
            ),
            theme::DANGER,
            theme::DANGER_BG,
        ),
        WalletConnectRisk::ForeignTransferSource { source } => (
            format!(
                "This transferFrom call attempts to move funds from {}.",
                source.to_checksum(None)
            ),
            theme::WARNING,
            theme::WARNING_BG,
        ),
        WalletConnectRisk::AttachedNativeValue(amount) => (
            format!(
                "This token operation also sends {} to the contract.",
                amount.display
            ),
            theme::WARNING,
            theme::WARNING_BG,
        ),
        WalletConnectRisk::UndecodedContractCall { selector } => (
            format!(
                "This contract call could not be decoded{}. Inspect the raw request before approving.",
                selector.map_or_else(String::new, |selector| {
                    format!(" (selector 0x{})", alloy::hex::encode(selector))
                })
            ),
            theme::WARNING,
            theme::WARNING_BG,
        ),
    };
    walletconnect_notice(message, border, background)
}

fn render_walletconnect_intent_card(
    request_key: &str,
    intent: &WalletConnectIntentView<'_>,
    chain_id: &str,
    content_width: Pixels,
) -> gpui::Div {
    let mut card = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
        .bg(rgb(theme::SURFACE_ELEVATED))
        .p(px(12.0))
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    app_strong_text(intent.hero.verb)
                        .text_size(px(18.0))
                        .whitespace_normal(),
                )
                .child(walletconnect_approved_chain_chip(
                    &approved_chain_display_item(chain_id),
                )),
        )
        .child(render_walletconnect_intent_hero(intent));

    if walletconnect_should_render_token_contract(intent.action)
        && let Some(token) = walletconnect_intent_token_contract(&intent.amount)
    {
        card = card.child(render_walletconnect_address_row(
            "Token contract",
            None,
            token,
            Some(walletconnect_token_contract_recognition(&intent.amount)),
            request_key,
            content_width,
        ));
    }

    let party_count = intent.parties.len();
    let mut party_flow = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_3()
        .pt(px(6.0));
    for (index, party) in intent.parties.iter().enumerate() {
        party_flow = party_flow.child(render_walletconnect_party_row(
            party,
            request_key,
            content_width,
        ));
        if intent.action.allows_party_connector() && index + 1 < party_count {
            party_flow = party_flow.child(
                div()
                    .w_full()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(gpui_component::Icon::new(IconName::ArrowDown)),
            );
        }
    }
    if party_count > 0 {
        card = card.child(party_flow);
    }
    if let Some(attached_native) = intent.attached_native.as_ref() {
        card = card.child(
            app_muted_text(format!(
                "Attached native value: {}",
                attached_native.display
            ))
            .whitespace_normal(),
        );
    }
    card
}

fn render_walletconnect_intent_hero(intent: &WalletConnectIntentView<'_>) -> gpui::Div {
    let mut hero = div().w_full().min_w(px(0.0)).flex().items_start().gap_3();
    if let Some(path) = intent.icon.clone() {
        hero = hero.child(img(path).size(px(34.0)).rounded_full().flex_none());
    }
    let mut body = div().min_w(px(0.0)).flex_1().flex().flex_col().gap_1();
    match &intent.hero.summary {
        WalletConnectHeroSummary::Amount => {
            let (label, danger) = walletconnect_intent_amount_label(&intent.amount);
            body = body.child(
                app_strong_text(label)
                    .text_size(px(22.0))
                    .text_color(rgb(if danger { theme::DANGER } else { theme::TEXT }))
                    .whitespace_normal(),
            );
            if let Some(context) = intent.usd_context.as_deref() {
                body = body.child(app_muted_text(walletconnect_approximate_usd_label(context)));
            }
            if let Some(effect) = intent.hero.approval_effect.as_deref() {
                body = body.child(app_muted_text(effect.to_owned()).whitespace_normal());
            }
        }
        WalletConnectHeroSummary::PersonalMessage(summary) => {
            body = body.child(app_strong_text(format!("{} bytes", summary.bytes)));
            body = body.child(app_muted_text(summary.preview.as_deref().map_or_else(
                || "Binary or control-heavy message; inspect raw request".to_owned(),
                |preview| format!("Message: {preview}"),
            )));
        }
        WalletConnectHeroSummary::TypedData(summary) => {
            body = body.child(app_strong_text(
                summary
                    .domain_name
                    .as_deref()
                    .unwrap_or("No domain name supplied")
                    .to_owned(),
            ));
            body = body.child(app_muted_text(format!(
                "Primary type: {}",
                summary.primary_type
            )));
        }
        WalletConnectHeroSummary::UndecodedCall { selector } => {
            body = body.child(app_strong_text("Could not decode contract call"));
            body = body.child(app_muted_text(format!(
                "{}; inspect raw request",
                selector.map_or_else(
                    || "No calldata selector supplied".to_owned(),
                    |selector| format!("Selector 0x{}", alloy::hex::encode(selector)),
                )
            )));
        }
        WalletConnectHeroSummary::None => {
            body = body.child(app_strong_text("Review request details"));
        }
    }
    hero.child(body)
}

fn walletconnect_intent_amount_label(amount: &WalletConnectAmount) -> (String, bool) {
    match amount {
        WalletConnectAmount::KnownToken { display, .. }
        | WalletConnectAmount::Native { display, .. }
        | WalletConnectAmount::Unlimited { display, .. }
        | WalletConnectAmount::RawToken { display, .. } => (
            display.clone(),
            matches!(amount, &WalletConnectAmount::Unlimited { .. }),
        ),
        WalletConnectAmount::None => ("No amount specified".to_owned(), false),
    }
}

const fn walletconnect_intent_token_contract(amount: &WalletConnectAmount) -> Option<Address> {
    match amount {
        WalletConnectAmount::KnownToken { token, .. }
        | WalletConnectAmount::Unlimited { token, .. }
        | WalletConnectAmount::RawToken { token, .. } => Some(*token),
        WalletConnectAmount::Native { .. } | WalletConnectAmount::None => None,
    }
}

fn render_walletconnect_party_row(
    party: &WalletConnectParty,
    request_key: &str,
    content_width: Pixels,
) -> gpui::Div {
    let badge = walletconnect_party_badge_label(party.role, &party.badge);
    render_walletconnect_address_row(
        walletconnect_party_role_label(party.role),
        Some(party.role),
        party.address,
        badge.as_deref(),
        request_key,
        content_width,
    )
}

fn render_walletconnect_address_row(
    role: &'static str,
    party_role: Option<WalletConnectPartyRole>,
    address: Address,
    badge: Option<&str>,
    request_key: &str,
    content_width: Pixels,
) -> gpui::Div {
    let full_address = address.to_checksum(None);
    let copy_id = SharedString::from(format!("walletconnect-request-{request_key}-{role}-copy"));
    let body = div()
        .min_w(px(0.0))
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            app_muted_text(role)
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEXT_SUBTLE)),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    app_strong_text(walletconnect_party_address_label(
                        party_role.unwrap_or(WalletConnectPartyRole::Contract),
                        &address,
                        content_width,
                    ))
                    .font_family(APP_MONO_FONT_FAMILY),
                )
                .when_some(badge, |this, badge| {
                    this.child(walletconnect_party_badge_chip(badge))
                })
                .child(clipboard_with_toast(copy_id, full_address)),
        );
    div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .items_start()
        .gap_2()
        .child(body)
}

const fn walletconnect_party_role_label(role: WalletConnectPartyRole) -> &'static str {
    match role {
        WalletConnectPartyRole::Sender => "From",
        WalletConnectPartyRole::Recipient => "To",
        WalletConnectPartyRole::Spender => "Spender",
        WalletConnectPartyRole::Source => "Source",
        WalletConnectPartyRole::Caller => "Caller",
        WalletConnectPartyRole::Contract => "Contract",
        WalletConnectPartyRole::Creator => "Creator",
        WalletConnectPartyRole::Signer => "Signer",
        WalletConnectPartyRole::WrappedNativeContract => "Wrapped-native contract",
    }
}

fn walletconnect_party_badge_chip(label: &str) -> gpui::Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
        .bg(rgb(theme::SURFACE_HOVER_SUBTLE))
        .px(px(5.0))
        .py(px(2.0))
        .text_size(px(11.0))
        .text_color(rgb(theme::TEXT_MUTED))
        .whitespace_normal()
        .child(SharedString::from(label.to_owned()))
}

fn render_walletconnect_request_provenance(
    request: &WalletConnectRequestUi,
    intent: &WalletConnectIntentView<'_>,
) -> gpui::Div {
    let expiry = match walletconnect_request_expiry_status(
        request.item.expiry_timestamp,
        current_unix_seconds(),
    ) {
        WalletConnectRequestExpiryStatus::Missing => "No request expiry supplied".to_owned(),
        WalletConnectRequestExpiryStatus::Remaining(seconds) => {
            format!("Expires in {}", walletconnect_duration_label(seconds))
        }
        WalletConnectRequestExpiryStatus::Expired => "Expired at deadline".to_owned(),
    };
    div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_1()
        .px(px(2.0))
        .child(walletconnect_kv_row("Site", intent.provenance.site.clone()))
        .when_some(intent.provenance.dapp_name.as_ref(), |this, name| {
            this.child(walletconnect_provenance_dapp_row(name))
        })
        .when(
            walletconnect_selected_account_provenance_visible(
                request.item.account,
                &intent.parties,
            ),
            |this| {
                this.child(walletconnect_kv_row(
                    "Public account",
                    short_address(&request.item.account),
                ))
            },
        )
        .child(walletconnect_kv_row("Request expiry", expiry))
}

fn walletconnect_provenance_dapp_row(name: &str) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .text_size(APP_TEXT_SIZE)
        .child(
            div()
                .flex_none()
                .text_color(rgb(theme::TEXT_MUTED))
                .child("Dapp"),
        )
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .text_align(gpui::TextAlign::Right)
                .truncate()
                .text_color(rgb(theme::TEXT_MUTED))
                .child(SharedString::from(name.to_owned())),
        )
}

fn walletconnect_duration_label(seconds: u64) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

fn render_walletconnect_request_disclosure(
    root: &Entity<WalletRoot>,
    request_key: &str,
    disclosure: WalletConnectRequestDisclosure,
    label: &'static str,
    open: bool,
    content: gpui::Div,
) -> gpui::Div {
    let toggle_root = root.clone();
    let toggle_key = request_key.to_owned();
    let id = format!("walletconnect-request-{request_key}-{label}-disclosure");
    let mut disclosure_row = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            app_button_base(SharedString::from(id))
                .ghost()
                .w_full()
                .small()
                .justify_between()
                .child(app_muted_text(label))
                .child(gpui_component::Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                }))
                .on_click(move |_event, _window, cx| {
                    toggle_root.update(cx, |root, cx| {
                        root.walletconnect
                            .toggle_request_disclosure(&toggle_key, disclosure);
                        cx.notify();
                    });
                }),
        );
    if open {
        disclosure_row = disclosure_row.child(content);
    }
    disclosure_row
}

fn render_walletconnect_transaction_details(
    details: &super::super::intent::WalletConnectTransactionDetails<'_>,
    request_key: &str,
    content_width: Pixels,
) -> gpui::Div {
    let transaction = details.transaction;
    let mut content = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(theme::BORDER_SUBTLE))
        .bg(rgb(theme::SURFACE))
        .p(px(8.0))
        .child(walletconnect_kv_row(
            "Chain ID",
            details.chain_id.to_string(),
        ));
    if let Some(target) = transaction.to {
        content = content.child(render_walletconnect_address_row(
            "Target",
            None,
            target,
            Some("Contract target"),
            request_key,
            content_width,
        ));
    } else {
        content = content.child(walletconnect_kv_row(
            "Target",
            "Contract creation".to_owned(),
        ));
    }
    content
        .child(walletconnect_kv_row(
            "Native value",
            transaction
                .value
                .map_or_else(|| "0".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Gas",
            transaction
                .gas
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Gas price",
            transaction
                .gas_price
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Max fee per gas",
            transaction
                .max_fee_per_gas
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Max priority fee",
            transaction
                .max_priority_fee_per_gas
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Nonce",
            transaction
                .nonce
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Transaction type",
            transaction
                .transaction_type
                .map_or_else(|| "Not supplied".to_owned(), |value| value.to_string()),
        ))
        .child(walletconnect_kv_row(
            "Access list",
            if transaction.access_list.is_some() {
                "Present".to_owned()
            } else {
                "None supplied".to_owned()
            },
        ))
        .child(walletconnect_kv_row(
            "Calldata selector",
            walletconnect_transaction_selector(transaction).map_or_else(
                || "None".to_owned(),
                |selector| format!("0x{}", alloy::hex::encode(selector)),
            ),
        ))
}

fn walletconnect_transaction_selector(
    transaction: &WalletConnectEvmTransaction,
) -> Option<[u8; 4]> {
    let data = transaction.data.as_deref()?;
    let data = data.strip_prefix("0x").unwrap_or(data);
    let bytes = alloy::hex::decode(data).ok()?;
    bytes.get(..4)?.try_into().ok()
}
