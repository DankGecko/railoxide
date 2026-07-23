#[cfg(not(feature = "hardware"))]
use super::HardwareWalletSyncIntent;
use super::{
    Arc, Context, DesktopViewSession, Focusable, HardwareDeviceKind, PRIMARY_WALLET_LABEL,
    VaultError, VaultState, ViewUnlock, WalletOption, WalletRoot, WalletSetupMode, WalletSource,
    Window, Zeroizing, default_hardware_wallet_setup_intent, generate_opaque_id,
    generate_seed_material, remembered_wallet_option, wallet_options_from_metadata,
};
#[cfg(feature = "hardware")]
use super::{HardwareProfileUnlockPurpose, parse_hardware_wallet_restore_account_index};

struct VaultUnlockResult {
    session: Option<DesktopViewSession>,
    metadata: Vec<wallet_ops::vault::WalletMetadataBundle>,
    vault_view_unlock: Arc<ViewUnlock>,
    setup_password: Option<Zeroizing<String>>,
    #[cfg(feature = "hardware")]
    remembered_hardware_wallet_id: Option<Arc<str>>,
}

impl VaultUnlockResult {
    const fn new(
        session: Option<DesktopViewSession>,
        metadata: Vec<wallet_ops::vault::WalletMetadataBundle>,
        vault_view_unlock: Arc<ViewUnlock>,
        setup_password: Option<Zeroizing<String>>,
    ) -> Self {
        Self {
            session,
            metadata,
            vault_view_unlock,
            setup_password,
            #[cfg(feature = "hardware")]
            remembered_hardware_wallet_id: None,
        }
    }

    #[cfg(feature = "hardware")]
    const fn remembered_hardware(
        wallet_id: Arc<str>,
        metadata: Vec<wallet_ops::vault::WalletMetadataBundle>,
        vault_view_unlock: Arc<ViewUnlock>,
    ) -> Self {
        Self {
            session: None,
            metadata,
            vault_view_unlock,
            setup_password: None,
            remembered_hardware_wallet_id: Some(wallet_id),
        }
    }
}

