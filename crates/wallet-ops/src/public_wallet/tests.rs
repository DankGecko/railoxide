use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use alloy::primitives::{Bytes, TxKind, U256, address};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use alloy::uint;
use eyre::eyre;
use local_db::{DbConfig, DbStore};
use zeroize::Zeroizing;

use super::types::PlannedPublicBalanceCall;
use super::*;
use crate::hardware::{
    ConfirmedHardwarePublicAccount, HardwareDerivationDescriptor, HardwareDeviceKind,
    HardwareOperationOutput, HardwarePublicAccountDescriptor, HardwareTypedDataSigningMode,
    HardwareWalletSyncIntent, hardware_view_access_key_from_hardware_output, parse_bip32_path,
    synthetic_entropy_from_hardware_output,
};
use crate::hardware_typed_data::HardwareEip712Model;
use crate::settings::{EffectiveChainConfig, EffectiveChainGasSettings};
use crate::signer::SoftwareEvmSigner;
use crate::vault::{
    DesktopVaultStore, DesktopViewSession, HardwareProfileBinding, HardwareProfileSession,
    KdfParams, PublicAccountMetadata, PublicAccountScope, PublicAccountSource, PublicAccountStatus,
    TrezorPassphraseMode, VaultError, WalletSource,
};
use crate::{GAS_LIMIT_BUFFER, HttpContext, SelfBroadcastTipFallback};

const TEST_PASSWORD: &str = "correct horse battery staple";
const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const TEST_IMPORTED_PRIVATE_KEY: &str =
    "0x59c6995e998f97a5a0044966f0945387e7d5e4a4dbd4b3f1b530b87d9b4a5c2f";
static TEMP_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn public_action_attempt_errors_distinguish_signing_from_retryable_sending() {
    let signing = PublicActionAttemptError::Signing(eyre!("user rejected on device"));
    let sending = PublicActionAttemptError::Sending(eyre!("rpc rejected transaction"));

    assert!(matches!(signing, PublicActionAttemptError::Signing(_)));
    assert!(matches!(sending, PublicActionAttemptError::Sending(_)));
}

#[test]
fn public_action_pre_broadcast_checkpoint_yields_for_abort() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    runtime.block_on(async {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = ready_tx.send(());
            public_action_before_raw_broadcast_checkpoint().await;
            true
        });

        ready_rx.await.expect("checkpoint task started");
        task.abort();
        let error = task.await.expect_err("checkpoint task should abort");
        assert!(error.is_cancelled());
    });
}

#[test]
fn walletconnect_send_rejects_expired_request_before_raw_broadcast() {
    assert!(ensure_public_action_broadcast_not_expired(None, "walletconnect").is_ok());
    assert!(
        ensure_public_action_broadcast_not_expired(
            Some(public_action_current_unix_seconds() + 60),
            "walletconnect",
        )
        .is_ok()
    );

    let error = ensure_public_action_broadcast_not_expired(
        Some(public_action_current_unix_seconds()),
        "walletconnect",
    )
    .expect_err("expired request");

    assert!(
        error
            .to_string()
            .contains("request expired before transaction broadcast")
    );
}

fn test_kdf() -> KdfParams {
    KdfParams::new(1024, 1, 1)
}

fn temp_db_root() -> PathBuf {
    let dir = std::env::temp_dir().join("railoxide-public-wallet-tests");
    fs::create_dir_all(&dir).expect("create temp db dir");
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let counter = TEMP_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("db-{pid}-{nanos}-{counter}"))
}

fn public_action_request_parts() -> (
    PathBuf,
    Arc<DbStore>,
    Arc<DesktopVaultStore>,
    Arc<DesktopViewSession>,
) {
    let root_dir = temp_db_root();
    let db = Arc::new(
        DbStore::open(DbConfig {
            root_dir: root_dir.clone(),
        })
        .expect("open db"),
    );
    let store = Arc::new(DesktopVaultStore::from_db(Arc::clone(&db)));
    let _created = store
        .create_vault_with_params(TEST_PASSWORD, test_kdf())
        .expect("create vault");
    let wallet_id = "public-action-wallet";
    let metadata = store
        .new_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            0,
            WalletSource::Imported,
            "Public action",
        )
        .expect("wallet metadata");
    store
        .import_wallet_mnemonic_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            0,
            "english",
            TEST_MNEMONIC,
            &metadata,
        )
        .expect("import wallet");
    let view_session = Arc::new(
        store
            .load_view_session(TEST_PASSWORD, wallet_id)
            .expect("view session"),
    );
    (root_dir, db, store, view_session)
}

#[test]
fn balance_plan_batches_native_and_known_tokens_per_account() {
    let account = PublicAccountMetadata {
        public_account_uuid: "public-1".to_string(),
        address: address!("0x1111111111111111111111111111111111111111"),
        label: None,
        source: PublicAccountSource::Derived,
        scope: PublicAccountScope::PrivateWallet {
            wallet_uuid: "wallet-1".to_string(),
        },
        derivation_index: Some(0),
        hardware_descriptor: None,
        status: PublicAccountStatus::Active,
        display_order: 0,
    };
    let multicall = address!("0xcA11bde05977b3631167028862bE2a173976CA11");
    let calls = plan_public_balance_calls(1, multicall, &[account], None);

    assert_eq!(calls.first().expect("native call").target, multicall);
    assert_eq!(
        calls.first().expect("native call").asset.id,
        PublicAssetId::Native
    );
    assert!(
        calls
            .iter()
            .any(|call| matches!(call.asset.id, PublicAssetId::Erc20(_)))
    );
}

