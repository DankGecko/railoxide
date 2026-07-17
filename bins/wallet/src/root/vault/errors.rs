use super::{Arc, VaultError};

pub(in crate::root) const fn vault_error_kind(error: &VaultError) -> &'static str {
    match error {
        VaultError::Random => "random",
        VaultError::InvalidKdfParams => "invalid_kdf_params",
        VaultError::Kdf => "kdf",
        VaultError::KeySeparation => "key_separation",
        VaultError::Encrypt => "encrypt",
        VaultError::Decrypt => "decrypt",
        VaultError::WalletPrivateMigrationConflict { .. } => "wallet_private_migration_conflict",
        VaultError::Encode(_) => "encode",
        VaultError::Decode(_) => "decode",
        VaultError::Db(_) => "db",
        VaultError::InvalidWalletCacheKey(_) => "invalid_wallet_cache_key",
        VaultError::Io(_) => "io",
        VaultError::Key(_) => "key",
        VaultError::UnsupportedVersion(_) => "unsupported_version",
        VaultError::VaultAlreadyExists => "vault_already_exists",
        VaultError::VaultNotFound => "vault_not_found",
        VaultError::UnlockFailed => "unlock_failed",
        VaultError::InvalidSpendGrant => "invalid_spend_grant",
        VaultError::WalletNotFound => "wallet_not_found",
        VaultError::InvalidWalletLabel => "invalid_wallet_label",
        VaultError::DuplicateWalletLabel => "duplicate_wallet_label",
        VaultError::InvalidWalletOrder => "invalid_wallet_order",
        VaultError::LastActiveWallet => "last_active_wallet",
        VaultError::WalletDisplayOrderOverflow => "wallet_display_order_overflow",
        VaultError::PublicAccountNotFound => "public_account_not_found",
        VaultError::DuplicatePublicAccountAddress => "duplicate_public_account_address",
        VaultError::InvalidPublicAccountOperation => "invalid_public_account_operation",
        VaultError::PublicAccountDisplayOrderOverflow => "public_account_display_order_overflow",
        VaultError::InvalidPublicEvmPrivateKey => "invalid_public_evm_private_key",
        VaultError::PublicEvmKeyDerivation => "public_evm_key_derivation",
        VaultError::InvalidAddressBookLabel => "invalid_address_book_label",
        VaultError::InvalidPrivateAddressBookAddress => "invalid_private_address_book_address",
        VaultError::DuplicatePrivateAddressBookAddress => "duplicate_private_address_book_address",
        VaultError::PrivateAddressBookEntryNotFound => "private_address_book_entry_not_found",
        VaultError::PrivateAddressBookDisplayOrderOverflow => {
            "private_address_book_display_order_overflow"
        }
        VaultError::InvalidPublicAddressBookAddress => "invalid_public_address_book_address",
        VaultError::DuplicatePublicAddressBookAddress => "duplicate_public_address_book_address",
        VaultError::PublicAddressBookEntryNotFound => "public_address_book_entry_not_found",
        VaultError::PublicAddressBookDisplayOrderOverflow => {
            "public_address_book_display_order_overflow"
        }
        VaultError::InvalidBroadcasterPreferenceAddress => "invalid_broadcaster_preference_address",
        VaultError::InvalidHardwareWalletDescriptor => "invalid_hardware_wallet_descriptor",
        VaultError::HardwareWalletAccountIndexOverflow => "hardware_wallet_account_index_overflow",
        VaultError::DuplicateHardwareWalletAccountIndex => {
            "duplicate_hardware_wallet_account_index"
        }
        VaultError::HardwareWalletIdentityMismatch => "hardware_wallet_identity_mismatch",
        VaultError::HardwareWalletViewRequiresDevice => "hardware_wallet_view_requires_device",
        VaultError::WalletChainMetadataUnavailable => "wallet_chain_metadata_unavailable",
        VaultError::WalletMnemonicUnavailable => "wallet_mnemonic_unavailable",
        VaultError::UnsupportedHardwareCustodyBackend(_) => "unsupported_hardware_custody_backend",
        VaultError::InvalidHardwareAccountRecoveryRange => {
            "invalid_hardware_account_recovery_range"
        }
        VaultError::HardwareWalletReceiveAddress => "hardware_wallet_receive_address",
    }
}

pub(in crate::root) fn vault_error_message(error: &VaultError) -> Arc<str> {
    match error {
        VaultError::UnlockFailed => "Unlock failed. Check the password and try again.".into(),
        VaultError::Key(_) => "Invalid recovery phrase. Paste it again to retry.".into(),
        VaultError::VaultNotFound => {
            "Wallet vault not found. Create a new vault to continue.".into()
        }
        VaultError::InvalidWalletLabel => "Enter a wallet name before continuing.".into(),
        VaultError::DuplicateWalletLabel => {
            "A wallet with that name already exists. Choose a different wallet name.".into()
        }
        VaultError::DuplicateHardwareWalletAccountIndex => {
            "A hardware-derived wallet with that account index already exists. Choose a different restore index or unhide the existing wallet.".into()
        }
        VaultError::HardwareWalletIdentityMismatch => {
            "Hardware wallet identity mismatch. Check that the correct device, passphrase wallet, path, and account index are active, then try again.".into()
        }
        VaultError::HardwareWalletViewRequiresDevice => {
            "Connect the matching hardware wallet to view this account.".into()
        }
        VaultError::WalletChainMetadataUnavailable => {
            "Wallet sync metadata could not be verified. Open each wallet to finish migration, then retry.".into()
        }
        VaultError::WalletMnemonicUnavailable => {
            "Mnemonic seed export is unavailable for hardware-derived wallets.".into()
        }
        VaultError::UnsupportedHardwareCustodyBackend(_) => {
            "This hardware wallet custody backend is not supported by this app version.".into()
        }
        VaultError::InvalidHardwareAccountRecoveryRange => {
            "Enter a valid bounded hardware account recovery range.".into()
        }
        _ => "Wallet vault operation failed. See logs for non-sensitive diagnostics.".into(),
    }
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) fn hardware_setup_vault_error_message(
    error: &VaultError,
    label: &str,
) -> Arc<str> {
    match error {
        VaultError::DuplicateWalletLabel => format!(
            "A wallet named {:?} already exists. Choose a different wallet name.",
            label.trim()
        )
        .into(),
        VaultError::InvalidWalletLabel => "Enter a wallet name before continuing.".into(),
        _ => vault_error_message(error),
    }
}

#[cfg(feature = "hardware")]
#[derive(Clone, Copy)]
pub(in crate::root::vault) enum HardwareSetupErrorFocus {
    WalletName,
    VaultPassword,
}

#[cfg(feature = "hardware")]
pub(super) const fn hardware_setup_vault_error_preserves_password(error: &VaultError) -> bool {
    matches!(
        error,
        VaultError::InvalidWalletLabel | VaultError::DuplicateWalletLabel
    )
}

#[cfg(feature = "hardware")]
pub(super) const fn hardware_setup_vault_error_focus(
    error: &VaultError,
) -> HardwareSetupErrorFocus {
    match error {
        VaultError::InvalidWalletLabel | VaultError::DuplicateWalletLabel => {
            HardwareSetupErrorFocus::WalletName
        }
        _ => HardwareSetupErrorFocus::VaultPassword,
    }
}
