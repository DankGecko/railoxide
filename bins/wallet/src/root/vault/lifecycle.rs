use std::collections::BTreeSet;

#[cfg(not(feature = "hardware"))]
use super::hardware_device_wallet_select_label;
use super::{
    Arc, BroadcasterActivityTab, ChainUtxoState, Context, DesktopViewSession, ParentElement,
    SearchableVec, Styled, VaultError, VaultState, ViewUnlock, WalletMetadataBundle, WalletRoot,
    WalletSetupMode, WalletTab, Window, WindowExt, Zeroizing, app_strong_text,
    default_wallet_label_for_metadata, dialog_content_max_height, dialog_max_height,
    hardware_device_kind_from_wallet_select_value, px, scrollable_dialog_content,
    secondary_dialog_content_width, vault_error_kind, vault_error_message,
    wallet_options_from_metadata, wallet_select_items_from_metadata,
    wallet_select_value_for_selected_wallet,
};

pub(in crate::root) const fn vault_lock_is_allowed(
    maintenance_idle: bool,
    wallet_deletion_in_progress: bool,
) -> bool {
    maintenance_idle && !wallet_deletion_in_progress
}
#[cfg(feature = "hardware")]
use super::{
    HardwareProfileMetadata, HardwareProfileUnlockPurpose, HardwareProfileUnlockState,
    hardware_device_kind_from_source,
};

impl WalletRoot {
    pub(in crate::root) fn select_wallet(
        &mut self,
        wallet_id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.manage_wallets.deleting_wallet_id.is_some() {
            self.sync_wallet_select(window, cx);
            return;
        }
        if let Some(device_kind) = hardware_device_kind_from_wallet_select_value(wallet_id) {
            #[cfg(feature = "hardware")]
            {
                self.open_hardware_profile_unlock_dialog_for_device(
                    device_kind,
                    HardwareProfileUnlockPurpose::Open,
                    window,
                    cx,
                );
                self.sync_wallet_select(window, cx);
            }
            #[cfg(not(feature = "hardware"))]
            {
                self.set_vault_error(
                    format!(
                        "{} support is not enabled in this build.",
                        hardware_device_wallet_select_label(device_kind)
                    ),
                    cx,
                );
                self.sync_wallet_select(window, cx);
            }
            return;
        }
        if self.selected_wallet_id.as_deref() == Some(wallet_id) {
            return;
        }
        window.close_all_dialogs(cx);
        self.switch_active_wallet(wallet_id, window, cx);
    }