#[test]
fn walletconnect_personal_sign_uses_spend_authorized_public_signer() {
    let (root_dir, db, store, view_session) = public_action_request_parts();
    let account = store
        .import_public_account(
            TEST_PASSWORD,
            &view_session,
            TEST_IMPORTED_PRIVATE_KEY,
            Some("WalletConnect signer"),
            false,
        )
        .expect("import public account");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let denied = runtime.block_on(walletconnect_sign_personal_message(
        WalletConnectPersonalSignRequest {
            view_session: Arc::clone(&view_session),
            vault_store: Arc::clone(&store),
            vault_password: Zeroizing::new("wrong password".to_owned()),
            trezor_app_passphrase: None,
            trezor_pin_matrix_provider: None,
            public_account_uuid: account.public_account_uuid.clone(),
            message: b"hello".to_vec(),
            event_tx: None,
        },
    ));
    assert!(denied.is_err());

    let signature = runtime
        .block_on(walletconnect_sign_personal_message(
            WalletConnectPersonalSignRequest {
                view_session: Arc::clone(&view_session),
                vault_store: Arc::clone(&store),
                vault_password: Zeroizing::new(TEST_PASSWORD.to_owned()),
                trezor_app_passphrase: None,
                trezor_pin_matrix_provider: None,
                public_account_uuid: account.public_account_uuid,
                message: b"hello".to_vec(),
                event_tx: None,
            },
        ))
        .expect("personal sign");

    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 132);

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn walletconnect_typed_data_signs_for_software_public_account() {
    let (root_dir, db, store, view_session) = public_action_request_parts();
    let account = store
        .import_public_account(
            TEST_PASSWORD,
            &view_session,
            TEST_IMPORTED_PRIVATE_KEY,
            Some("WalletConnect typed data"),
            false,
        )
        .expect("import public account");
    let typed_data = serde_json::json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "Message": [
                { "name": "contents", "type": "string" }
            ]
        },
        "primaryType": "Message",
        "domain": {
            "name": "RailOxide",
            "version": "1",
            "chainId": 1
        },
        "message": {
            "contents": "hello"
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let signature = runtime
        .block_on(walletconnect_sign_typed_data_v4(
            WalletConnectTypedDataSignRequest {
                view_session: Arc::clone(&view_session),
                vault_store: Arc::clone(&store),
                vault_password: Zeroizing::new(TEST_PASSWORD.to_owned()),
                trezor_app_passphrase: None,
                trezor_pin_matrix_provider: None,
                public_account_uuid: account.public_account_uuid,
                typed_data,
                hash_fallback_confirmed: false,
                event_tx: None,
            },
        ))
        .expect("typed-data sign");

    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 132);

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn walletconnect_typed_data_signs_primitive_prefixed_custom_types_for_software_public_account() {
    let (root_dir, db, store, view_session) = public_action_request_parts();
    let account = store
        .import_public_account(
            TEST_PASSWORD,
            &view_session,
            TEST_IMPORTED_PRIVATE_KEY,
            Some("WalletConnect custom typed data"),
            false,
        )
        .expect("import public account");
    let typed_data = serde_json::json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "bytesPayload": [
                { "name": "digest", "type": "bytes32" }
            ],
            "Message": [
                { "name": "payload", "type": "bytesPayload" }
            ]
        },
        "primaryType": "Message",
        "domain": {
            "name": "RailOxide",
            "chainId": 1
        },
        "message": {
            "payload": {
                "digest": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let signature = runtime
        .block_on(walletconnect_sign_typed_data_v4(
            WalletConnectTypedDataSignRequest {
                view_session: Arc::clone(&view_session),
                vault_store: Arc::clone(&store),
                vault_password: Zeroizing::new(TEST_PASSWORD.to_owned()),
                trezor_app_passphrase: None,
                trezor_pin_matrix_provider: None,
                public_account_uuid: account.public_account_uuid,
                typed_data,
                hash_fallback_confirmed: false,
                event_tx: None,
            },
        ))
        .expect("typed-data sign");

    assert!(signature.starts_with("0x"));
    assert_eq!(signature.len(), 132);

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn hardware_typed_data_hash_fallback_confirmation_error_survives_context() {
    let session = HardwareProfileSession::matched(
        HardwareDeviceKind::Ledger,
        "profile-a",
        HardwareProfileBinding::evm_address_fingerprint("fingerprint-a"),
        None,
    );
    let error = eyre::Report::from(
        WalletConnectHardwareTypedDataHashFallbackConfirmationRequired::new(Some(session.clone())),
    )
    .wrap_err("WalletConnect eth_signTypedData_v4");

    assert!(is_walletconnect_hardware_typed_data_hash_fallback_confirmation_required(&error));
    assert_eq!(
        walletconnect_hardware_typed_data_hash_fallback_confirmation_session(&error),
        Some(session)
    );
    assert_eq!(
        format!(
            "{:?}",
            error
                .downcast_ref::<WalletConnectHardwareTypedDataHashFallbackConfirmationRequired>()
                .expect("confirmation error")
        ),
        "WalletConnectHardwareTypedDataHashFallbackConfirmationRequired"
    );
}

fn hardware_typed_data_signer_with_mode(
    mode: HardwareTypedDataSigningMode,
) -> HardwarePublicEvmSigner {
    let descriptor =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Ledger, 0, 0)
            .expect("ledger descriptor");
    let mut hardware_session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Ledger,
        HardwareProfileBinding::evm_address_fingerprint(
            "ledger:evm:0x1111111111111111111111111111111111111111",
        ),
        None,
    );
    hardware_session
        .cache_typed_data_signing_mode(&descriptor, mode)
        .expect("cache typed-data mode");
    HardwarePublicEvmSigner {
        address: address!("0x1111111111111111111111111111111111111111"),
        descriptor,
        hardware_session: std::sync::Mutex::new(hardware_session),
        trezor_app_passphrase: std::sync::Mutex::new(None),
        trezor_pin_matrix_provider: None,
    }
}

fn hardware_typed_data_model_for_tests() -> HardwareEip712Model {
    HardwareEip712Model::from_walletconnect_typed_data_json(serde_json::json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "Message": [
                { "name": "contents", "type": "string" }
            ]
        },
        "primaryType": "Message",
        "domain": {
            "name": "RailOxide",
            "chainId": 1
        },
        "message": {
            "contents": "hello"
        }
    }))
    .expect("typed-data model")
}

