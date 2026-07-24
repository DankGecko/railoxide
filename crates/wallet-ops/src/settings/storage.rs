use super::{
    DEFAULT_AUTO_LOCK_TIMEOUT_SECS, DbStore, WALLET_SETTINGS_KEY, WALLET_SETTINGS_VERSION,
    WALLET_UI_STATE_KEY, WALLET_UI_STATE_VERSION, WalletSettings, WalletSettingsError,
    WalletUiState, WalletUiStateError,
};

/// Loads and migrates a supported settings record without requiring semantic validity.
/// Runtime consumers must validate the returned settings before using them.
pub fn load_wallet_settings(store: &DbStore) -> Result<WalletSettings, WalletSettingsError> {
    let Some(payload) = store.get_app_settings_record(WALLET_SETTINGS_KEY)? else {
        return Ok(WalletSettings::default());
    };
    let (mut settings, version_migrated) = decode_wallet_settings_with_migration(&payload)?;
    let identity_migrated = settings.poi.artifact.migrate_legacy_official_identity();
    if version_migrated || identity_migrated {
        let payload = encode_wallet_settings(&settings)?;
        store.put_app_settings_record(WALLET_SETTINGS_KEY, &payload)?;
    }
    Ok(settings)
}

pub fn save_wallet_settings(
    store: &DbStore,
    settings: &WalletSettings,
) -> Result<(), WalletSettingsError> {
    let mut settings = settings.clone();
    settings.version = WALLET_SETTINGS_VERSION;
    settings.validate()?;
    let payload = encode_wallet_settings(&settings)?;
    store.put_app_settings_record(WALLET_SETTINGS_KEY, &payload)?;
    Ok(())
}

pub fn delete_wallet_settings(store: &DbStore) -> Result<(), WalletSettingsError> {
    store.delete_app_settings_record(WALLET_SETTINGS_KEY)?;
    Ok(())
}

pub fn load_wallet_ui_state(store: &DbStore) -> Result<WalletUiState, WalletUiStateError> {
    let Some(payload) = store.get_app_settings_record(WALLET_UI_STATE_KEY)? else {
        return Ok(WalletUiState::default());
    };

    match decode_wallet_ui_state(&payload) {
        Ok(state) => Ok(state),
        Err(
            error @ (WalletUiStateError::Decode(_) | WalletUiStateError::UnsupportedVersion { .. }),
        ) => {
            tracing::warn!(%error, "ignoring invalid wallet UI state");
            Ok(WalletUiState::default())
        }
        Err(error) => Err(error),
    }
}

pub fn save_wallet_ui_state(
    store: &DbStore,
    state: &WalletUiState,
) -> Result<(), WalletUiStateError> {
    let payload = encode_wallet_ui_state(state)?;
    store.put_app_settings_record(WALLET_UI_STATE_KEY, &payload)?;
    Ok(())
}

pub fn encode_wallet_settings(settings: &WalletSettings) -> Result<Vec<u8>, WalletSettingsError> {
    let mut settings = settings.clone();
    settings.version = WALLET_SETTINGS_VERSION;
    Ok(rmp_serde::to_vec_named(&settings)?)
}

pub fn decode_wallet_settings(data: &[u8]) -> Result<WalletSettings, WalletSettingsError> {
    decode_wallet_settings_with_migration(data).map(|(settings, _migrated)| settings)
}

fn decode_wallet_settings_with_migration(
    data: &[u8],
) -> Result<(WalletSettings, bool), WalletSettingsError> {
    let mut settings: WalletSettings = rmp_serde::from_slice(data)?;
    let migrated = match settings.version {
        WALLET_SETTINGS_VERSION => false,
        1 => {
            settings.version = WALLET_SETTINGS_VERSION;
            settings.runtime.auto_lock_timeout_secs = Some(DEFAULT_AUTO_LOCK_TIMEOUT_SECS);
            true
        }
        version => return Err(WalletSettingsError::UnsupportedVersion { version }),
    };
    Ok((settings, migrated))
}

pub fn encode_wallet_ui_state(state: &WalletUiState) -> Result<Vec<u8>, WalletUiStateError> {
    let mut state = state.clone();
    state.version = WALLET_UI_STATE_VERSION;
    Ok(rmp_serde::to_vec_named(&state)?)
}

pub fn decode_wallet_ui_state(data: &[u8]) -> Result<WalletUiState, WalletUiStateError> {
    let state: WalletUiState = rmp_serde::from_slice(data)?;
    if state.version != WALLET_UI_STATE_VERSION {
        return Err(WalletUiStateError::UnsupportedVersion {
            version: state.version,
        });
    }
    Ok(state)
}