fn load_first_password_unlockable_wallet_session(
    store: &wallet_ops::vault::DesktopVaultStore,
    vault_view_unlock: &ViewUnlock,
    active: &[WalletOption],
    skip_wallet_id: Option<&str>,
) -> Result<Option<DesktopViewSession>, VaultError> {
    for wallet in active {
        if skip_wallet_id == Some(wallet.wallet_id.as_ref()) {
            continue;
        }
        match store.load_view_session_with_view_unlock(vault_view_unlock, wallet.wallet_id.as_ref())
        {
            Ok(session) => return Ok(Some(session)),
            Err(
                VaultError::HardwareWalletViewRequiresDevice
                | VaultError::UnsupportedHardwareCustodyBackend(_),
            ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

pub(in crate::root) fn load_preferred_password_unlockable_wallet_session(
    store: &wallet_ops::vault::DesktopVaultStore,
    vault_view_unlock: &ViewUnlock,
    active: &[WalletOption],
    remembered_wallet_id: Option<&str>,
) -> Result<Option<DesktopViewSession>, VaultError> {
    let mut skip_wallet_id = None;
    if let Some(wallet) = remembered_wallet_option(active, remembered_wallet_id)
        && !wallet.source.is_hardware_derived()
    {
        match store.load_view_session_with_view_unlock(vault_view_unlock, wallet.wallet_id.as_ref())
        {
            Ok(session) => return Ok(Some(session)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    wallet_id = wallet.wallet_id.as_ref(),
                    "remembered wallet could not be opened; falling back"
                );
                skip_wallet_id = Some(wallet.wallet_id.as_ref().to_owned());
            }
        }
    }

    load_first_password_unlockable_wallet_session(
        store,
        vault_view_unlock,
        active,
        skip_wallet_id.as_deref(),
    )
}

impl WalletRoot {
    pub(in crate::root) fn create_vault_from_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(store) = self.vault_store.as_ref() else {
            self.set_vault_error("Wallet vault storage is unavailable", cx);
            return;
        };
        let password = Self::read_and_clear_input(&self.new_password_input, window, cx);
        let confirm = Self::read_and_clear_input(&self.confirm_password_input, window, cx);

        if password.trim().is_empty() {
            self.set_vault_error("Enter a vault password to continue", cx);
            return;
        }
        if password.as_str() != confirm.as_str() {
            self.set_vault_error("Vault passwords do not match", cx);
            return;
        }

        match store.create_vault(password.as_str()) {
            Ok(created) => {
                Self::defer_wallet_name_input(PRIMARY_WALLET_LABEL.to_owned(), window, cx);
                self.vault_view_unlock = Some(Arc::new(created.view));
                self.setup_password = Some(password);
                self.vault_error = None;
                self.vault_state = VaultState::SetupWallet;
                self.wallet_setup_mode = WalletSetupMode::Choose;
                self.ensure_waku_started(cx);
                cx.notify();
            }
            Err(VaultError::VaultAlreadyExists) => {
                self.vault_state = VaultState::UnlockVault;
                self.focus_vault_input_on_render = true;
                self.set_vault_error("A wallet vault already exists. Unlock it to continue.", cx);
            }
            Err(error) => self.handle_vault_error(&error, cx),
        }
    }

    pub(in crate::root) fn unlock_vault_from_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.unlock_in_progress {
            return;
        }
        let Some(store) = self.vault_store.as_ref() else {
            self.set_vault_error("Wallet vault storage is unavailable", cx);
            return;
        };
        let password = Self::read_and_clear_input(&self.unlock_password_input, window, cx);
        if password.trim().is_empty() {
            self.set_vault_error("Enter the vault password to continue", cx);
            return;
        }

        let store = Arc::clone(store);
        let remembered_wallet_id = self.ui_state.last_wallet_id.clone();
        self.unlock_in_progress = true;
        self.vault_error = None;
        cx.notify();

        let join = self.runtime.spawn_blocking(move || {
            let view = store.unlock_view(password.as_str())?;
            let metadata = store.list_wallet_metadata_with_view_unlock(&view, true)?;
            let active = wallet_options_from_metadata(metadata.clone());
            let vault_view_unlock = Arc::new(view);
            if active.is_empty() {
                return Ok(VaultUnlockResult::new(
                    None,
                    metadata,
                    vault_view_unlock,
                    Some(password),
                ));
            }
            if let Some(wallet) = remembered_wallet_option(&active, remembered_wallet_id.as_deref())
                && wallet.source.is_hardware_derived()
            {
                #[cfg(feature = "hardware")]
                {
                    return Ok(VaultUnlockResult::remembered_hardware(
                        Arc::clone(&wallet.wallet_id),
                        metadata,
                        vault_view_unlock,
                    ));
                }
                #[cfg(not(feature = "hardware"))]
                {
                    tracing::warn!(
                        wallet_id = wallet.wallet_id.as_ref(),
                        "remembered hardware wallet cannot be opened in this build; falling back"
                    );
                }
            }
            let session = load_preferred_password_unlockable_wallet_session(
                &store,
                &vault_view_unlock,
                &active,
                remembered_wallet_id.as_deref(),
            )?;
            Ok(VaultUnlockResult::new(
                session,
                metadata,
                vault_view_unlock,
                None,
            ))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = join.await;
            let _ = this.update_in(cx, |root, window, cx| {
                root.unlock_in_progress = false;
                match result {
                    Ok(Ok(unlock)) if unlock.session.is_some() => {
                        root.vault_view_unlock = Some(unlock.vault_view_unlock);
                        root.enter_view_unlocked(
                            unlock.session.expect("checked above"),
                            unlock.metadata,
                            window,
                            cx,
                        );
                    }
                    Ok(Ok(unlock)) => {
                        root.enter_password_metadata_unlocked(
                            unlock.metadata,
                            unlock.vault_view_unlock,
                            unlock.setup_password,
                            window,
                            cx,
                        );
                        #[cfg(feature = "hardware")]
                        if let Some(wallet_id) = unlock.remembered_hardware_wallet_id {
                            root.vault_error = None;
                            root.open_hardware_profile_unlock_dialog_for_wallet(
                                wallet_id, window, cx,
                            );
                        }
                    }
                    Ok(Err(error)) => {
                        root.focus_vault_input_on_render = true;
                        root.handle_vault_error(&error, cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "desktop wallet vault unlock task failed");
                        root.focus_vault_input_on_render = true;
                        root.set_vault_error(
                            "Unlock failed. Check the password and try again.",
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    pub(in crate::root) fn choose_generated_wallet(&mut self, cx: &mut Context<'_, Self>) {
        match generate_seed_material() {
            Ok(seed) => {
                self.generated_seed = Some(seed);
                self.vault_error = None;
                self.wallet_setup_mode = WalletSetupMode::GeneratedReview;
                cx.notify();
            }
            Err(error) => self.handle_vault_error(&error, cx),
        }
    }

    pub(in crate::root) fn choose_import_wallet(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.generated_seed = None;
        self.vault_error = None;
        self.wallet_setup_mode = WalletSetupMode::Import;
        cx.notify();
        cx.defer_in(window, move |root, window, cx| {
            if root.wallet_setup_mode == WalletSetupMode::Import {
                root.import_mnemonic_input
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
            }
        });
    }

    #[cfg(feature = "hardware")]
    pub(in crate::root) fn choose_hardware_wallet(
        &mut self,
        device_kind: HardwareDeviceKind,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_hardware_profile_unlock_dialog_for_device(
            device_kind,
            HardwareProfileUnlockPurpose::Add,
            window,
            cx,
        );
    }

    #[cfg(not(feature = "hardware"))]
    pub(in crate::root) fn choose_hardware_wallet(
        &mut self,
        device_kind: HardwareDeviceKind,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.generated_seed = None;
        self.vault_error = None;
        self.wallet_setup_mode = WalletSetupMode::Hardware(device_kind);
        self.hardware_wallet_creation_intent = None;
        self.clear_hardware_wallet_restore_account_index(window, cx);
        cx.notify();
        cx.defer_in(window, move |root, window, cx| {
            if matches!(root.vault_state, VaultState::ViewUnlocked)
                && root.wallet_setup_mode == WalletSetupMode::Hardware(device_kind)
            {
                root.add_wallet_password_input
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
            }
        });
    }

    pub(in crate::root) fn submit_default_hardware_wallet_setup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let WalletSetupMode::Hardware(device_kind) = self.wallet_setup_mode else {
            return;
        };
        self.store_hardware_derived_wallet(
            device_kind,
            default_hardware_wallet_setup_intent(
                self.hardware_wallet_creation_intent,
                self.hardware_wallet_restore_account_index_set,
            ),
            window,
            cx,
        );
    }

    pub(in crate::root) fn back_to_wallet_setup_choice(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.generated_seed = None;
        self.import_mnemonic_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.vault_error = None;
        self.wallet_setup_mode = WalletSetupMode::Choose;
        self.hardware_wallet_creation_intent = None;
        self.clear_hardware_wallet_restore_account_index(window, cx);
        cx.notify();
    }

    pub(super) fn wallet_creation_password(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Zeroizing<String>> {
        if matches!(self.vault_state, VaultState::ViewUnlocked) {
            let password = Self::read_and_clear_input(&self.add_wallet_password_input, window, cx);
            if password.trim().is_empty() {
                self.set_vault_error("Enter the vault password to add a wallet", cx);
                return None;
            }
            return Some(password);
        }
        let Some(password) = self.setup_password.as_ref() else {
            self.set_vault_error("Unlock the wallet vault before adding a wallet", cx);
            return None;
        };
        Some(Zeroizing::new(password.to_string()))
    }

    #[cfg(feature = "hardware")]
    pub(super) fn hardware_wallet_creation_password(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> Option<Zeroizing<String>> {
        if matches!(self.vault_state, VaultState::ViewUnlocked) {
            let password =
                Zeroizing::new(self.add_wallet_password_input.read(cx).value().to_string());
            if password.trim().is_empty() {
                self.set_vault_error("Enter the vault password to add a wallet", cx);
                return None;
            }
            return Some(password);
        }
        let Some(password) = self.setup_password.as_ref() else {
            self.set_vault_error("Unlock the wallet vault before adding a wallet", cx);
            return None;
        };
        Some(Zeroizing::new(password.to_string()))
    }

    #[cfg(feature = "hardware")]
    #[allow(clippy::option_option)]
    pub(super) fn hardware_wallet_restore_account_index(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> Option<Option<u32>> {
        let value = self
            .hardware_wallet_restore_account_index_input
            .read(cx)
            .value()
            .to_string();
        match parse_hardware_wallet_restore_account_index(&value) {
            Ok(index) => Some(index),
            Err(message) => {
                self.set_vault_error(message, cx);
                None
            }
        }
    }

    pub(super) fn wallet_name_from_input(&self, cx: &Context<'_, Self>) -> String {
        self.wallet_name_input.read(cx).value().to_string()
    }

    pub(in crate::root) fn store_generated_wallet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let wallet_id = match generate_opaque_id() {
            Ok(wallet_id) => wallet_id,
            Err(error) => {
                self.handle_vault_error(&error, cx);
                return;
            }
        };
        let label = self.wallet_name_from_input(cx);
        let pending_create_new_chain_ids = self.enabled_chain_ids_for_created_wallet();
        let Some(password) = self.wallet_creation_password(window, cx) else {
            return;
        };
        let result = {
            let Some(store) = self.vault_store.as_ref() else {
                self.set_vault_error("Wallet vault storage is unavailable", cx);
                return;
            };
            let Some(seed) = self.generated_seed.as_ref() else {
                self.set_vault_error("Generate a recovery phrase before creating the wallet", cx);
                return;
            };
            let metadata = store.new_wallet_metadata_with_pending_create_new_chain_ids(
                password.as_str(),
                &wallet_id,
                0,
                WalletSource::Generated,
                &label,
                pending_create_new_chain_ids,
            );
            let metadata = match metadata {
                Ok(metadata) => metadata,
                Err(error) => return self.handle_vault_error(&error, cx),
            };
            store
                .store_generated_wallet_with_metadata(
                    password.as_str(),
                    &wallet_id,
                    0,
                    "english",
                    seed,
                    &metadata,
                )
                .and_then(|_| {
                    let metadata = store.list_wallet_metadata(password.as_str())?;
                    let session = store.load_view_session(password.as_str(), &wallet_id)?;
                    Ok((session, metadata))
                })
        };

        match result {
            Ok((session, metadata)) => {
                self.enter_new_wallet_view_unlocked(session, metadata, window, cx);
            }
            Err(error) => self.handle_vault_error(&error, cx),
        }
    }

    pub(in crate::root) fn store_imported_wallet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let mnemonic = Self::read_and_clear_input(&self.import_mnemonic_input, window, cx);
        if mnemonic.trim().is_empty() {
            self.set_vault_error("Paste a recovery phrase to import", cx);
            return;
        }
        let wallet_id = match generate_opaque_id() {
            Ok(wallet_id) => wallet_id,
            Err(error) => {
                self.handle_vault_error(&error, cx);
                return;
            }
        };
        let label = self.wallet_name_from_input(cx);
        let Some(password) = self.wallet_creation_password(window, cx) else {
            return;
        };

        let result = {
            let Some(store) = self.vault_store.as_ref() else {
                self.set_vault_error("Wallet vault storage is unavailable", cx);
                return;
            };
            let metadata = store.new_wallet_metadata(
                password.as_str(),
                &wallet_id,
                0,
                WalletSource::Imported,
                &label,
            );
            let metadata = match metadata {
                Ok(metadata) => metadata,
                Err(error) => return self.handle_vault_error(&error, cx),
            };
            store
                .import_wallet_mnemonic_with_metadata(
                    password.as_str(),
                    &wallet_id,
                    0,
                    "english",
                    mnemonic.as_str(),
                    &metadata,
                )
                .and_then(|_| {
                    let metadata = store.list_wallet_metadata(password.as_str())?;
                    let session = store.load_view_session(password.as_str(), &wallet_id)?;
                    Ok((session, metadata))
                })
        };

        match result {
            Ok((session, metadata)) => self.enter_view_unlocked(session, metadata, window, cx),
            Err(error) => self.handle_vault_error(&error, cx),
        }
    }

    #[cfg(not(feature = "hardware"))]
    pub(in crate::root) fn store_hardware_derived_wallet(
        &mut self,
        _device_kind: HardwareDeviceKind,
        sync_intent: HardwareWalletSyncIntent,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.hardware_wallet_creation_intent = Some(sync_intent);
        self.set_vault_error(
            "Hardware wallet support is not enabled in this build. Rebuild the wallet with the hardware feature to use Ledger-derived or Trezor-derived wallets.",
            cx,
        );
    }
}