#[test]
fn hardware_public_signer_requires_hash_fallback_confirmation_before_signing() {
    let signer =
        hardware_typed_data_signer_with_mode(HardwareTypedDataSigningMode::Eip712HashFallback);
    let model = hardware_typed_data_model_for_tests();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let error = runtime
        .block_on(signer.sign_typed_data_v4(
            &model,
            Some(HardwareTypedDataSigningMode::Eip712HashFallback),
            false,
        ))
        .expect_err("fallback confirmation required");

    assert!(is_walletconnect_hardware_typed_data_hash_fallback_confirmation_required(&error));
    assert_eq!(
        walletconnect_hardware_typed_data_hash_fallback_confirmation_session(&error)
            .and_then(|session| session.typed_data_signing_mode(&signer.descriptor)),
        Some(HardwareTypedDataSigningMode::Eip712HashFallback)
    );
}

#[cfg(not(feature = "hardware"))]
#[test]
fn hardware_public_signer_rejects_confirmed_hash_fallback_without_hardware_feature() {
    let signer =
        hardware_typed_data_signer_with_mode(HardwareTypedDataSigningMode::Eip712HashFallback);
    let model = hardware_typed_data_model_for_tests();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let error = runtime
        .block_on(signer.sign_typed_data_v4(
            &model,
            Some(HardwareTypedDataSigningMode::Eip712HashFallback),
            true,
        ))
        .expect_err("default build cannot sign fallback");

    assert!(
        error
            .to_string()
            .contains("hardware public signing is not enabled in this build")
    );
}

#[test]
fn hardware_typed_data_signature_recovery_mismatch_rejects() {
    let model = hardware_typed_data_model_for_tests();
    let signer = SoftwareEvmSigner::from_private_key([7u8; 32]).expect("software signer");
    let signature = signer
        .sign_typed_data_v4(model.typed_data())
        .expect("typed-data signature");

    let error = verify_hardware_typed_data_signature_address(
        address!("0x1111111111111111111111111111111111111111"),
        &signature,
        &model,
    )
    .expect_err("recovery mismatch");

    assert!(
        error
            .to_string()
            .contains("hardware public signer address mismatch")
    );
}

#[test]
fn balance_assets_use_effective_token_registry_overlays() {
    let mut settings = crate::settings::WalletSettings::default();
    settings
        .tokens
        .built_in_tombstones
        .push(crate::settings::TokenKey {
            chain_id: 1,
            token_address: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_string(),
        });
    settings
        .tokens
        .custom_tokens
        .push(crate::settings::CustomTokenSettings {
            chain_id: 1,
            token_address: "0x0000000000000000000000000000000000000002".to_string(),
            symbol: "CSTM".to_string(),
            decimals: 9,
            icon_path: None,
            price_anchor: None,
        });
    let registry = crate::settings::build_effective_token_registry(&settings)
        .expect("effective token registry");

    let assets = public_balance_assets_for_chain_with_registry(1, Some(&registry));

    assert!(assets.iter().any(|asset| asset.id == PublicAssetId::Native));
    assert!(!assets.iter().any(|asset| {
        asset.id == PublicAssetId::Erc20(address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"))
    }));
    let custom = assets
        .iter()
        .find(|asset| {
            asset.id == PublicAssetId::Erc20(address!("0x0000000000000000000000000000000000000002"))
        })
        .expect("custom token asset");
    assert_eq!(custom.symbol, "CSTM");
    assert_eq!(custom.decimals, 9);
}

#[test]
fn balance_snapshot_preserves_partial_success() {
    let account = PublicAccountMetadata {
        public_account_uuid: "public-1".to_string(),
        address: address!("0x1111111111111111111111111111111111111111"),
        label: None,
        source: PublicAccountSource::Derived,
        scope: PublicAccountScope::PrivateWallet {
            wallet_uuid: "wallet-1".to_string(),
        },
        derivation_index: Some(0),
        hardware_descriptor: None,
        status: PublicAccountStatus::Active,
        display_order: 0,
    };
    let planned = vec![
        PlannedPublicBalanceCall {
            public_account_uuid: account.public_account_uuid.clone(),
            account: account.address,
            asset: PublicBalanceAsset {
                id: PublicAssetId::Native,
                symbol: "ETH".to_string(),
                decimals: 18,
            },
            target: address!("0xcA11bde05977b3631167028862bE2a173976CA11"),
            data: Vec::new(),
        },
        PlannedPublicBalanceCall {
            public_account_uuid: account.public_account_uuid.clone(),
            account: account.address,
            asset: PublicBalanceAsset {
                id: PublicAssetId::Erc20(address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")),
                symbol: "WETH".to_string(),
                decimals: 18,
            },
            target: address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            data: Vec::new(),
        },
    ];

    let snapshot = public_balance_snapshot_from_results(
        1,
        &[account],
        &planned,
        vec![Some(U256::from(7_u64)), None],
    );

    let balances = &snapshot.accounts[0].balances;
    assert_eq!(balances[0].amount.amount(), Some(U256::from(7_u64)));
    assert!(matches!(
        balances[1].amount,
        PublicBalanceAmount::Unavailable
    ));
}

#[test]
fn refresh_coordinator_prevents_overlap_and_releases() {
    let coordinator = PublicBalanceRefreshCoordinator::new();
    let guard = coordinator.try_begin().expect("first refresh guard");

    assert!(coordinator.is_refreshing());
    assert!(coordinator.try_begin().is_none());
    drop(guard);
    assert!(!coordinator.is_refreshing());
    assert!(coordinator.try_begin().is_some());
}

#[test]
fn public_native_action_gas_reserve_uses_buffered_units() {
    let send_steps = [PublicActionProgressStep::Send];
    assert_eq!(
        public_native_action_gas_units(&send_steps),
        PUBLIC_NATIVE_SEND_GAS_UNITS + GAS_LIMIT_BUFFER,
    );
    assert_eq!(
        public_native_action_gas_reserve(2, &send_steps),
        U256::from((PUBLIC_NATIVE_SEND_GAS_UNITS + GAS_LIMIT_BUFFER) * 2),
    );

    let shield_steps = [
        PublicActionProgressStep::ShieldKey,
        PublicActionProgressStep::Shield,
    ];
    assert_eq!(
        public_native_action_gas_units(&shield_steps),
        PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS + GAS_LIMIT_BUFFER,
    );
    assert_eq!(
        public_native_action_gas_units_with_buffer(&send_steps, 7),
        PUBLIC_NATIVE_SEND_GAS_UNITS + 7,
    );
}

#[test]
fn public_action_gas_cost_uses_asset_specific_buffered_units() {
    let token = address!("0x3333333333333333333333333333333333333333");
    let quote = PublicActionGasFeeQuote {
        rpc_gas_price: 2,
        suggested_max_fee_per_gas: 3,
        suggested_max_priority_fee_per_gas: 1,
    };

    let native_send = estimate_public_action_gas_cost(
        1,
        None,
        PublicActionKind::Send,
        PublicAssetId::Native,
        PublicActionGasFeeSelection::Auto,
        Some(quote),
    )
    .expect("native send estimate");
    assert_eq!(
        native_send,
        U256::from((PUBLIC_NATIVE_SEND_GAS_UNITS + GAS_LIMIT_BUFFER) * 3)
    );

    let erc20_send = estimate_public_action_gas_cost(
        1,
        None,
        PublicActionKind::Send,
        PublicAssetId::Erc20(token),
        PublicActionGasFeeSelection::Auto,
        Some(quote),
    )
    .expect("erc20 send estimate");
    assert_eq!(erc20_send, U256::from((65_000 + GAS_LIMIT_BUFFER) * 3));

    let native_shield = estimate_public_action_gas_cost(
        1,
        None,
        PublicActionKind::Shield,
        PublicAssetId::Native,
        PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
        },
        None,
    )
    .expect("native shield estimate");
    assert_eq!(
        native_shield,
        U256::from(PUBLIC_NATIVE_RELAY_ADAPT_SHIELD_GAS_UNITS + GAS_LIMIT_BUFFER)
    );
    let erc20_shield = estimate_public_action_gas_cost(
        1,
        None,
        PublicActionKind::Shield,
        PublicAssetId::Erc20(token),
        PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
        },
        None,
    )
    .expect("erc20 shield estimate");
    assert_eq!(
        erc20_shield,
        U256::from(
            PUBLIC_NATIVE_APPROVE_GAS_UNITS
                + PUBLIC_NATIVE_SHIELD_GAS_UNITS
                + (2 * GAS_LIMIT_BUFFER),
        )
    );
}

