use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use alloy::primitives::{Address, U256};
use alloy::rpc::types::TransactionRequest;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::TxReceiptOutput;
use crate::hardware::HardwareTypedDataSigningMode;
use crate::settings::EffectiveChainConfig;
use crate::vault::{
    DesktopVaultStore, DesktopViewSession, HardwareProfileSession, PublicAccountMetadata,
};

pub type PublicActionGasFeeQuote = crate::SelfBroadcastGasFeeQuote;
pub type PublicActionGasFeeSelection = crate::SelfBroadcastGasFeeSelection;
pub type PublicActionCommandKind = crate::SelfBroadcastCommandKind;
pub type PublicActionCommand = crate::SelfBroadcastCommand;
pub type PublicActionCommandSender = tokio::sync::mpsc::UnboundedSender<PublicActionCommand>;
pub type PublicActionCommandReceiver = tokio::sync::mpsc::UnboundedReceiver<PublicActionCommand>;
pub type PublicActionAttemptInfo = crate::SelfBroadcastAttemptInfo;
pub type PublicActionSessionEventSender =
    tokio::sync::mpsc::UnboundedSender<PublicActionSessionEvent>;
#[cfg(feature = "hardware")]
pub type HardwareTrezorPinMatrixProvider = crate::hardware::trezor::TrezorPinMatrixProvider;
#[cfg(not(feature = "hardware"))]
pub type HardwareTrezorPinMatrixProvider = ();

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PublicAssetId {
    Native,
    Erc20(Address),
}

