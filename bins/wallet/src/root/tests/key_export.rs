use super::*;
use crate::root::key_export::{
    KeyExportSecretKind, WALLET_EXPORT_MENU_LABEL, key_export_context_warning,
    key_export_context_warning_predicates, key_export_copy_available, key_export_error_message,
    key_export_mnemonic_available, key_export_password_dialog_title,
    key_export_password_submit_button_id, key_export_reveal_button_id,
};

#[test]
fn key_export_menu_label_matches_requested_action() {
    assert_eq!(WALLET_EXPORT_MENU_LABEL, "Export keys");
}

#[test]
fn key_export_availability_matches_wallet_source() {
    assert!(key_export_mnemonic_available(WalletSource::Generated));
    assert!(key_export_mnemonic_available(WalletSource::Imported));
    assert!(!key_export_mnemonic_available(WalletSource::LedgerDerived));
    assert!(!key_export_mnemonic_available(WalletSource::TrezorDerived));
}

#[test]
fn key_export_copy_is_only_available_after_reveal() {
    assert!(!key_export_copy_available(false));
    assert!(key_export_copy_available(true));
}

#[test]
fn key_export_warning_is_context_specific() {
    let predicates = key_export_context_warning_predicates(
        Some(wallet_ops::vault::WalletSoftwareContextKind::Passphrase),
        Some("base-profile"),
    )
    .expect("passphrase context warning predicates");
    assert!(predicates.mnemonic_alone_is_insufficient);
    assert!(predicates.passphrase_is_external);
    assert!(predicates.passphrase_is_not_retained);
    let warning = key_export_context_warning(
        Some(wallet_ops::vault::WalletSoftwareContextKind::Passphrase),
        Some("base-profile"),
    )
    .expect("passphrase context warning");
    assert!(warning.contains("mnemonic alone cannot restore"));
    assert!(warning.contains("passphrase"));
    assert!(warning.contains("not retained"));
    assert!(
        key_export_context_warning_predicates(
            Some(wallet_ops::vault::WalletSoftwareContextKind::Passphrase),
            None,
        )
        .is_none()
    );
    assert!(
        key_export_context_warning(
            Some(wallet_ops::vault::WalletSoftwareContextKind::Standard),
            Some("base-profile"),
        )
        .is_none()
    );
}

#[test]
fn key_export_reveal_buttons_are_independent() {
    assert_eq!(
        key_export_reveal_button_id(KeyExportSecretKind::Mnemonic),
        "wallet-key-export-show-mnemonic"
    );
    assert_eq!(
        key_export_reveal_button_id(KeyExportSecretKind::ShareableViewingKey),
        "wallet-key-export-show-view-only"
    );
}

#[test]
fn key_export_password_dialog_labels_are_secret_specific() {
    assert_eq!(
        key_export_password_dialog_title(KeyExportSecretKind::Mnemonic),
        "Reveal mnemonic seed"
    );
    assert_eq!(
        key_export_password_dialog_title(KeyExportSecretKind::ShareableViewingKey),
        "Reveal view-only key"
    );
    assert_eq!(
        key_export_password_submit_button_id(KeyExportSecretKind::Mnemonic),
        "wallet-key-export-submit-mnemonic"
    );
    assert_eq!(
        key_export_password_submit_button_id(KeyExportSecretKind::ShareableViewingKey),
        "wallet-key-export-submit-view-only"
    );
}

#[test]
fn key_export_errors_are_actionable_and_redacted() {
    let hardware_message = key_export_error_message(
        &wallet_ops::vault::VaultError::HardwareWalletViewRequiresDevice,
        KeyExportSecretKind::ShareableViewingKey,
    );
    assert!(hardware_message.contains("Unlock the matching hardware wallet"));

    let mnemonic_message = key_export_error_message(
        &wallet_ops::vault::VaultError::WalletMnemonicUnavailable,
        KeyExportSecretKind::Mnemonic,
    );
    assert!(mnemonic_message.contains("Mnemonic seed export is unavailable"));
    assert!(!mnemonic_message.contains("entropy"));
    assert!(!hardware_message.contains("passphrase"));
}