#[test]
fn public_shield_protocol_fee_amount_uses_floor_rounding() {
    assert_eq!(
        public_shield_protocol_fee_amount(U256::from(12_345_u64)),
        U256::from(30_u64)
    );
}

#[test]
fn effective_public_chain_config_uses_settings_overrides() {
    let defaults = chain_defaults_for_public_chain(1).expect("ethereum defaults");
    let effective = EffectiveChainConfig {
        chain_id: 1,
        enabled: true,
        rpc_endpoints: vec!["https://rpc.example".to_string()],
        archive_rpc_url: None,
        quick_sync_enabled: true,
        quick_sync_endpoint: defaults.quick_sync_endpoint.map(|url| url.to_string()),
        indexed_artifact_source_mode: crate::settings::IndexedArtifactSourceModeSetting::Disabled,
        indexed_artifact_source: None,
        indexed_wallet_block_range: defaults.indexed_wallet_block_range,
        deployment_block: defaults.deployment_block,
        v2_start_block: defaults.v2_start_block,
        legacy_shield_block: defaults.legacy_shield_block,
        archive_until_block: defaults.archive_until_block,
        railgun_contract: "0x0000000000000000000000000000000000000001".to_string(),
        relay_adapt_contract: "0x0000000000000000000000000000000000000004".to_string(),
        relay_adapt_7702_contract: defaults.relay_adapt_7702_contract.to_string(),
        wrapped_native_token: Some("0x0000000000000000000000000000000000000002".to_string()),
        multicall_contract: "0x0000000000000000000000000000000000000003".to_string(),
        finality_depth: defaults.finality_depth,
        block_range: None,
        poll_interval_secs: None,
        gas: EffectiveChainGasSettings {
            gas_limit_buffer: 42,
            gas_price_buffer_numerator: 111,
            gas_price_buffer_denominator: 100,
        },
    };

    let config = public_chain_runtime_config(1, Some(&effective)).expect("effective config");

    assert_eq!(config.rpc_urls.len(), 1);
    assert_eq!(config.rpc_urls[0].as_str(), "https://rpc.example/");
    assert_eq!(
        config.railgun_contract,
        address!("0x0000000000000000000000000000000000000001")
    );
    assert_eq!(
        config.relay_adapt_contract,
        address!("0x0000000000000000000000000000000000000004")
    );
    assert_eq!(
        config.wrapped_native_token,
        Some(address!("0x0000000000000000000000000000000000000002"))
    );
    assert_eq!(
        config.multicall_contract,
        address!("0x0000000000000000000000000000000000000003")
    );
    assert_eq!(config.gas.gas_limit_buffer, 42);
}

#[test]
fn walletconnect_effective_public_chain_config_rejects_disabled_chain() {
    let defaults = chain_defaults_for_public_chain(1).expect("ethereum defaults");
    let effective = EffectiveChainConfig {
        chain_id: 1,
        enabled: false,
        rpc_endpoints: vec!["https://rpc.example".to_string()],
        archive_rpc_url: None,
        quick_sync_enabled: true,
        quick_sync_endpoint: defaults.quick_sync_endpoint.map(|url| url.to_string()),
        indexed_artifact_source_mode: crate::settings::IndexedArtifactSourceModeSetting::Disabled,
        indexed_artifact_source: None,
        indexed_wallet_block_range: defaults.indexed_wallet_block_range,
        deployment_block: defaults.deployment_block,
        v2_start_block: defaults.v2_start_block,
        legacy_shield_block: defaults.legacy_shield_block,
        archive_until_block: defaults.archive_until_block,
        railgun_contract: defaults.contract.to_string(),
        relay_adapt_contract: defaults.relay_adapt_contract.to_string(),
        relay_adapt_7702_contract: defaults.relay_adapt_7702_contract.to_string(),
        wrapped_native_token: None,
        multicall_contract: defaults.multicall_contract.to_string(),
        finality_depth: defaults.finality_depth,
        block_range: None,
        poll_interval_secs: None,
        gas: EffectiveChainGasSettings {
            gas_limit_buffer: 42,
            gas_price_buffer_numerator: 111,
            gas_price_buffer_denominator: 100,
        },
    };

    let Err(error) = public_chain_runtime_config(1, Some(&effective)) else {
        panic!("disabled chain was accepted")
    };

    assert!(error.to_string().contains("disabled"));
}