impl PublicAssetId {
    #[must_use]
    pub const fn token_address(self) -> Option<Address> {
        match self {
            Self::Native => None,
            Self::Erc20(token) => Some(token),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicActionKind {
    Shield,
    Send,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBalanceAsset {
    pub id: PublicAssetId,
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicBalanceAmount {
    Available(U256),
    Unavailable,
}

impl PublicBalanceAmount {
    #[must_use]
    pub const fn amount(&self) -> Option<U256> {
        match self {
            Self::Available(amount) => Some(*amount),
            Self::Unavailable => None,
        }
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        matches!(self, Self::Available(amount) if amount.is_zero())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBalanceEntry {
    pub asset: PublicBalanceAsset,
    pub amount: PublicBalanceAmount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccountBalance {
    pub account: PublicAccountMetadata,
    pub balances: Vec<PublicBalanceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBalanceSnapshot {
    pub chain_id: u64,
    pub refreshed_at: SystemTime,
    pub accounts: Vec<PublicAccountBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedPublicBalanceCall {
    pub(crate) public_account_uuid: String,
    pub(crate) account: Address,
    pub(crate) asset: PublicBalanceAsset,
    pub(crate) target: Address,
    pub(crate) data: Vec<u8>,
}

#[derive(Default)]
pub struct PublicBalanceRefreshCoordinator {
    refreshing: Arc<AtomicBool>,
}

pub struct PublicBalanceRefreshGuard {
    refreshing: Arc<AtomicBool>,
}

impl PublicBalanceRefreshCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn try_begin(&self) -> Option<PublicBalanceRefreshGuard> {
        self.refreshing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| PublicBalanceRefreshGuard {
                refreshing: Arc::clone(&self.refreshing),
            })
    }

    #[must_use]
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::Acquire)
    }
}

impl Drop for PublicBalanceRefreshGuard {
    fn drop(&mut self) {
        self.refreshing.store(false, Ordering::Release);
    }
}

pub struct PublicSendRequest {
    pub chain_id: u64,
    pub effective_chain: Option<EffectiveChainConfig>,
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub vault_password: Zeroizing<String>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
    pub asset: PublicAssetId,
    pub amount: U256,
    pub recipient: Address,
    pub gas_fee: PublicActionGasFeeSelection,
    pub command_rx: Option<PublicActionCommandReceiver>,
    pub event_tx: Option<PublicActionSessionEventSender>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSendResult {
    pub tx: TxReceiptOutput,
}

pub struct PublicShieldRequest {
    pub chain_id: u64,
    pub effective_chain: Option<EffectiveChainConfig>,
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub vault_password: Zeroizing<String>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
    pub asset: PublicAssetId,
    pub amount: U256,
    pub gas_fee: PublicActionGasFeeSelection,
    pub command_rx: Option<PublicActionCommandReceiver>,
    pub event_tx: Option<PublicActionSessionEventSender>,
}

pub struct WalletConnectPersonalSignRequest {
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub vault_password: Zeroizing<String>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
    pub message: Vec<u8>,
    pub event_tx: Option<PublicActionSessionEventSender>,
}

pub struct WalletConnectTypedDataSignRequest {
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub vault_password: Zeroizing<String>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
    pub typed_data: Value,
    pub hash_fallback_confirmed: bool,
    pub event_tx: Option<PublicActionSessionEventSender>,
}

pub struct WalletConnectHardwareTypedDataHashFallbackConfirmationRequired {
    refreshed_hardware_session: Option<HardwareProfileSession>,
}

impl WalletConnectHardwareTypedDataHashFallbackConfirmationRequired {
    #[must_use]
    pub const fn new(refreshed_hardware_session: Option<HardwareProfileSession>) -> Self {
        Self {
            refreshed_hardware_session,
        }
    }

    #[must_use]
    pub fn refreshed_hardware_session(&self) -> Option<HardwareProfileSession> {
        self.refreshed_hardware_session.clone()
    }
}

impl fmt::Debug for WalletConnectHardwareTypedDataHashFallbackConfirmationRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WalletConnectHardwareTypedDataHashFallbackConfirmationRequired")
    }
}

impl fmt::Display for WalletConnectHardwareTypedDataHashFallbackConfirmationRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "WalletConnect hardware typed-data hash fallback requires confirmation before device approval",
        )
    }
}

impl std::error::Error for WalletConnectHardwareTypedDataHashFallbackConfirmationRequired {}

#[must_use]
pub fn walletconnect_hardware_typed_data_hash_fallback_confirmation_session(
    error: &eyre::Report,
) -> Option<HardwareProfileSession> {
    error
        .downcast_ref::<WalletConnectHardwareTypedDataHashFallbackConfirmationRequired>()
        .and_then(WalletConnectHardwareTypedDataHashFallbackConfirmationRequired::refreshed_hardware_session)
}

#[must_use]
pub fn is_walletconnect_hardware_typed_data_hash_fallback_confirmation_required(
    error: &eyre::Report,
) -> bool {
    error
        .downcast_ref::<WalletConnectHardwareTypedDataHashFallbackConfirmationRequired>()
        .is_some()
}

pub struct WalletConnectHardwareTypedDataCapabilityRequest {
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletConnectHardwareTypedDataCapabilityResult {
    pub mode: HardwareTypedDataSigningMode,
    pub refreshed_hardware_session: Option<HardwareProfileSession>,
}

pub struct WalletConnectSendTransactionRequest {
    pub chain_id: u64,
    pub effective_chain: Option<EffectiveChainConfig>,
    pub view_session: Arc<DesktopViewSession>,
    pub vault_store: Arc<DesktopVaultStore>,
    pub vault_password: Zeroizing<String>,
    pub trezor_app_passphrase: Option<Zeroizing<String>>,
    pub trezor_pin_matrix_provider: Option<HardwareTrezorPinMatrixProvider>,
    pub public_account_uuid: String,
    pub tx_req: TransactionRequest,
    pub gas_fee: PublicActionGasFeeSelection,
    pub expiry_timestamp: Option<u64>,
    pub event_tx: Option<PublicActionSessionEventSender>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletConnectSendTransactionResult {
    pub tx_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PublicActionProgressStep {
    ShieldKey,
    Send,
    Wrap,
    Approve,
    Shield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicActionProgressStatus {
    Pending,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicActionProgressUpdate {
    pub step: PublicActionProgressStep,
    pub status: PublicActionProgressStatus,
    pub tx_hash: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicActionSessionEvent {
    StepFailed {
        step: PublicActionProgressStep,
        message: String,
    },
    AttemptHandoff {
        step: PublicActionProgressStep,
    },
    AttemptSubmitted {
        step: PublicActionProgressStep,
        attempt: PublicActionAttemptInfo,
    },
    AttemptRejected {
        step: PublicActionProgressStep,
        message: String,
    },
    HardwareApprovalStarted,
    HardwareApprovalCompleted,
    HardwareApprovalFailed {
        message: String,
    },
    HardwareProfileSessionRefreshed {
        session: HardwareProfileSession,
    },
}
