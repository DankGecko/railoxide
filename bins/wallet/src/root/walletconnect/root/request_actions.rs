use super::*;

impl WalletRoot {
    pub(in crate::root::walletconnect) fn render_walletconnect_request(
        &self,
        root: &Entity<Self>,
        request: &WalletConnectRequestUi,
    ) -> gpui::Div {
        let approve_root = root.clone();
        let reject_root = root.clone();
        let request_key = Arc::<str>::from(request.key.as_str());
        let reject_key = Arc::clone(&request_key);
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
        let approve_label = walletconnect_request_approve_label(
            in_flight,
            hardware_request,
            hardware_typed_data_hash_fallback,
        );
        let mut card = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::BORDER_SUBTLE))
            .bg(rgb(theme::SURFACE_ELEVATED))
            .p(px(10.0))
            .child(walletconnect_kv_element_row(
                "Dapp",
                app_strong_text(request.item.dapp_name.clone()),
            ))
            .child(walletconnect_kv_row(
                "Method",
                request.item.method.as_str().to_owned(),
            ))
            .child(walletconnect_kv_element_row(
                "Chain",
                walletconnect_approved_chain_chip(&approved_chain_display_item(
                    &request.item.chain_id,
                )),
            ))
            .child(walletconnect_kv_row(
                "Public account",
                short_address(&request.item.account),
            ));
        if let Some(summary) = request.item.decoded_summary.as_ref() {
            card = card.child(walletconnect_kv_row(
                "Decoded summary",
                erc20_summary_label(summary),
            ));
        }
        card = card.child(walletconnect_raw_details(&request.item.raw_details));
        if hardware_typed_data_hash_fallback {
            card = card.child(walletconnect_notice(
                "This hardware session will use the device's EIP-712 hash-signing fallback. RailOxide computed the typed-data hashes from the validated request and will verify the signature before responding, but the device may show hashes instead of structured fields. Continue only if you accept this reduced device visibility.",
                theme::WARNING,
                theme::WARNING_BG,
            ));
        }
        if matches!(request.account_source, PublicAccountSource::HardwareDerived) {
            card = card.child(walletconnect_notice(
                hardware_walletconnect_notice(request.item.method),
                theme::WARNING,
                theme::WARNING_BG,
            ));
            #[cfg(feature = "hardware")]
            {
                if self.current_session_needs_trezor_app_passphrase() {
                    card = card.child(walletconnect_trezor_app_passphrase_input(
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
            card = card.child(render_walletconnect_approval_stepper(progress));
        }
        card.child(
            div()
                .flex()
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
                .child(
                    app_button(
                        SharedString::from(format!(
                            "walletconnect-request-approve-{}",
                            request.key
                        )),
                        approve_label,
                    )
                    .primary()
                    .small()
                    .loading(in_flight)
                    .disabled(in_flight)
                    .on_click(move |_event, window, cx| {
                        let key = Arc::clone(&request_key);
                        approve_root.update(cx, |root, cx| {
                            root.approve_walletconnect_request(key.as_ref(), window, cx);
                        });
                    }),
                ),
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
                window,
                cx,
            );
        } else {
            let intent = SpendAuthorizationIntent::WalletConnectRequest {
                request_key: request_key.to_owned(),
                review_token: request.review_token,
            };
            let summary = walletconnect_request_authorization_summary(&request);
            self.request_spend_authorization(intent, summary, window, cx);
        }
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    pub(in crate::root) fn submit_walletconnect_request_authorized(
        &mut self,
        request_key: &str,
        review_token: u64,
        vault_password: Zeroizing<String>,
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
                        if root.walletconnect.request_dialog_key.as_deref()
                            == Some(request_key.as_str())
                        {
                            root.clear_trezor_app_passphrase_input(window, cx);
                            if show_completion {
                                root.walletconnect.request_dialog_open = true;
                            } else {
                                root.walletconnect.request_dialog_open = false;
                                root.walletconnect.request_dialog_key = None;
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
                    root.walletconnect.request_dialog_open = false;
                    root.walletconnect.request_dialog_key = None;
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
                            root.walletconnect.request_dialog_open = false;
                            root.walletconnect.request_dialog_key = None;
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
                            root.walletconnect.request_dialog_open = false;
                            root.walletconnect.request_dialog_key = None;
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