#[test]
fn effective_public_chain_config_uses_default_rpc_fallbacks() {
    let defaults = chain_defaults_for_public_chain(1).expect("ethereum defaults");
    let config = public_chain_runtime_config(1, None).expect("default config");

    assert_eq!(config.rpc_urls, defaults.rpc_urls);
    assert!(config.rpc_urls.len() > 1);
}

#[test]
fn public_send_request_uses_native_value_or_erc20_transfer() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let recipient = address!("0x2222222222222222222222222222222222222222");
    let token = address!("0x3333333333333333333333333333333333333333");
    let amount = U256::from(5_u64);

    let native = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Transfer {
            asset: PublicAssetId::Native,
            amount,
            recipient,
        },
    )
    .expect("native transfer request");
    assert_eq!(native.to, Some(recipient.into()));
    assert_eq!(native.value, Some(amount));

    let erc20 = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Transfer {
            asset: PublicAssetId::Erc20(token),
            amount,
            recipient,
        },
    )
    .expect("ERC20 transfer request");
    assert_eq!(erc20.to, Some(token.into()));
    let expected_transfer = PublicErc20::transferCall { recipient, amount }.abi_encode();
    assert_eq!(
        erc20.input.input().expect("transfer input").as_ref(),
        expected_transfer.as_slice()
    );
}

#[test]
fn public_send_request_supports_raw_calls_and_contract_creation() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let to = address!("0x2222222222222222222222222222222222222222");
    let call_data = Bytes::from_static(&[0x12, 0x34, 0x56, 0x78, 0xaa]);
    let call = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Raw {
            to: Some(to),
            value: U256::from(7_u64),
            data: call_data.clone(),
        },
    )
    .expect("raw call request");
    assert_eq!(call.to, Some(to.into()));
    assert_eq!(call.value, Some(U256::from(7_u64)));
    assert_eq!(call.input.input(), Some(&call_data));

    let init_code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00]);
    let creation = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Raw {
            to: None,
            value: U256::ZERO,
            data: init_code.clone(),
        },
    )
    .expect("contract creation request");
    assert_eq!(creation.to, Some(TxKind::Create));
    assert_eq!(creation.value, Some(U256::ZERO));
    assert_eq!(creation.input.input(), Some(&init_code));

    let mut managed_creation =
        public_action_eip1559_transaction_request(creation, 1, from, 10, 2, 3);
    managed_creation.gas = Some(100_000);
    let built = managed_creation
        .build_consensus_tx()
        .expect("build managed contract creation transaction");
    let alloy::consensus::TypedTransaction::Eip1559(built) = built else {
        panic!("expected EIP-1559 contract creation")
    };
    assert_eq!(built.to, TxKind::Create);
    assert_eq!(built.value, U256::ZERO);
    assert_eq!(built.input, init_code);

    let value_call = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Raw {
            to: Some(to),
            value: U256::from(1_u64),
            data: Bytes::new(),
        },
    )
    .expect("positive-value empty-data call");
    assert_eq!(value_call.to, Some(to.into()));
    assert_eq!(value_call.value, Some(U256::from(1_u64)));
}

#[test]
fn public_send_request_rejects_empty_raw_intents() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let to = address!("0x2222222222222222222222222222222222222222");

    let empty_creation = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Raw {
            to: None,
            value: U256::from(1_u64),
            data: Bytes::new(),
        },
    )
    .expect_err("empty creation must fail");
    assert!(empty_creation.to_string().contains("non-empty init code"));

    let no_op = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Raw {
            to: Some(to),
            value: U256::ZERO,
            data: Bytes::new(),
        },
    )
    .expect_err("empty zero-value call must fail");
    assert!(no_op.to_string().contains("native value or data"));
}

#[test]
fn advanced_public_transaction_authorization_is_payload_and_gas_bounded() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let to = address!("0x2222222222222222222222222222222222222222");
    let intent = PublicTransactionIntent::Raw {
        to: Some(to),
        value: U256::from(7_u64),
        data: Bytes::from_static(&[0x12, 0x34, 0x56, 0x78]),
    };
    let fingerprint = public_advanced_transaction_payload_fingerprint(1, from, &intent, 10, 2);
    let authorization = PublicAdvancedTransactionAuthorization {
        payload_fingerprint: fingerprint,
        gas_limit: buffered_advanced_gas_limit(50_000, 12_345),
    };

    assert_eq!(authorization.gas_limit, 62_345);
    assert_eq!(
        public_send_authorized_gas_limit(
            1,
            from,
            &intent,
            Some(authorization),
            PublicActionGasFeeSelection::Custom {
                max_fee_per_gas: 10,
                max_priority_fee_per_gas: 2,
            },
        )
        .expect("matching authorization"),
        Some(62_345)
    );
    assert!(ensure_advanced_gas_estimate_authorized(62_345, 62_345).is_ok());
    assert!(ensure_advanced_gas_estimate_authorized(62_346, 62_345).is_err());

    let changed_intent = PublicTransactionIntent::Raw {
        to: Some(to),
        value: U256::from(8_u64),
        data: Bytes::from_static(&[0x12, 0x34, 0x56, 0x78]),
    };
    let stale = public_send_authorized_gas_limit(
        1,
        from,
        &changed_intent,
        Some(authorization),
        PublicActionGasFeeSelection::Custom {
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 2,
        },
    )
    .expect_err("changed payload must invalidate authorization");
    assert!(stale.to_string().contains("changed after gas estimation"));

    let authorized_fee = PublicActionGasFeeSelection::Custom {
        max_fee_per_gas: 10,
        max_priority_fee_per_gas: 2,
    };
    assert!(
        ensure_public_action_command_gas_fee_authorized(Some(authorized_fee), authorized_fee)
            .is_ok()
    );
    assert!(
        ensure_public_action_command_gas_fee_authorized(
            Some(authorized_fee),
            PublicActionGasFeeSelection::Custom {
                max_fee_per_gas: 11,
                max_priority_fee_per_gas: 2,
            },
        )
        .is_err()
    );
}

