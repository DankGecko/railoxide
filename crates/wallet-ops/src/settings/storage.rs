use super::{
    DbStore, WALLET_SETTINGS_KEY, WALLET_SETTINGS_VERSION, WALLET_UI_STATE_KEY,
    WALLET_UI_STATE_VERSION, WalletSettings, WalletSettingsError, WalletUiState,
    WalletUiStateError,
};

pub fn load_wallet_settings(store: &DbStore) -> Result<WalletSettings, WalletSettingsError> {
    let Some(payload) = store.get_app_settings_record(WALLET_SETTINGS_KEY)? else {
        return Ok(WalletSettings::default());
    };
    let mut settings = decode_wallet_settings(&payload)?;
    if settings.poi.artifact.migrate_legacy_official_identity() {
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
    let settings: WalletSettings = rmp_serde::from_slice(data)?;
    if settings.version != WALLET_SETTINGS_VERSION {
        return Err(WalletSettingsError::UnsupportedVersion {
            version: settings.version,
        });
    }
    Ok(settings)
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
