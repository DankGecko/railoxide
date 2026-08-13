use super::{Arc, HardwareDeviceKind, WalletSource};

pub(in crate::root) enum VaultState {
    CreateVault,
    UnlockVault,
    SwitchingWallet,
    SetupWallet,
    PendingSoftwareProfileOpen,
    ViewUnlocked,
    Error(Arc<str>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::root) enum WalletSetupMode {
    Choose,
    GeneratedReview,
    Import,
    #[allow(dead_code)]
    Hardware(HardwareDeviceKind),
}

#[derive(Clone)]
pub(in crate::root) struct WalletOption {
    pub(in crate::root) wallet_id: Arc<str>,
    pub(in crate::root) source: WalletSource,
}