#[test]
fn advanced_native_balance_exposure_includes_value_and_authorized_gas() {
    assert_eq!(
        public_action_native_exposure(U256::from(7_u64), 50_000, 10),
        U256::from(500_007_u64)
    );
}

#[test]
fn transaction_receipt_output_omits_absent_contract_address() {
    let transfer = crate::TxReceiptOutput {
        tx_hash: "0x01".to_string(),
        status: true,
        block_number: 1,
        gas_used: 21_000,
        contract_address: None,
    };
    let transfer_json = serde_json::to_value(&transfer).expect("serialize transfer receipt");
    assert!(transfer_json.get("contract_address").is_none());

    let creation = crate::TxReceiptOutput {
        contract_address: Some("0x2222222222222222222222222222222222222222".to_string()),
        ..transfer
    };
    let creation_json = serde_json::to_value(&creation).expect("serialize creation receipt");
    assert_eq!(
        creation_json["contract_address"],
        "0x2222222222222222222222222222222222222222"
    );
}

#[test]
fn public_native_shield_request_wraps_and_shields_through_relay_adapt() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let relay_adapt = address!("0x2222222222222222222222222222222222222222");
    let amount = uint!(5_U256);
    let shield_data = vec![0x04, 0x4a, 0x40, 0xc3, 0xaa];

    let tx =
        public_native_shield_transaction_request(1, from, relay_adapt, amount, shield_data.clone());

    assert_eq!(tx.to, Some(relay_adapt.into()));
    assert_eq!(tx.value, Some(amount));
    let input = tx.input.input().expect("relay adapt multicall input");
    let multicall =
        PublicRelayAdapt::multicallCall::abi_decode(input).expect("decode relay adapt multicall");
    assert!(multicall._requireSuccess);
    assert_eq!(multicall._calls.len(), 2);
    assert_eq!(multicall._calls[0].to, relay_adapt);
    assert_eq!(multicall._calls[0].value, U256::ZERO);
    let wrap = PublicRelayAdapt::wrapBaseCall::abi_decode(&multicall._calls[0].data)
        .expect("decode wrap base call");
    assert_eq!(wrap._amount, amount);
    assert_eq!(multicall._calls[1].to, relay_adapt);
    assert_eq!(multicall._calls[1].value, U256::ZERO);
    assert_eq!(multicall._calls[1].data.as_ref(), shield_data);
}

#[test]
fn public_action_eip1559_request_sets_fee_caps_and_nonce() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let recipient = address!("0x2222222222222222222222222222222222222222");
    let base = public_send_transaction_request(
        1,
        from,
        &PublicTransactionIntent::Transfer {
            asset: PublicAssetId::Native,
            amount: U256::from(5_u64),
            recipient,
        },
    )
    .expect("native transfer request");

    let tx = public_action_eip1559_transaction_request(base, 1, from, 42, 3, 9);

    assert_eq!(tx.chain_id, Some(1));
    assert_eq!(tx.from, Some(from));
    assert_eq!(tx.to, Some(recipient.into()));
    assert_eq!(tx.max_fee_per_gas, Some(42));
    assert_eq!(tx.max_priority_fee_per_gas, Some(3));
    assert_eq!(tx.nonce, Some(9));
}

#[test]
fn walletconnect_transaction_fill_preserves_supplied_fee_and_nonce_fields() {
    let from = address!("0x1111111111111111111111111111111111111111");
    let recipient = address!("0x2222222222222222222222222222222222222222");
    let legacy = TransactionRequest {
        from: Some(from),
        to: Some(recipient.into()),
        gas_price: Some(9),
        gas: Some(21_000),
        nonce: Some(4),
        ..Default::default()
    };

    let legacy = public_action_fill_walletconnect_transaction_request(legacy, 1, from, 42, 3, 4)
        .expect("fill legacy request");

    assert_eq!(legacy.gas_price, Some(9));
    assert_eq!(legacy.max_fee_per_gas, None);
    assert_eq!(legacy.max_priority_fee_per_gas, None);
    assert_eq!(legacy.gas, Some(21_000));
    assert_eq!(legacy.nonce, Some(4));

    let eip1559 = TransactionRequest {
        from: Some(from),
        to: Some(recipient.into()),
        max_fee_per_gas: Some(42),
        nonce: Some(5),
        ..Default::default()
    };
    let eip1559 = public_action_fill_walletconnect_transaction_request(eip1559, 1, from, 99, 3, 5)
        .expect("fill eip1559 request");

    assert_eq!(eip1559.max_fee_per_gas, Some(42));
    assert_eq!(eip1559.max_priority_fee_per_gas, Some(3));
    assert_eq!(eip1559.nonce, Some(5));
}

#[test]
fn public_action_replacement_bump_reuses_self_broadcast_policy() {
    assert_eq!(public_action_replacement_bumped_fee(8), 9);
    assert_eq!(public_action_replacement_bumped_fee(9), 11);
}

#[test]
fn public_action_tip_fallback_uses_rpc_gas_price_only_for_bnb() {
    assert_eq!(
        public_action_tip_fallback(56),
        SelfBroadcastTipFallback::RpcGasPrice,
    );
    assert_eq!(
        public_action_tip_fallback(1),
        SelfBroadcastTipFallback::Minimum,
    );
}

