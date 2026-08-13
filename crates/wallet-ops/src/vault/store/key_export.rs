use super::{
    DesktopVaultStore, DesktopViewSession, HardwareRailgunAccountIdentity, KEY_LEN, Serialize,
    U256, VaultError, ViewingKeyData, WalletMetadataBundle, WalletSoftwareContextKind, Zeroizing,
    bip39_mnemonic_from_entropy, wallet_spend_record_key, wallet_view_record_key,
};

#[derive(Serialize)]
struct ShareableViewingKeyPayload<'a> {
    vpriv: &'a str,
    spub: &'a str,
}

const BABYJUB_SIGN_THRESHOLD_BE: [u8; KEY_LEN] = [
    0x18, 0x32, 0x27, 0x39, 0x70, 0x98, 0xd0, 0x14, 0xdc, 0x28, 0x22, 0xdb, 0x40, 0xc0, 0xac, 0x2e,
    0x94, 0x19, 0xf4, 0x24, 0x3c, 0xdc, 0xb8, 0x48, 0xa1, 0xf0, 0xfa, 0xc9, 0xf8, 0x00, 0x00, 0x00,
];

impl DesktopVaultStore {
    pub fn export_wallet_mnemonic(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<Zeroizing<String>, VaultError> {
        let view = self.unlock_view(password)?;
        self.export_wallet_mnemonic_for_context_with_view(&view, password, wallet_id)
    }

    pub fn export_wallet_mnemonic_for_context(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<Zeroizing<String>, VaultError> {
        let view = self.unlock_view(password)?;
        self.export_wallet_mnemonic_for_context_with_view(&view, password, wallet_id)
    }

    fn export_wallet_mnemonic_for_context_with_view(
        &self,
        view: &super::ViewUnlock,
        password: &str,
        wallet_id: &str,
    ) -> Result<Zeroizing<String>, VaultError> {
        let (_selected, base_profile_uuid) =
            self.resolve_software_export_context(view, wallet_id)?;

        let record = self
            .encrypted_record_optional(&wallet_spend_record_key(&base_profile_uuid))?
            .ok_or(VaultError::WalletMnemonicUnavailable)?;
        let mut grant = self.create_spend_grant(password)?;
        let spend_bundle = grant
            .take_spend_unlock()?
            .decrypt_spend_bundle(&base_profile_uuid, &record)?;
        Ok(Zeroizing::new(bip39_mnemonic_from_entropy(
            &spend_bundle.bip39_entropy,
        )?))
    }

    pub fn export_wallet_shareable_viewing_key(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<Zeroizing<String>, VaultError> {
        let bundle = self.load_view_bundle(password, wallet_id)?;
        shareable_viewing_key_from_parts(bundle.scan_keys(), bundle.spending_public_key())
    }

    pub fn export_wallet_shareable_viewing_key_for_context(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<Zeroizing<String>, VaultError> {
        let view = self.unlock_view(password)?;
        self.resolve_software_export_context(&view, wallet_id)?;
        let record = self.encrypted_record(&wallet_view_record_key(wallet_id))?;
        let bundle = view.decrypt_view_bundle(wallet_id, &record)?;
        shareable_viewing_key_from_parts(bundle.scan_keys(), bundle.spending_public_key())
    }

    pub fn export_hardware_wallet_shareable_viewing_key(
        &self,
        password: &str,
        wallet_id: &str,
        active_view_session: Option<&DesktopViewSession>,
    ) -> Result<Zeroizing<String>, VaultError> {
        let password_view = self.unlock_view(password)?;
        let metadata = self.load_wallet_metadata_with_view(&password_view, wallet_id)?;
        if !metadata.source.is_hardware_derived() {
            return Err(VaultError::HardwareWalletViewRequiresDevice);
        }
        let Some(account) = metadata.hardware_account.as_ref() else {
            return Err(VaultError::HardwareWalletViewRequiresDevice);
        };
        Self::ensure_supported_hardware_account(account)?;

        let Some(view_session) = active_view_session else {
            return Err(VaultError::HardwareWalletViewRequiresDevice);
        };
        if view_session.wallet_id() != wallet_id {
            return Err(VaultError::HardwareWalletIdentityMismatch);
        }
        let Some(hardware_session) = view_session.hardware_profile_session() else {
            return Err(VaultError::HardwareWalletViewRequiresDevice);
        };
        hardware_session.verify_account(account)?;

        let session_identity = HardwareRailgunAccountIdentity {
            spending_public_key: view_session
                .spending_public_key()
                .map(|value| value.to_be_bytes()),
            viewing_public_key: view_session.scan_keys().viewing_public_key,
        };
        if session_identity != account.account_identity {
            return Err(VaultError::HardwareWalletIdentityMismatch);
        }

        view_session.shareable_viewing_key()
    }
}

impl DesktopVaultStore {
    fn resolve_software_export_context(
        &self,
        view: &super::ViewUnlock,
        wallet_id: &str,
    ) -> Result<(WalletMetadataBundle, String), VaultError> {
        let selected = self.load_wallet_metadata_with_view(view, wallet_id)?;
        if selected.source.is_hardware_derived()
            || selected.hardware_descriptor.is_some()
            || selected.hardware_account.is_some()
        {
            return Err(VaultError::WalletMnemonicUnavailable);
        }
        let Some(context) = selected.software_context.as_ref() else {
            return Err(VaultError::InvalidWalletMetadata);
        };
        let base_profile_uuid = context.base_profile_uuid.clone();
        let base = self.load_wallet_metadata_with_view(view, &base_profile_uuid)?;
        let Some(base_context) = base.software_context.as_ref() else {
            return Err(VaultError::InvalidWalletMetadata);
        };
        let base_context_is_standard_profile = base_context.kind
            == WalletSoftwareContextKind::Standard
            && base_context.base_profile_uuid == base.wallet_uuid;
        if base.source.is_hardware_derived()
            || !base_context_is_standard_profile
            || (context.kind == WalletSoftwareContextKind::Standard
                && selected.wallet_uuid != base.wallet_uuid)
        {
            return Err(VaultError::InvalidWalletMetadata);
        }
        Ok((selected, base_profile_uuid))
    }
}

impl DesktopViewSession {
    pub fn shareable_viewing_key(&self) -> Result<Zeroizing<String>, VaultError> {
        shareable_viewing_key_from_parts(self.scan_keys(), self.spending_public_key())
    }
}

pub(super) fn shareable_viewing_key_from_parts(
    viewing_key_data: ViewingKeyData,
    spending_public_key: [U256; 2],
) -> Result<Zeroizing<String>, VaultError> {
    let vpriv = Zeroizing::new(alloy::hex::encode(viewing_key_data.viewing_private_key));
    let spub = Zeroizing::new(alloy::hex::encode(pack_spending_public_key(
        spending_public_key,
    )));
    let payload = ShareableViewingKeyPayload {
        vpriv: &vpriv,
        spub: &spub,
    };
    let encoded = Zeroizing::new(rmp_serde::to_vec_named(&payload)?);
    Ok(Zeroizing::new(alloy::hex::encode(&encoded)))
}

fn pack_spending_public_key(spending_public_key: [U256; 2]) -> [u8; KEY_LEN] {
    let mut packed = spending_public_key[1].to_le_bytes::<KEY_LEN>();
    if spending_public_key[0] > U256::from_be_bytes(BABYJUB_SIGN_THRESHOLD_BE) {
        packed[KEY_LEN - 1] |= 0x80;
    }
    packed
}
