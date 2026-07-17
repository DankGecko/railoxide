use super::super::*;
use super::helpers::*;
use std::fs;

#[test]
fn hardware_derived_wallet_stores_view_and_descriptor_without_spend_entropy() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "hardware-wallet";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(descriptor.account_index);
    let pending_chain_ids = BTreeSet::from([1, 137]);
    let metadata = store
        .new_hardware_wallet_metadata_with_pending_create_new_chain_ids(
            TEST_PASSWORD,
            wallet_id,
            "Ledger wallet",
            descriptor.clone(),
            pending_chain_ids.clone(),
        )
        .expect("hardware metadata");

    let stored = store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            descriptor.account_index,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(descriptor.account_index),
        )
        .expect("store hardware wallet");

    assert!(
        db.get_desktop_wallet_vault_record(&stored.view_record_key)
            .expect("load view record")
            .is_some()
    );
    assert!(
        db.get_desktop_wallet_vault_record(&stored.metadata_record_key)
            .expect("load metadata record")
            .is_some()
    );
    assert!(
        db.get_desktop_wallet_vault_record(&wallet_spend_record_key(wallet_id))
            .expect("load spend record")
            .is_none()
    );

    let loaded = store
        .load_wallet_metadata(TEST_PASSWORD, wallet_id)
        .expect("load hardware metadata");
    assert_eq!(loaded.source, WalletSource::LedgerDerived);
    assert_eq!(loaded.hardware_descriptor, Some(descriptor.clone()));
    assert_eq!(loaded.pending_create_new_chain_ids, pending_chain_ids);
    let hardware_account = loaded
        .hardware_account
        .as_ref()
        .expect("hardware account metadata");
    assert_eq!(hardware_account.account_index, descriptor.account_index);
    assert_eq!(hardware_account.descriptor, descriptor);
    assert_eq!(
        hardware_account.custody_backend,
        HardwareRailgunAccountCustodyBackend::SyntheticSoftwareV1
    );
    assert!(hardware_account.custody_backend.is_supported());
    let expected_receive_address = test_hardware_receive_address(descriptor.account_index);
    assert_eq!(
        hardware_account.receive_address.as_deref(),
        Some(expected_receive_address.as_str())
    );
    let profiles = store
        .list_hardware_profile_metadata(TEST_PASSWORD)
        .expect("hardware profiles");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].device_kind, HardwareDeviceKind::Ledger);
    assert_eq!(
        profiles[0].passphrase_used,
        HardwareProfilePassphraseState::Unknown
    );
    assert!(profiles[0].preferred_trezor_passphrase_mode.is_none());
    let accounts = store
        .list_hardware_accounts_for_profile(TEST_PASSWORD, &profiles[0].profile_id)
        .expect("hardware accounts for profile");
    assert_eq!(accounts, vec![hardware_account.clone()]);

    assert!(matches!(
        store.load_view_session(TEST_PASSWORD, wallet_id),
        Err(VaultError::HardwareWalletViewRequiresDevice)
    ));
    let view_session = load_test_hardware_view_session(&store, wallet_id, &descriptor);
    assert!(matches!(
        store
            .wallet_spend_source_for_session(&view_session, wallet_id)
            .expect("spend source"),
        WalletSpendSource::HardwareDerived(found) if found == descriptor
    ));
    let mut grant = store
        .create_spend_grant(TEST_PASSWORD)
        .expect("create grant");
    assert!(matches!(
        store.load_spend_bundle(&mut grant, wallet_id),
        Err(VaultError::VaultNotFound)
    ));

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn recovered_hardware_wallet_metadata_never_infers_pending_create_new_chains() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let mut descriptor = test_hardware_descriptor(0);
    descriptor.sync_intent = HardwareWalletSyncIntent::RecoverExisting;

    let metadata = store
        .new_hardware_wallet_metadata_with_pending_create_new_chain_ids(
            TEST_PASSWORD,
            "recovered-hardware-wallet",
            "Recovered hardware",
            descriptor,
            BTreeSet::from([1, 137]),
        )
        .expect("create recovered hardware metadata");

    assert!(metadata.pending_create_new_chain_ids.is_empty());

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_view_session_backfills_receive_address() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "hardware-wallet-backfill";
    let descriptor = test_hardware_descriptor(1);
    let wallet = test_hardware_wallet(descriptor.account_index);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            "Ledger wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            descriptor.account_index,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(descriptor.account_index),
        )
        .expect("store hardware wallet");
    let mut loaded = store
        .load_wallet_metadata(TEST_PASSWORD, wallet_id)
        .expect("load hardware metadata");
    loaded
        .hardware_account
        .as_mut()
        .expect("hardware account")
        .receive_address = None;
    store
        .store_wallet_metadata(TEST_PASSWORD, &loaded)
        .expect("store legacy hardware metadata");

    let session = load_test_hardware_view_session(&store, wallet_id, &descriptor);
    let expected_receive_address = session.receive_address().expect("receive address");
    let refreshed = store
        .load_wallet_metadata(TEST_PASSWORD, wallet_id)
        .expect("load refreshed hardware metadata");

    assert_eq!(
        refreshed
            .hardware_account
            .as_ref()
            .and_then(|account| account.receive_address.as_deref()),
        Some(expected_receive_address.as_str())
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_view_session_rejects_wrong_context_or_view_key() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "hardware-wallet-wrong-context";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            "Ledger wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");

    let wrong_session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Ledger,
        HardwareProfileBinding::evm_address_fingerprint(
            "ledger:evm:0x3333333333333333333333333333333333333333",
        ),
        None,
    );
    assert!(matches!(
        store.load_hardware_view_session(
            TEST_PASSWORD,
            &wrong_session,
            wallet_id,
            &test_hardware_view_access_key(0),
        ),
        Err(VaultError::HardwareWalletIdentityMismatch)
    ));

    let hardware_session = store
        .hardware_profile_session_for_fingerprint(
            TEST_PASSWORD,
            HardwareDeviceKind::Ledger,
            &descriptor.profile_fingerprint,
            None,
        )
        .expect("hardware session");
    assert!(matches!(
        store.load_hardware_view_session(
            TEST_PASSWORD,
            &hardware_session,
            wallet_id,
            &HardwareViewAccessKey::new([9u8; KEY_LEN]),
        ),
        Err(VaultError::Decrypt)
    ));

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_cache_keys_require_hardware_view_context() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "hardware-wallet-cache";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            "Ledger wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    let view_session = load_test_hardware_view_session(&store, wallet_id, &descriptor);

    let hardware_keys = view_session
        .derive_cache_keys("hardware-chain")
        .expect("hardware cache keys");
    let password_keys = store
        .unlock_view(TEST_PASSWORD)
        .expect("password view")
        .derive_cache_keys("hardware-chain")
        .expect("password cache keys");
    let row_id = hardware_keys.row_id(0, 1, b"stable-utxo");
    let record = hardware_keys
        .encrypt_row(&row_id, b"private cache row")
        .expect("encrypt hardware cache row");

    assert!(password_keys.decrypt_row(&row_id, &record).is_err());
    assert_eq!(
        &*hardware_keys
            .decrypt_row(&row_id, &record)
            .expect("decrypt hardware cache row"),
        b"private cache row",
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn unindexed_hardware_metadata_does_not_require_hiding_for_software_deletion() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let software_wallet_id = "11111111111111111111111111111111";
    let software_session = import_wallet_with_metadata(&store, software_wallet_id, "Software");
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let hardware_metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "chain-hardware",
            "Hardware",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "chain-hardware",
            descriptor.account_index,
            &wallet,
            &hardware_metadata,
            &test_hardware_view_access_key(descriptor.account_index),
        )
        .expect("store hardware wallet");
    let hardware_session = load_test_hardware_view_session(&store, "chain-hardware", &descriptor);

    store
        .store_wallet_chain_metadata_with_session(
            &hardware_session,
            &WalletChainMetadataBundle {
                wallet_chain_uuid: "000-hardware-chain".to_owned(),
                wallet_uuid: "chain-hardware".to_owned(),
                chain_type: 0,
                chain_id: 1,
                contract: "0x1111111111111111111111111111111111111111".to_owned(),
                start_block: 1,
                last_scanned_block: 0,
                last_scanned_block_hash: None,
                poi_read_source: None,
            },
        )
        .expect("store hardware chain metadata");
    let later_hardware_chain = WalletChainMetadataBundle {
        wallet_chain_uuid: "999-hardware-chain".to_owned(),
        wallet_uuid: "chain-hardware".to_owned(),
        chain_type: 0,
        chain_id: 56,
        contract: "0x3333333333333333333333333333333333333333".to_owned(),
        start_block: 2,
        last_scanned_block: 1,
        last_scanned_block_hash: None,
        poi_read_source: None,
    };
    store
        .store_wallet_chain_metadata_with_session(&hardware_session, &later_hardware_chain)
        .expect("store later hardware chain metadata");
    for key in [
        wallet_chain_index_record_key("chain-hardware", "000-hardware-chain"),
        wallet_chain_index_record_key("chain-hardware", &later_hardware_chain.wallet_chain_uuid),
        wallet_chain_index_complete_record_key("chain-hardware"),
    ] {
        db.delete_desktop_wallet_vault_record(&key)
            .expect("remove legacy ownership record");
    }
    store
        .find_wallet_chain_metadata_for_session(
            &hardware_session,
            0,
            1,
            "0x1111111111111111111111111111111111111111",
        )
        .expect("scan legacy hardware metadata")
        .expect("requested hardware metadata");
    assert!(
        db.get_desktop_wallet_vault_record(&wallet_chain_index_record_key(
            "chain-hardware",
            &later_hardware_chain.wallet_chain_uuid,
        ))
        .expect("load later hardware ownership index")
        .is_some()
    );
    assert!(
        db.get_desktop_wallet_vault_record(&wallet_chain_index_complete_record_key(
            "chain-hardware",
        ))
        .expect("load hardware ownership completeness")
        .is_some()
    );
    let unindexed_hardware_key =
        wallet_chain_index_record_key("chain-hardware", "000-hardware-chain");
    db.delete_desktop_wallet_vault_record(&unindexed_hardware_key)
        .expect("remove foreign hardware ownership index");
    db.delete_desktop_wallet_vault_record(&wallet_chain_index_complete_record_key(
        software_wallet_id,
    ))
    .expect("remove software creation-time completeness");
    let expected = WalletChainMetadataBundle {
        wallet_chain_uuid: "22222222222222222222222222222222".to_owned(),
        wallet_uuid: software_wallet_id.to_owned(),
        chain_type: 0,
        chain_id: 1,
        contract: "0x2222222222222222222222222222222222222222".to_owned(),
        start_block: 10,
        last_scanned_block: 9,
        last_scanned_block_hash: None,
        poi_read_source: None,
    };
    store
        .store_wallet_chain_metadata_with_session(&software_session, &expected)
        .expect("store software chain metadata");

    let found = store
        .find_wallet_chain_metadata_for_session(
            &software_session,
            expected.chain_type,
            expected.chain_id,
            &expected.contract,
        )
        .expect("find software chain metadata")
        .expect("software chain metadata present");
    assert_eq!(found.wallet_chain_uuid, expected.wallet_chain_uuid);
    assert_eq!(found.wallet_uuid, expected.wallet_uuid);
    assert_eq!(found.chain_type, expected.chain_type);
    assert_eq!(found.chain_id, expected.chain_id);
    assert_eq!(found.contract, expected.contract);
    assert_eq!(found.start_block, expected.start_block);
    assert_eq!(found.last_scanned_block, expected.last_scanned_block);
    assert!(
        db.get_desktop_wallet_vault_record(&wallet_chain_index_complete_record_key(
            software_wallet_id,
        ))
        .expect("load software ownership completeness")
        .is_some()
    );
    assert!(
        db.get_desktop_wallet_vault_record(&unindexed_hardware_key)
            .expect("load foreign hardware ownership index")
            .is_none()
    );

    store
        .delete_wallet_for_session(&software_session, software_wallet_id)
        .expect("delete software wallet with foreign hardware metadata");
    let first_hardware_metadata_key = wallet_chain_metadata_record_key("000-hardware-chain");
    assert!(
        db.get_desktop_wallet_vault_record(&first_hardware_metadata_key)
            .expect("load retained first hardware metadata")
            .is_some()
    );
    assert!(
        db.get_desktop_wallet_vault_record(&wallet_chain_metadata_record_key(
            &later_hardware_chain.wallet_chain_uuid,
        ))
        .expect("load retained later hardware metadata")
        .is_some()
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_profile_metadata_serializes_without_passphrase_hint() {
    let descriptor = test_hardware_descriptor(0);
    let profile = HardwareProfileMetadata::from_descriptor(&descriptor);
    let profile_json = serde_json::to_value(&profile).expect("profile json");
    let descriptor_json = serde_json::to_value(&descriptor).expect("descriptor json");

    assert!(profile_json.get("label").is_some());
    assert!(profile_json.get("passphrase_used").is_some());
    assert!(profile_json.get("passphrase_hint").is_none());
    assert!(descriptor_json.get("passphrase_hint").is_none());
}
#[test]
fn unsupported_hardware_custody_backend_fails_closed() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let view_session = import_wallet_with_metadata(&store, "unsupported-wallet", "Unsupported");
    let descriptor = test_hardware_descriptor(0);
    let profile = HardwareProfileMetadata::from_descriptor(&descriptor);
    let mut metadata = store
        .load_wallet_metadata(TEST_PASSWORD, "unsupported-wallet")
        .expect("load metadata");
    metadata.hardware_account = Some(HardwareRailgunAccountMetadata {
        profile_id: profile.profile_id,
        account_index: descriptor.account_index,
        label: "Unsupported".to_owned(),
        descriptor,
        account_identity: HardwareRailgunAccountIdentity {
            spending_public_key: view_session
                .spending_public_key()
                .map(|value| value.to_be_bytes()),
            viewing_public_key: view_session.scan_keys().viewing_public_key,
        },
        receive_address: None,
        custody_backend: HardwareRailgunAccountCustodyBackend::Unsupported(
            "future_native".to_owned(),
        ),
    });
    store
        .store_wallet_metadata(TEST_PASSWORD, &metadata)
        .expect("store unsupported metadata");

    assert!(matches!(
        store.load_view_session(TEST_PASSWORD, "unsupported-wallet"),
        Err(VaultError::UnsupportedHardwareCustodyBackend(name)) if name == "future_native"
    ));
    let backend: HardwareRailgunAccountCustodyBackend =
        serde_json::from_str("\"future_native\"").expect("backend");
    assert_eq!(
        backend,
        HardwareRailgunAccountCustodyBackend::Unsupported("future_native".to_owned())
    );
    assert!(!backend.is_supported());

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn first_view_session_skips_unsupported_hardware_accounts() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let unsupported_session = import_wallet_with_metadata(&store, "aaa-unsupported", "Unsupported");
    let descriptor = test_hardware_descriptor(0);
    let profile = HardwareProfileMetadata::from_descriptor(&descriptor);
    let mut metadata = store
        .load_wallet_metadata(TEST_PASSWORD, "aaa-unsupported")
        .expect("load metadata");
    metadata.hardware_account = Some(HardwareRailgunAccountMetadata {
        profile_id: profile.profile_id,
        account_index: descriptor.account_index,
        label: "Unsupported".to_owned(),
        descriptor,
        account_identity: HardwareRailgunAccountIdentity {
            spending_public_key: unsupported_session
                .spending_public_key()
                .map(|value| value.to_be_bytes()),
            viewing_public_key: unsupported_session.scan_keys().viewing_public_key,
        },
        receive_address: None,
        custody_backend: HardwareRailgunAccountCustodyBackend::Unsupported(
            "future_native".to_owned(),
        ),
    });
    store
        .store_wallet_metadata(TEST_PASSWORD, &metadata)
        .expect("store unsupported metadata");
    import_wallet_with_metadata(&store, "zzz-software", "Software");

    let unlocked = store
        .unlock_first_view_session(TEST_PASSWORD)
        .expect("unlock first supported view")
        .expect("software view session");

    assert_eq!(unlocked.wallet_id(), "zzz-software");
    assert!(matches!(
        store.load_view_session(TEST_PASSWORD, "aaa-unsupported"),
        Err(VaultError::UnsupportedHardwareCustodyBackend(name)) if name == "future_native"
    ));

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_recovery_account_indices_are_bounded_or_exact() {
    assert_eq!(
        DesktopVaultStore::default_hardware_recovery_account_index(),
        0
    );
    assert_eq!(
        DesktopVaultStore::bounded_hardware_recovery_account_indices(2, 3)
            .expect("bounded indices"),
        vec![2, 3, 4]
    );
    assert_eq!(
        DesktopVaultStore::bounded_hardware_recovery_account_indices(0, 255)
            .expect("max bounded indices")
            .len(),
        255
    );
    assert_eq!(
        DesktopVaultStore::exact_hardware_recovery_account_index(9).expect("exact index"),
        9
    );
    assert!(matches!(
        DesktopVaultStore::bounded_hardware_recovery_account_indices(0, 0),
        Err(VaultError::InvalidHardwareAccountRecoveryRange)
    ));
    assert!(matches!(
        DesktopVaultStore::bounded_hardware_recovery_account_indices(0, 256),
        Err(VaultError::InvalidHardwareAccountRecoveryRange)
    ));
    assert!(matches!(
        DesktopVaultStore::exact_hardware_recovery_account_index(
            crate::hardware::HARDENED_BIP32_INDEX
        ),
        Err(VaultError::InvalidHardwareAccountRecoveryRange)
    ));
}
#[test]
fn hardware_profile_session_matches_known_and_new_profiles() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-session",
            "Ledger wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-session",
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(descriptor.account_index),
        )
        .expect("store hardware wallet");

    let matched = store
        .hardware_profile_session_for_fingerprint(
            TEST_PASSWORD,
            HardwareDeviceKind::Ledger,
            &descriptor.profile_fingerprint,
            None,
        )
        .expect("matched session");
    let loaded = store
        .load_wallet_metadata(TEST_PASSWORD, "hardware-wallet-session")
        .expect("load hardware metadata");
    let account = loaded.hardware_account.expect("hardware account");
    assert!(matched.profile_id.is_some());
    DesktopVaultStore::verify_hardware_profile_session_for_account(&matched, &account)
        .expect("session verifies account");

    let new_profile = store
        .hardware_profile_session_for_fingerprint(
            TEST_PASSWORD,
            HardwareDeviceKind::Ledger,
            "ledger:evm:0x2222222222222222222222222222222222222222",
            None,
        )
        .expect("new session");
    assert!(new_profile.profile_id.is_none());
    assert_eq!(new_profile.device_kind, HardwareDeviceKind::Ledger);

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_profile_session_rejects_wrong_profile_and_discards_trezor_session() {
    let descriptor = test_hardware_descriptor(0);
    let profile = HardwareProfileMetadata::from_descriptor(&descriptor);
    let account = HardwareRailgunAccountMetadata::synthetic_software_v1(
        profile.profile_id.clone(),
        descriptor.account_index,
        "Ledger wallet",
        descriptor.clone(),
        HardwareRailgunAccountIdentity::from_wallet_keys(&test_hardware_wallet(0)),
    );
    let mut wrong_session = HardwareProfileSession::matched(
        HardwareDeviceKind::Trezor,
        profile.profile_id,
        HardwareProfileBinding::evm_address_fingerprint(descriptor.profile_fingerprint),
        Some(vec![1, 2, 3]),
    );

    assert!(matches!(
        DesktopVaultStore::verify_hardware_profile_session_for_account(&wrong_session, &account),
        Err(VaultError::HardwareWalletIdentityMismatch)
    ));
    assert_eq!(
        wrong_session.trezor_passphrase_mode(),
        TrezorPassphraseMode::NoPassphrase
    );
    wrong_session.set_trezor_passphrase_mode(TrezorPassphraseMode::EnterInApp);
    assert!(wrong_session.uses_trezor_app_passphrase());
    wrong_session.discard_trezor_session();
    assert!(wrong_session.trezor_session_id.is_none());
    assert!(wrong_session.uses_trezor_app_passphrase());
}
#[test]
fn hardware_profile_session_typed_data_capability_is_runtime_scoped() {
    let mut session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Trezor,
        HardwareProfileBinding::evm_address_fingerprint(
            "trezor:evm:0x1111111111111111111111111111111111111111",
        ),
        Some(vec![1, 2, 3]),
    );
    let descriptor =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Trezor, 0, 0)
            .expect("trezor descriptor");
    let other_account =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Trezor, 0, 1)
            .expect("other trezor descriptor");

    assert_eq!(session.typed_data_signing_mode(&descriptor), None);
    session
        .cache_typed_data_signing_mode(
            &descriptor,
            crate::hardware::HardwareTypedDataSigningMode::ClearSign,
        )
        .expect("cache typed-data mode");

    assert_eq!(
        session.typed_data_signing_mode(&descriptor),
        Some(crate::hardware::HardwareTypedDataSigningMode::ClearSign)
    );
    assert_eq!(session.typed_data_signing_mode(&other_account), None);

    session.trezor_session_id = Some(vec![4, 5, 6]);
    assert_eq!(session.typed_data_signing_mode(&descriptor), None);

    session
        .cache_typed_data_signing_mode(
            &descriptor,
            crate::hardware::HardwareTypedDataSigningMode::Eip712HashFallback,
        )
        .expect("cache refreshed typed-data mode");
    assert_eq!(
        session.typed_data_signing_mode(&descriptor),
        Some(crate::hardware::HardwareTypedDataSigningMode::Eip712HashFallback)
    );
    session.discard_trezor_session();
    assert_eq!(session.typed_data_signing_mode(&descriptor), None);
}
#[test]
fn hardware_profile_session_downgrades_clear_typed_data_capability_to_hash_fallback() {
    let mut session = HardwareProfileSession::unmatched(
        HardwareDeviceKind::Ledger,
        HardwareProfileBinding::evm_address_fingerprint(
            "ledger:evm:0x1111111111111111111111111111111111111111",
        ),
        None,
    );
    let descriptor =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Ledger, 0, 0)
            .expect("ledger descriptor");
    let other_account =
        HardwarePublicAccountDescriptor::for_wallet_public_index(HardwareDeviceKind::Ledger, 0, 1)
            .expect("other ledger descriptor");

    assert!(
        !session
            .downgrade_typed_data_signing_mode_to_hash_fallback(&descriptor)
            .expect("downgrade without cache")
    );
    session
        .cache_typed_data_signing_mode(
            &descriptor,
            crate::hardware::HardwareTypedDataSigningMode::ClearSign,
        )
        .expect("cache clear mode");

    assert!(
        !session
            .downgrade_typed_data_signing_mode_to_hash_fallback(&other_account)
            .expect("downgrade mismatched cache")
    );
    assert!(
        session
            .downgrade_typed_data_signing_mode_to_hash_fallback(&descriptor)
            .expect("downgrade clear mode")
    );
    assert_eq!(
        session.typed_data_signing_mode(&descriptor),
        Some(crate::hardware::HardwareTypedDataSigningMode::Eip712HashFallback)
    );
    assert!(
        !session
            .downgrade_typed_data_signing_mode_to_hash_fallback(&descriptor)
            .expect("downgrade fallback mode")
    );
}
#[test]
fn view_session_clone_with_hardware_profile_session_refreshes_trezor_session() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "trezor-session-refresh-wallet";
    let descriptor = test_trezor_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            "Trezor wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    let view_session = load_test_hardware_view_session(&store, wallet_id, &descriptor);
    let mut refreshed_session = view_session
        .hardware_profile_session()
        .expect("hardware session")
        .clone();
    refreshed_session.trezor_session_id = Some(vec![4, 5, 6]);
    refreshed_session.set_trezor_passphrase_mode(TrezorPassphraseMode::EnterInApp);

    let refreshed = view_session.clone_with_hardware_profile_session(refreshed_session.clone());

    assert_eq!(refreshed.wallet_id(), view_session.wallet_id());
    assert_eq!(
        refreshed.derivation_index(),
        view_session.derivation_index()
    );
    let refreshed_keys = refreshed.scan_keys();
    let original_keys = view_session.scan_keys();
    assert_eq!(
        refreshed_keys.viewing_private_key,
        original_keys.viewing_private_key
    );
    assert_eq!(
        refreshed_keys.viewing_public_key,
        original_keys.viewing_public_key
    );
    assert_eq!(refreshed_keys.nullifying_key, original_keys.nullifying_key);
    assert_eq!(
        refreshed_keys.master_public_key,
        original_keys.master_public_key
    );
    assert_eq!(
        refreshed.hardware_profile_session(),
        Some(&refreshed_session)
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_spend_signer_rejects_wrong_derived_key() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let wallet_id = "hardware-wallet";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(descriptor.account_index);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            wallet_id,
            "Ledger wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            wallet_id,
            descriptor.account_index,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(descriptor.account_index),
        )
        .expect("store hardware wallet");
    let view_session = load_test_hardware_view_session(&store, wallet_id, &descriptor);

    assert!(matches!(
        store.hardware_railgun_spend_signer_from_entropy(&view_session, &descriptor, &[43u8; 32]),
        Err(VaultError::HardwareWalletIdentityMismatch)
    ));
    store
        .hardware_railgun_spend_signer_from_entropy(&view_session, &descriptor, &[42u8; 32])
        .expect("matching hardware entropy signs");

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_profile_account_index_auto_increments() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let profile = HardwareWalletProfile {
        device_kind: crate::hardware::HardwareDeviceKind::Ledger,
        profile_fingerprint: "ledger-profile-fingerprint".to_owned(),
    };
    assert_eq!(
        store
            .next_hardware_account_index_for_profile(TEST_PASSWORD, &profile)
            .expect("next empty index"),
        0
    );

    for (wallet_id, label, account_index) in [
        ("hardware-wallet-0", "Ledger wallet 0", 0),
        ("hardware-wallet-2", "Ledger wallet 2", 2),
    ] {
        let descriptor = test_hardware_descriptor(account_index);
        let wallet = test_hardware_wallet(account_index);
        let metadata = store
            .new_hardware_wallet_metadata(TEST_PASSWORD, wallet_id, label, descriptor.clone())
            .expect("hardware metadata");
        store
            .store_hardware_derived_wallet_with_metadata(
                TEST_PASSWORD,
                wallet_id,
                account_index,
                &wallet,
                &metadata,
                &test_hardware_view_access_key(account_index),
            )
            .expect("store hardware wallet");
    }

    assert_eq!(
        store
            .next_hardware_account_index_for_profile(TEST_PASSWORD, &profile)
            .expect("next used index"),
        3
    );
    assert_eq!(
        store
            .list_hardware_wallet_profiles(TEST_PASSWORD)
            .expect("profiles"),
        vec![profile]
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn deleted_hardware_wallet_account_index_remains_reserved() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let _primary_session = import_wallet_with_metadata(&store, "software-wallet", "Software");
    let profile = HardwareWalletProfile {
        device_kind: crate::hardware::HardwareDeviceKind::Ledger,
        profile_fingerprint: "ledger-profile-fingerprint".to_owned(),
    };
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-0",
            "Ledger wallet 0",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-0",
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    for record in db
        .list_desktop_wallet_vault_records(HARDWARE_WALLET_ACCOUNT_INDEX_PREFIX)
        .expect("list hardware index reservations")
    {
        db.delete_desktop_wallet_vault_record(&record.key)
            .expect("delete setup reservation");
    }

    let hardware_session =
        load_test_hardware_view_session(&store, "hardware-wallet-0", &descriptor);
    store
        .delete_wallet_for_session(&hardware_session, "hardware-wallet-0")
        .expect("delete hardware wallet");
    assert_eq!(
        db.list_desktop_wallet_vault_records(HARDWARE_WALLET_ACCOUNT_INDEX_PREFIX)
            .expect("list deletion-time hardware index reservation")
            .len(),
        1
    );

    assert_eq!(
        store
            .next_hardware_account_index_for_profile(TEST_PASSWORD, &profile)
            .expect("next reserved index"),
        1
    );

    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-restored",
            "Ledger wallet restored",
            descriptor.clone(),
        )
        .expect("explicit restore metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-restored",
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store restored hardware wallet");
    let loaded = store
        .load_wallet_metadata(TEST_PASSWORD, "hardware-wallet-restored")
        .expect("load restored hardware metadata");
    assert_eq!(loaded.hardware_descriptor, Some(descriptor));
    assert_eq!(
        store
            .next_hardware_account_index_for_profile(TEST_PASSWORD, &profile)
            .expect("next index after explicit restore"),
        1
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_wallet_account_index_rejects_existing_inactive_wallet() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let _primary_session = import_wallet_with_metadata(&store, "software-wallet", "Software");
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-0",
            "Ledger wallet 0",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-0",
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    store
        .deactivate_wallet(TEST_PASSWORD, "hardware-wallet-0")
        .expect("deactivate hardware wallet");

    assert!(matches!(
        store.new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-copy",
            "Ledger wallet copy",
            descriptor,
        ),
        Err(VaultError::DuplicateHardwareWalletAccountIndex)
    ));

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn hardware_wallet_metadata_rejects_duplicate_labels_and_invalid_sources() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-a",
            "Ledger wallet",
            descriptor,
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-a",
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");

    assert!(matches!(
        store.new_hardware_wallet_metadata(
            TEST_PASSWORD,
            "hardware-wallet-b",
            "Ledger wallet",
            test_hardware_descriptor(1),
        ),
        Err(VaultError::DuplicateWalletLabel)
    ));

    let mut invalid = metadata;
    invalid.source = WalletSource::Imported;
    assert!(matches!(
        store.store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            "hardware-wallet-a",
            0,
            &wallet,
            &invalid,
            &test_hardware_view_access_key(0),
        ),
        Err(VaultError::InvalidHardwareWalletDescriptor)
    ));

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn permanent_wallet_delete_purges_hardware_private_chain_cache_records() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let _software_session =
        import_wallet_with_metadata(&store, "software-delete-survivor", "Software");
    let hardware_wallet_id = "68617264776172652d64656c65746521";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            "Hardware delete wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    let hardware_session = load_test_hardware_view_session(&store, hardware_wallet_id, &descriptor);
    let chain_metadata = store
        .wallet_chain_metadata_for_session(
            &hardware_session,
            0,
            1,
            "0x1111111111111111111111111111111111111111",
            100,
        )
        .expect("hardware chain metadata");
    let wallet_chain_uuid = chain_metadata.wallet_chain_uuid;
    let wallet_cache_key = wallet_chain_uuid
        .parse::<WalletCacheKey>()
        .expect("hardware wallet cache key");
    db.put_wallet_utxo(&wallet_cache_key, "hardware-row", b"hardware cache row")
        .expect("store hardware cache row");
    assert!(
        store
            .load_wallet_chain_metadata(TEST_PASSWORD, &wallet_chain_uuid)
            .is_err(),
        "hardware chain metadata must require the hardware private view"
    );
    let deleted = store
        .delete_wallet_for_session(&hardware_session, hardware_wallet_id)
        .expect("delete hardware wallet");

    assert_eq!(deleted.wallet_uuid, hardware_wallet_id);
    for key in [
        wallet_metadata_record_key(hardware_wallet_id),
        wallet_view_record_key(hardware_wallet_id),
        wallet_chain_metadata_record_key(&wallet_chain_uuid),
    ] {
        assert!(
            db.get_desktop_wallet_vault_record(&key)
                .expect("load deleted hardware record")
                .is_none(),
            "expected {key} to be deleted"
        );
    }
    assert!(
        db.list_wallet_utxos(&wallet_cache_key)
            .expect("list deleted hardware cache")
            .is_empty()
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
#[test]
fn permanent_wallet_delete_refuses_password_or_wrong_wallet_session() {
    let (root_dir, db, store) = desktop_store_with_vault();
    let software_session =
        import_wallet_with_metadata(&store, "software-delete-survivor", "Software");
    let hardware_wallet_id = "hardware-delete-password-view";
    let descriptor = test_hardware_descriptor(0);
    let wallet = test_hardware_wallet(0);
    let metadata = store
        .new_hardware_wallet_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            "Hardware delete wallet",
            descriptor.clone(),
        )
        .expect("hardware metadata");
    store
        .store_hardware_derived_wallet_with_metadata(
            TEST_PASSWORD,
            hardware_wallet_id,
            0,
            &wallet,
            &metadata,
            &test_hardware_view_access_key(0),
        )
        .expect("store hardware wallet");
    let hardware_session = load_test_hardware_view_session(&store, hardware_wallet_id, &descriptor);
    let chain_metadata = store
        .wallet_chain_metadata_for_session(
            &hardware_session,
            0,
            1,
            "0x1111111111111111111111111111111111111111",
            100,
        )
        .expect("hardware chain metadata");
    let wallet_chain_uuid = chain_metadata.wallet_chain_uuid;
    let wallet_cache_key = wallet_chain_uuid
        .parse::<WalletCacheKey>()
        .expect("hardware wallet cache key");
    db.put_wallet_utxo(&wallet_cache_key, "hardware-row", b"hardware cache row")
        .expect("store hardware cache row");

    assert!(matches!(
        store.delete_wallet_for_session(&software_session, hardware_wallet_id),
        Err(VaultError::HardwareWalletIdentityMismatch)
    ));
    let view = store.unlock_view(TEST_PASSWORD).expect("password view");
    assert!(matches!(
        store.delete_wallet_with_view_unlock(&view, hardware_wallet_id),
        Err(VaultError::HardwareWalletViewRequiresDevice)
    ));
    let mut account_only_metadata = store
        .list_wallet_metadata_with_view_unlock(&view, true)
        .expect("list hardware metadata")
        .into_iter()
        .find(|metadata| metadata.wallet_uuid == hardware_wallet_id)
        .expect("hardware metadata present");
    account_only_metadata.hardware_descriptor = None;
    store
        .store_wallet_metadata(TEST_PASSWORD, &account_only_metadata)
        .expect("store account-only hardware metadata");
    assert!(matches!(
        store.delete_wallet_with_view_unlock(&view, hardware_wallet_id),
        Err(VaultError::HardwareWalletViewRequiresDevice)
    ));
    assert!(matches!(
        store.delete_wallet_for_session(&software_session, hardware_wallet_id),
        Err(VaultError::HardwareWalletIdentityMismatch)
    ));
    for key in [
        wallet_metadata_record_key(hardware_wallet_id),
        wallet_view_record_key(hardware_wallet_id),
        wallet_chain_metadata_record_key(&wallet_chain_uuid),
    ] {
        assert!(
            db.get_desktop_wallet_vault_record(&key)
                .expect("load hardware private cache record")
                .is_some(),
            "expected {key} to remain without hardware private view"
        );
    }
    assert_eq!(
        db.list_wallet_utxos(&wallet_cache_key)
            .expect("list retained hardware private cache")
            .len(),
        1
    );

    drop(store);
    drop(db);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}