#[test]
fn public_action_receipt_poll_error_message_requires_all_checked_providers_to_fail() {
    assert!(public_action_receipt_poll_error_message(0, 0, None).is_none());
    assert!(
        public_action_receipt_poll_error_message(
            2,
            1,
            Some("https://rpc.example: rate limited".to_string()),
        )
        .is_none()
    );

    let message = public_action_receipt_poll_error_message(
        2,
        0,
        Some("https://rpc.example: rate limited".to_string()),
    )
    .expect("all checked providers failed");

    assert!(message.contains("all accepted RPC providers"));
    assert!(message.contains("2 checked"));
    assert!(message.contains("rate limited"));
}

#[test]
fn public_actions_reject_zero_amount_before_signing() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let (root_dir, db, store, view_session) = public_action_request_parts();
    let http = HttpContext::direct_for_tests();
    let recipient = address!("0x2222222222222222222222222222222222222222");

    let send_result = runtime.block_on(submit_public_send(
        PublicSendRequest {
            chain_id: 1,
            effective_chain: None,
            view_session: Arc::clone(&view_session),
            vault_store: Arc::clone(&store),
            vault_password: Zeroizing::new(TEST_PASSWORD.to_string()),
            trezor_app_passphrase: None,
            trezor_pin_matrix_provider: None,
            public_account_uuid: "unused".to_string(),
            intent: PublicTransactionIntent::Transfer {
                asset: PublicAssetId::Native,
                amount: U256::ZERO,
                recipient,
            },
            advanced_authorization: None,
            gas_fee: PublicActionGasFeeSelection::Auto,
            command_rx: None,
            event_tx: None,
        },
        &http,
    ));
    match send_result {
        Ok(_) => panic!("zero-value public send unexpectedly succeeded"),
        Err(error) => assert!(error.to_string().contains("amount is required")),
    }

    let shield_result = runtime.block_on(submit_public_shield(
        PublicShieldRequest {
            chain_id: 1,
            effective_chain: None,
            view_session,
            vault_store: store,
            vault_password: Zeroizing::new(TEST_PASSWORD.to_string()),
            trezor_app_passphrase: None,
            trezor_pin_matrix_provider: None,
            public_account_uuid: "unused".to_string(),
            asset: PublicAssetId::Native,
            amount: U256::ZERO,
            gas_fee: PublicActionGasFeeSelection::Auto,
            command_rx: None,
            event_tx: None,
        },
        &http,
    ));
    match shield_result {
        Ok(_) => panic!("zero-value public shield unexpectedly succeeded"),
        Err(error) => assert!(error.to_string().contains("amount is required")),
    }

    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn vaulted_public_signer_resolves_private_self_broadcast_gas_payers() {
    let (root_dir, db, store, view_session) = public_action_request_parts();
    let derived = store
        .list_active_public_accounts_for_session(&view_session)
        .expect("active accounts")
        .into_iter()
        .find(|account| account.source == PublicAccountSource::Derived)
        .expect("derived account");
    let derived_secret_key = format!("public-account-secret|{}", derived.public_account_uuid);
    assert!(
        db.get_desktop_wallet_vault_record(&derived_secret_key)
            .expect("load derived secret record")
            .is_none()
    );

    let derived_signer = vaulted_public_signer(
        &store,
        &view_session,
        Some(TEST_PASSWORD),
        &derived.public_account_uuid,
        None,
        None,
    )
    .expect("derived signer");
    assert_eq!(derived_signer.address(), derived.address);
    let Err(missing_password) = vaulted_public_signer(
        &store,
        &view_session,
        None,
        &derived.public_account_uuid,
        None,
        None,
    ) else {
        panic!("software public signer without password unexpectedly succeeded");
    };
    assert!(
        missing_password
            .to_string()
            .contains("vault password required for software public account signer")
    );

    let hardware_index = store
        .next_derived_public_account_index_for_session(&view_session)
        .expect("next hardware public index");
    let hardware_descriptor = HardwarePublicAccountDescriptor::for_wallet_public_index(
        HardwareDeviceKind::Ledger,
        view_session.derivation_index(),
        hardware_index,
    )
    .expect("hardware descriptor");
    let hardware_address = address!("0x2222222222222222222222222222222222222222");
    let confirmed =
        ConfirmedHardwarePublicAccount::new_for_tests(hardware_descriptor, hardware_address);
    assert!(matches!(
        store.add_hardware_public_account(&view_session, &confirmed, Some("Ledger Gas")),
        Err(VaultError::HardwareWalletViewRequiresDevice)
    ));

    let hardware_wallet_id = "hardware-public-action-wallet";
    let hardware_private_descriptor = HardwareDerivationDescriptor::ledger_eip1024_v1(
        parse_bip32_path("m/44'/60'/0'/0/0").expect("hardware path"),
        0,
        "ledger:evm:0x1111111111111111111111111111111111111111".to_string(),
        HardwareWalletSyncIntent::CreateNew,
    );
    let output = HardwareOperationOutput::new([42; 32]);
    let view_access_key =
        hardware_view_access_key_from_hardware_output(&hardware_private_descriptor, &output)
            .expect("hardware view key");
    let entropy = synthetic_entropy_from_hardware_output(&hardware_private_descriptor, output)
        .expect("hardware entropy");
    let hardware_metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            "Hardware public action",
            hardware_private_descriptor.clone(),
        )
        .expect("hardware wallet metadata");
    store
        .store_hardware_derived_wallet_from_entropy_with_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            hardware_private_descriptor.account_index,
            entropy.expose_secret(),
            &hardware_metadata,
            &view_access_key,
        )
        .expect("store hardware wallet");
    let hardware_session = store
        .hardware_profile_session_for_fingerprint(
            TEST_PASSWORD,
            HardwareDeviceKind::Ledger,
            &hardware_private_descriptor.profile_fingerprint,
            None,
        )
        .expect("hardware profile session");
    let hardware_view_session = store
        .load_hardware_view_session(
            TEST_PASSWORD,
            &hardware_session,
            hardware_wallet_id,
            &view_access_key,
        )
        .expect("hardware view session");
    let hardware_public_descriptor = HardwarePublicAccountDescriptor::for_wallet_public_index(
        HardwareDeviceKind::Ledger,
        hardware_view_session.derivation_index(),
        0,
    )
    .expect("hardware public descriptor");
    let hardware_public = store
        .add_hardware_public_account(
            &hardware_view_session,
            &ConfirmedHardwarePublicAccount::new_for_tests(
                hardware_public_descriptor,
                address!("0x3333333333333333333333333333333333333333"),
            ),
            Some("Hardware Ledger Gas"),
        )
        .expect("hardware public account under hardware view");
    let hardware_secret_key = format!(
        "public-account-secret|{}",
        hardware_public.public_account_uuid
    );
    assert!(
        db.get_desktop_wallet_vault_record(&hardware_secret_key)
            .expect("load hardware public secret record")
            .is_none()
    );
    let hardware_signer = vaulted_public_signer(
        &store,
        &hardware_view_session,
        None,
        &hardware_public.public_account_uuid,
        None,
        None,
    )
    .expect("hardware signer with profile session");
    assert_eq!(hardware_signer.address(), hardware_public.address);
    assert!(hardware_signer.requires_device_approval());

    let imported = store
        .import_public_account(
            TEST_PASSWORD,
            &view_session,
            TEST_IMPORTED_PRIVATE_KEY,
            Some("Imported"),
            false,
        )
        .expect("import public account");
    let imported_signer = vaulted_public_signer(
        &store,
        &view_session,
        Some(TEST_PASSWORD),
        &imported.public_account_uuid,
        None,
        None,
    )
    .expect("imported signer");
    assert_eq!(imported_signer.address(), imported.address);

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn hardware_public_signer_consumes_trezor_app_passphrase_once() {
    let mut hardware_session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Trezor,
        HardwareProfileBinding::evm_address_fingerprint(
            "trezor:evm:0x1111111111111111111111111111111111111111",
        ),
        Some(vec![1, 2, 3]),
    );
    hardware_session.set_trezor_passphrase_mode(TrezorPassphraseMode::EnterInApp);
    let signer = HardwarePublicEvmSigner {
        address: address!("0x1111111111111111111111111111111111111111"),
        descriptor: HardwarePublicAccountDescriptor::for_wallet_public_index(
            HardwareDeviceKind::Trezor,
            0,
            0,
        )
        .expect("trezor descriptor"),
        hardware_session: std::sync::Mutex::new(hardware_session),
        trezor_app_passphrase: std::sync::Mutex::new(Some(Zeroizing::new("app secret".to_owned()))),
        trezor_pin_matrix_provider: None,
    };

    let passphrase = signer
        .take_trezor_app_passphrase()
        .expect("first passphrase take");
    assert_eq!(passphrase.as_str(), "app secret");
    assert!(signer.take_trezor_app_passphrase().is_none());
}

