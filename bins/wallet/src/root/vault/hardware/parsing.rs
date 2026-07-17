#[cfg(not(feature = "hardware"))]
use super::HardwareWalletSyncIntent;
#[cfg(feature = "hardware")]
use super::{
    Address, HardwareProfileBindingKind, HardwareProfileSession, HardwareWalletSyncIntent,
};

#[cfg(any(feature = "hardware", test))]
use wallet_ops::hardware::HARDENED_BIP32_INDEX;
#[cfg(any(feature = "hardware", test))]
use wallet_ops::vault::TrezorPassphraseMode;

#[cfg(feature = "hardware")]
const TREZOR_APP_PASSPHRASE_REQUIRED_ERROR_TEXT: &str =
    "Trezor requested an app-entered passphrase but none was provided";

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const fn hardware_session_needs_trezor_app_passphrase(
    session: &wallet_ops::vault::HardwareProfileSession,
) -> bool {
    session.uses_trezor_app_passphrase() && session.trezor_session_id.is_none()
}

#[cfg(feature = "hardware")]
fn trezor_app_passphrase_required_error_message(message: &str) -> bool {
    message.contains(TREZOR_APP_PASSPHRASE_REQUIRED_ERROR_TEXT)
}

#[cfg(feature = "hardware")]
pub(in crate::root) fn trezor_session_stale_error_message(message: &str) -> bool {
    trezor_app_passphrase_required_error_message(message)
        || message.contains("derived hardware wallet key does not match the stored wallet")
        || message.contains("Hardware wallet identity mismatch")
        || message.contains("hardware wallet identity mismatch")
        || message.contains("hardware public signer profile mismatch")
        || message.contains("hardware public account identity mismatch")
        || message.contains("wrong hardware device or passphrase context is active")
}

#[cfg(feature = "hardware")]
pub(in crate::root) fn hardware_profile_evm_address_for_session(
    session: Option<&HardwareProfileSession>,
) -> Option<String> {
    let session = session?;
    if session.binding.kind != HardwareProfileBindingKind::EvmAddressFingerprint {
        return None;
    }
    let prefix = format!("{}:evm:", session.device_kind.as_str());
    let address = session.binding.fingerprint.strip_prefix(&prefix)?;
    let parsed: Address = address.parse().ok()?;
    Some(format!("{parsed:#x}"))
}

pub(in crate::root) const fn default_hardware_wallet_setup_intent(
    retry_intent: Option<HardwareWalletSyncIntent>,
    restore_account_index_set: bool,
) -> HardwareWalletSyncIntent {
    if restore_account_index_set {
        return HardwareWalletSyncIntent::RecoverExisting;
    }
    match retry_intent {
        Some(intent) => intent,
        None => HardwareWalletSyncIntent::CreateNew,
    }
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) fn parse_hardware_wallet_restore_account_index(
    value: &str,
) -> Result<Option<u32>, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let index = value.parse::<u32>().map_err(
        |_| "Enter a valid Railgun account index to restore, or leave the restore index blank.",
    )?;
    if index >= HARDENED_BIP32_INDEX {
        return Err(
            "Enter a Railgun account index below 2147483648 to restore, or leave the restore index blank.",
        );
    }
    Ok(Some(index))
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const fn hardware_profile_label_warning() -> &'static str {
    "Profile labels are saved as non-secret metadata. Do not put your hardware passphrase or passphrase fragments in the label."
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const HARDWARE_PROFILE_ADD_SUBACCOUNT_BUTTON_ID: &str =
    "hardware-profile-add-subaccount";
#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const HARDWARE_PROFILE_RECOVER_EXACT_BUTTON_ID: &str =
    "hardware-profile-recover-exact";
#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const HARDWARE_PROFILE_RECOVER_RANGE_BUTTON_ID: &str =
    "hardware-profile-recover-range";

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const fn trezor_passphrase_mode_copy(
    mode: wallet_ops::vault::TrezorPassphraseMode,
) -> &'static str {
    match mode {
        wallet_ops::vault::TrezorPassphraseMode::NoPassphrase => {
            "Use the standard Trezor wallet. If your Trezor asks on-device, leave the passphrase blank."
        }
        wallet_ops::vault::TrezorPassphraseMode::EnterOnTrezor => {
            "Use a hidden wallet passphrase entered on your Trezor. The app stores only the live Trezor session id."
        }
        wallet_ops::vault::TrezorPassphraseMode::EnterInApp => {
            "Enter in app sends the passphrase to this Trezor request once, then clears it immediately. It is never saved."
        }
    }
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) const fn effective_trezor_passphrase_mode(
    mode: TrezorPassphraseMode,
    passphrase_always_on_device: bool,
) -> TrezorPassphraseMode {
    if passphrase_always_on_device && matches!(mode, TrezorPassphraseMode::EnterInApp) {
        TrezorPassphraseMode::NoPassphrase
    } else {
        mode
    }
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) fn parse_hardware_recovery_range(
    start_value: &str,
    count_value: &str,
) -> Result<Vec<u32>, &'static str> {
    let start = parse_hardware_recovery_index(start_value, "Enter a valid start account index.")?;
    let count = count_value
        .trim()
        .parse::<u32>()
        .map_err(|_| "Enter a valid recovery count.")?;
    if count == 0 {
        return Err("Enter a recovery count above zero.");
    }
    if count > wallet_ops::vault::MAX_HARDWARE_RECOVERY_RANGE_COUNT {
        return Err("Enter a recovery count no greater than 255.");
    }
    let end = start
        .checked_add(count)
        .ok_or("Enter a bounded recovery range below 2147483648.")?;
    if end > HARDENED_BIP32_INDEX {
        return Err("Enter a bounded recovery range below 2147483648.");
    }
    Ok((start..end).collect())
}

#[cfg(any(feature = "hardware", test))]
pub(in crate::root) fn parse_hardware_exact_recovery_index(
    value: &str,
) -> Result<u32, &'static str> {
    parse_hardware_recovery_index(value, "Enter a valid account index to recover.")
}

#[cfg(any(feature = "hardware", test))]
fn parse_hardware_recovery_index(value: &str, error: &'static str) -> Result<u32, &'static str> {
    let index = value.trim().parse::<u32>().map_err(|_| error)?;
    if index >= HARDENED_BIP32_INDEX {
        return Err("Enter a Railgun account index below 2147483648.");
    }
    Ok(index)
}