    pub(in crate::root) fn open_add_wallet_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_add_wallet_dialog_with_mode(WalletSetupMode::Choose, window, cx);
    }

    pub(super) fn open_add_wallet_dialog_with_mode(
        &mut self,
        initial_mode: WalletSetupMode,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.close_all_dialogs(cx);
        self.generated_seed = None;
        self.vault_error = None;
        self.wallet_setup_mode = initial_mode;
        let label = default_wallet_label_for_metadata(&self.wallet_metadata);
        let root = cx.entity();
        let dialog_width = (window.viewport_size().width * 0.92).min(px(520.0));
        let dialog_max_height = dialog_max_height(window);
        let content_max_height = dialog_content_max_height(window);
        let content_width = secondary_dialog_content_width(dialog_width);
        window.open_dialog(cx, move |dialog, _window, cx| {
            let content_root = root.clone();
            dialog
                .w(dialog_width)
                .max_h(dialog_max_height)
                .title(app_strong_text("Add wallet"))
                .child(scrollable_dialog_content(
                    content_max_height,
                    content_root
                        .read(cx)
                        .render_add_wallet_dialog_content(content_root.clone(), content_width),
                ))
        });
        cx.defer_in(window, move |root, window, cx| {
            root.set_wallet_name_input(&label, window, cx);
            root.add_wallet_password_input
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
    }

    #[cfg_attr(not(feature = "hardware"), allow(clippy::needless_pass_by_ref_mut))]
    pub(super) fn switch_active_wallet(
        &mut self,
        wallet_id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.manage_wallets.deleting_wallet_id.is_some() {
            self.sync_wallet_select(window, cx);
            return;
        }
        #[cfg(feature = "hardware")]
        if self.wallet_metadata.iter().any(|metadata| {
            metadata.wallet_uuid == wallet_id
                && hardware_device_kind_from_source(metadata.source).is_some()
        }) {
            self.open_hardware_profile_unlock_dialog_for_wallet(
                Arc::from(wallet_id.to_owned()),
                window,
                cx,
            );
            return;
        }
        let Some(store) = self.vault_store.clone() else {
            self.set_vault_error("Wallet vault storage is unavailable", cx);
            return;
        };
        let Some(current_session) = self.view_session.clone() else {
            self.open_wallet_from_vault_view_unlock(wallet_id, window, cx);
            return;
        };

        let current_wallet_id: Arc<str> = Arc::from(current_session.wallet_id().to_owned());
        let active_wallet_generation = self.active_wallet_generation;
        self.wallet_switch_generation = self.wallet_switch_generation.wrapping_add(1);
        let switch_generation = self.wallet_switch_generation;
        self.vault_error = None;
        let wallet_id_string = wallet_id.to_owned();
        let metadata = self.wallet_metadata.clone();
        let join = self.runtime.spawn_blocking(move || {
            store.load_view_session_with_view_session(current_session.as_ref(), &wallet_id_string)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                if root.wallet_switch_generation != switch_generation
                    || !root.is_active_wallet_generation(
                        current_wallet_id.as_ref(),
                        active_wallet_generation,
                    )
                {
                    return;
                }
                match result {
                    Ok(Ok(session)) => root.install_view_session(session, metadata, window, cx),
                    Ok(Err(error)) => {
                        root.handle_vault_error(&error, cx);
                        root.sync_wallet_select(window, cx);
                    }
                    Err(error) => {
                        root.set_vault_error(
                            format!("Failed to switch wallet: {error}").as_str(),
                            cx,
                        );
                        root.sync_wallet_select(window, cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn open_wallet_from_vault_view_unlock(
        &mut self,
        wallet_id: &str,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(store) = self.vault_store.clone() else {
            self.set_vault_error("Wallet vault storage is unavailable", cx);
            return;
        };
        let Some(vault_view_unlock) = self.vault_view_unlock.clone() else {
            self.set_vault_error("Wallet vault is locked", cx);
            return;
        };

        let active_wallet_generation = self.active_wallet_generation;
        self.wallet_switch_generation = self.wallet_switch_generation.wrapping_add(1);
        let switch_generation = self.wallet_switch_generation;
        self.vault_error = None;
        let wallet_id_string = wallet_id.to_owned();
        let metadata = self.wallet_metadata.clone();
        let join = self.runtime.spawn_blocking(move || {
            store.load_view_session_with_view_unlock(&vault_view_unlock, &wallet_id_string)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                if root.wallet_switch_generation != switch_generation
                    || root.active_wallet_generation != active_wallet_generation
                {
                    return;
                }
                match result {
                    Ok(Ok(session)) => root.install_view_session(session, metadata, window, cx),
                    Ok(Err(error)) => {
                        root.handle_vault_error(&error, cx);
                        root.sync_wallet_select(window, cx);
                    }
                    Err(error) => {
                        root.set_vault_error(
                            format!("Failed to open wallet: {error}").as_str(),
                            cx,
                        );
                        root.sync_wallet_select(window, cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    #[allow(dead_code)]
    pub(super) fn deactivate_wallet_and_switch(
        &mut self,
        wallet_id: &str,
        password: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(store) = self.vault_store.clone() else {
            self.set_vault_error("Wallet vault storage is unavailable", cx);
            return;
        };
        if let Err(error) = store.deactivate_wallet(password, wallet_id) {
            self.handle_vault_error(&error, cx);
            return;
        }
        let metadata = match store.list_wallet_metadata(password) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.handle_vault_error(&error, cx);
                return;
            }
        };
        self.wallet_metadata.clone_from(&metadata);
        self.wallet_options = wallet_options_from_metadata(metadata.clone());

        if self.selected_wallet_id.as_deref() != Some(wallet_id) {
            self.sync_wallet_select(window, cx);
            cx.notify();
            return;
        }

        let Some(next_wallet_id) = self
            .wallet_options
            .first()
            .map(|option| Arc::clone(&option.wallet_id))
        else {
            self.set_vault_error("No active wallet remains after deactivation", cx);
            return;
        };
        match store.load_view_session(password, next_wallet_id.as_ref()) {
            Ok(session) => self.install_view_session(session, metadata, window, cx),
            Err(error) => self.handle_vault_error(&error, cx),
        }
    }

    pub(super) fn install_view_session(
        &mut self,
        session: DesktopViewSession,
        metadata: Vec<WalletMetadataBundle>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.install_view_session_with_dialog_policy(
            session,
            metadata,
            true,
            wallet_ops::CreatedWalletChainInitPolicy::Resumed,
            window,
            cx,
        );
    }

    pub(in crate::root) fn install_view_session_after_management(
        &mut self,
        session: DesktopViewSession,
        metadata: Vec<WalletMetadataBundle>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.install_view_session_with_dialog_policy(
            session,
            metadata,
            false,
            wallet_ops::CreatedWalletChainInitPolicy::Resumed,
            window,
            cx,
        );
    }

    #[cfg(feature = "hardware")]
    fn active_hardware_profile_for_wallet(&self) -> Option<HardwareProfileMetadata> {
        let selected_wallet_id = self.selected_wallet_id.as_deref()?;
        let account = self
            .wallet_metadata
            .iter()
            .find(|wallet| wallet.wallet_uuid == selected_wallet_id)
            .and_then(|wallet| wallet.hardware_account.as_ref())?;

        self.hardware_profile_unlock
            .profile
            .as_ref()
            .filter(|profile| profile.profile_id == account.profile_id)
            .cloned()
            .or_else(|| {
                self.active_hardware_profile
                    .as_ref()
                    .filter(|profile| profile.profile_id == account.profile_id)
                    .cloned()
            })
    }

    fn install_view_session_with_dialog_policy(
        &mut self,
        session: DesktopViewSession,
        metadata: Vec<WalletMetadataBundle>,
        close_dialogs: bool,
        created_wallet_init_policy: wallet_ops::CreatedWalletChainInitPolicy,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.manage_wallets.deleting_wallet_id.is_some() {
            self.sync_wallet_select(window, cx);
            return;
        }
        self.supersede_wallet_sessions();
        let vault_view_unlock = Arc::new(session.clone_vault_view_unlock());
        let session = Arc::new(session);
        let wallet_id: Arc<str> = Arc::from(session.wallet_id().to_owned());
        if close_dialogs {
            window.close_all_dialogs(cx);
        }
        self.active_wallet_generation = self.active_wallet_generation.wrapping_add(1);
        self.view_session = Some(session);
        self.vault_view_unlock = Some(vault_view_unlock);
        self.wallet_metadata = metadata;
        self.wallet_options = wallet_options_from_metadata(self.wallet_metadata.clone());
        self.selected_wallet_id = Some(Arc::clone(&wallet_id));
        self.ui_state.last_wallet_id = Some(wallet_id.as_ref().to_owned());
        self.save_ui_state();
        #[cfg(feature = "hardware")]
        {
            self.active_hardware_profile = self.active_hardware_profile_for_wallet();
        }
        self.sync_wallet_select(window, cx);
        self.reset_wallet_scoped_state(cx);
        self.reload_address_books(cx);
        self.reload_broadcaster_preferences(cx);
        self.reload_public_accounts(window, cx);
        self.setup_password = None;
        self.generated_seed = None;
        #[cfg(feature = "hardware")]
        {
            self.hardware_profile_unlock = HardwareProfileUnlockState::default();
            self.clear_hardware_profile_sensitive_inputs(window, cx);
        }
        self.hardware_wallet_creation_in_progress = false;
        self.hardware_wallet_creation_generation =
            self.hardware_wallet_creation_generation.wrapping_add(1);
        self.hardware_wallet_creation_intent = None;
        self.clear_hardware_wallet_restore_account_index(window, cx);
        self.add_wallet_password_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.import_mnemonic_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.clear_key_export_dialog_state(window, cx);
        self.vault_error = None;
        self.vault_state = VaultState::ViewUnlocked;
        self.wallet_setup_mode = WalletSetupMode::Choose;
        self.ensure_waku_started(cx);
        self.ensure_chain_load_with_start_policy(
            self.selected_chain,
            Some(created_wallet_init_policy.sync_start_policy()),
            cx,
        );
        self.initialize_created_wallet_chain_metadata(created_wallet_init_policy);
        cx.notify();
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    pub(in crate::root) fn sync_wallet_select(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let items = wallet_select_items_from_metadata(&self.wallet_metadata);
        let selected_value = self.selected_wallet_id.as_ref().map(|wallet_id| {
            wallet_select_value_for_selected_wallet(wallet_id, &self.wallet_metadata)
        });
        self.wallet_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            if let Some(value) = selected_value.as_ref() {
                select.set_selected_value(value, window, cx);
            } else {
                select.set_selected_index(None, window, cx);
            }
        });
    }

    pub(super) fn enter_view_unlocked(
        &mut self,
        session: DesktopViewSession,
        metadata: Vec<WalletMetadataBundle>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.install_view_session(session, metadata, window, cx);
    }

    pub(super) fn enter_new_wallet_view_unlocked(
        &mut self,
        session: DesktopViewSession,
        metadata: Vec<WalletMetadataBundle>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.install_view_session_with_dialog_policy(
            session,
            metadata,
            true,
            wallet_ops::CreatedWalletChainInitPolicy::InitialCreate,
            window,
            cx,
        );
    }

    pub(super) fn enabled_chain_ids_for_created_wallet(&self) -> BTreeSet<u64> {
        self.effective_chain_configs
            .values()
            .filter(|chain| chain.enabled)
            .map(|chain| chain.chain_id)
            .collect()
    }

    fn initialize_created_wallet_chain_metadata(
        &mut self,
        init_policy: wallet_ops::CreatedWalletChainInitPolicy,
    ) {
        let Some(view_session) = self.view_session.clone() else {
            return;
        };
        let Some(vault_store) = self.vault_store.as_ref() else {
            return;
        };
        let effective_chains = self.effective_chain_configs.clone();
        let db = vault_store.db();
        let http = self.http.clone();
        let skip_chain_id = Some(self.selected_chain);

        let task = self.runtime.spawn(async move {
            wallet_ops::initialize_created_wallet_chain_metadata_for_session(
                view_session,
                effective_chains,
                db,
                http,
                skip_chain_id,
                init_policy,
            )
            .await;
        });
        self.wallet_sync_lifecycle.track_wallet_task(task);
    }

    pub(in crate::root) fn enter_password_metadata_unlocked(
        &mut self,
        metadata: Vec<WalletMetadataBundle>,
        vault_view_unlock: Arc<ViewUnlock>,
        setup_password: Option<Zeroizing<String>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let active = wallet_options_from_metadata(metadata.clone());
        if active.is_empty() {
            if let Some(password) = setup_password {
                self.set_default_wallet_name_from_password(password.as_str(), window, cx);
                self.setup_password = Some(password);
            }
            self.vault_view_unlock = Some(vault_view_unlock);
            self.vault_error = None;
            self.vault_state = VaultState::SetupWallet;
            self.wallet_setup_mode = WalletSetupMode::Choose;
            self.ensure_waku_started(cx);
            cx.notify();
            return;
        }

        window.close_all_dialogs(cx);
        self.active_wallet_generation = self.active_wallet_generation.wrapping_add(1);
        self.view_session = None;
        self.vault_view_unlock = Some(vault_view_unlock);
        self.wallet_metadata = metadata;
        self.wallet_options = active;
        self.selected_wallet_id = None;
        self.sync_wallet_select(window, cx);
        self.shutdown_wallet_session_store();
        self.reset_wallet_scoped_state(cx);
        self.setup_password = None;
        self.generated_seed = None;
        self.clear_key_export_dialog_state(window, cx);
        #[cfg(feature = "hardware")]
        {
            self.active_hardware_profile = None;
            self.hardware_profile_unlock = HardwareProfileUnlockState::default();
            self.clear_hardware_profile_sensitive_inputs(window, cx);
        }
        self.hardware_wallet_creation_in_progress = false;
        self.hardware_wallet_creation_generation =
            self.hardware_wallet_creation_generation.wrapping_add(1);
        self.hardware_wallet_creation_intent = None;
        self.clear_hardware_wallet_restore_account_index(window, cx);
        self.vault_error = None;
        self.vault_state = VaultState::ViewUnlocked;
        self.wallet_setup_mode = WalletSetupMode::Choose;
        self.ensure_waku_started(cx);
        cx.notify();
    }

    pub(in crate::root) fn lock_vault(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !vault_lock_is_allowed(
            self.maintenance_controller.read(cx).is_idle(),
            self.manage_wallets.deleting_wallet_id.is_some(),
        ) {
            return;
        }
        self.shutdown_wallet_session_store();
        self.clear_private_broadcaster_progress_state();
        self.stop_waku();
        window.close_all_dialogs(cx);
        self.clear_spend_authorization(cx);
        self.view_session = None;
        self.wallet_metadata.clear();
        self.wallet_options.clear();
        self.private_address_book.clear();
        self.public_address_book.clear();
        self.set_broadcaster_preferences(wallet_ops::vault::BroadcasterPreferences::default(), cx);
        self.broadcaster_preference_error = None;
        self.address_book.search_query = Arc::from("");
        self.address_book
            .search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.address_book.clear_dialog_state(window, cx);
        self.favorite_broadcaster_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.banned_broadcaster_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.active_broadcaster_tab = BroadcasterActivityTab::default();
        self.address_book_save_error = None;
        self.selected_wallet_id = None;
        self.active_wallet_generation = self.active_wallet_generation.wrapping_add(1);
        self.sync_wallet_select(window, cx);
        self.send_forms.clear();
        self.unshield_forms.clear();
        self.reset_public_wallet_state(window, cx);
        self.private_action_form = None;
        self.broadcaster_picker = None;
        self.active_wallet_tab = WalletTab::default();
        self.setup_password = None;
        self.vault_view_unlock = None;
        self.generated_seed = None;
        self.clear_key_export_dialog_state(window, cx);
        #[cfg(feature = "hardware")]
        {
            self.active_hardware_profile = None;
            self.hardware_profile_unlock = HardwareProfileUnlockState::default();
            self.clear_hardware_profile_sensitive_inputs(window, cx);
        }
        self.hardware_wallet_creation_in_progress = false;
        self.hardware_wallet_creation_generation =
            self.hardware_wallet_creation_generation.wrapping_add(1);
        self.hardware_wallet_creation_intent = None;
        self.clear_hardware_wallet_restore_account_index(window, cx);
        self.vault_error = None;
        self.repair_cache_error = None;
        self.vault_state = VaultState::UnlockVault;
        self.wallet_setup_mode = WalletSetupMode::Choose;
        self.focus_vault_input_on_render = true;
        for state in self.chain_states.values_mut() {
            *state = ChainUtxoState::Idle;
        }
        self.sync_utxo_table(cx);
        cx.notify();
    }

    pub(in crate::root) fn handle_vault_error(
        &mut self,
        error: &VaultError,
        cx: &mut Context<'_, Self>,
    ) {
        tracing::warn!(
            error_kind = vault_error_kind(error),
            "desktop wallet vault operation failed"
        );
        self.set_vault_error(vault_error_message(error), cx);
    }

    pub(in crate::root) fn set_vault_error(
        &mut self,
        message: impl Into<Arc<str>>,
        cx: &mut Context<'_, Self>,
    ) {
        self.vault_error = Some(message.into());
        cx.notify();
    }
}