#[test]
fn hardware_public_signer_updates_in_memory_trezor_session_id_preserving_typed_data_mode() {
    let mut hardware_session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Trezor,
        HardwareProfileBinding::evm_address_fingerprint(
            "trezor:evm:0x1111111111111111111111111111111111111111",
        ),
        Some(vec![1, 2, 3]),
    );
    hardware_session.set_trezor_passphrase_mode(TrezorPassphraseMode::EnterInApp);
    let descriptor =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Trezor, 0, 0)
            .expect("trezor descriptor");
    hardware_session
        .cache_typed_data_signing_mode(&descriptor, HardwareTypedDataSigningMode::ClearSign)
        .expect("cache typed-data mode");
    let signer = HardwarePublicEvmSigner {
        address: address!("0x1111111111111111111111111111111111111111"),
        descriptor,
        hardware_session: std::sync::Mutex::new(hardware_session),
        trezor_app_passphrase: std::sync::Mutex::new(None),
        trezor_pin_matrix_provider: None,
    };

    signer
        .replace_trezor_session_id_if_trezor(Some(vec![4, 5, 6]))
        .expect("replace Trezor session id");
    assert_eq!(
        signer
            .hardware_session()
            .expect("hardware session")
            .trezor_session_id,
        Some(vec![4, 5, 6])
    );
    assert_eq!(
        signer
            .hardware_session()
            .expect("hardware session")
            .typed_data_signing_mode(&signer.descriptor),
        Some(HardwareTypedDataSigningMode::ClearSign)
    );
    signer
        .replace_trezor_session_id_if_trezor(None)
        .expect("clear Trezor session id");
    assert_eq!(
        signer
            .hardware_session()
            .expect("hardware session")
            .trezor_session_id,
        None
    );
    assert_eq!(
        signer
            .hardware_session()
            .expect("hardware session")
            .typed_data_signing_mode(&signer.descriptor),
        Some(HardwareTypedDataSigningMode::ClearSign)
    );
}

#[cfg(not(feature = "hardware"))]
#[test]
fn hardware_typed_data_probe_is_unsupported_without_hardware_feature() {
    let hardware_session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Ledger,
        HardwareProfileBinding::evm_address_fingerprint(
            "ledger:evm:0x1111111111111111111111111111111111111111",
        ),
        None,
    );
    let signer = HardwarePublicEvmSigner {
        address: address!("0x1111111111111111111111111111111111111111"),
        descriptor: HardwarePublicAccountDescriptor::for_wallet_public_index(
            HardwareDeviceKind::Ledger,
            0,
            0,
        )
        .expect("ledger descriptor"),
        hardware_session: std::sync::Mutex::new(hardware_session),
        trezor_app_passphrase: std::sync::Mutex::new(None),
        trezor_pin_matrix_provider: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let mode = runtime
        .block_on(signer.typed_data_signing_mode())
        .expect("default-build typed-data mode");

    assert_eq!(mode, HardwareTypedDataSigningMode::Unsupported);
    assert_eq!(
        signer
            .hardware_session()
            .expect("hardware session")
            .typed_data_signing_mode(&signer.descriptor),
        Some(HardwareTypedDataSigningMode::Unsupported)
    );
}
