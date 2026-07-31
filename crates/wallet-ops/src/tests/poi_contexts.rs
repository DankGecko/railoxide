use super::helpers::*;
use railgun_wallet::tx::{
    MixedPrivateOutputRole, MixedPrivateOutputSource, MixedPrivatePlannedOutput,
    MixedPrivateSendRole,
};

fn wallet_cache_key(label: &str) -> String {
    local_db::WalletCacheKey::from_opaque_bytes(label.as_bytes())
        .expect("non-empty test wallet cache key")
        .to_string()
}

#[test]
fn mixed_pending_output_contexts_follow_planned_chunk_and_role_metadata() {
    let token = address(0x32);
    let recipient_note = sample_note(19, token, 5);
    let change_note = sample_note(20, token, 3);
    let recipient_chunk = sample_chunk(3, 0x19, vec![recipient_note.clone()], false);
    let change_chunk = sample_chunk(4, 0x20, vec![change_note.clone()], false);
    let chunks = vec![recipient_chunk, change_chunk];
    let outputs = vec![
        MixedPrivatePlannedOutput {
            source: MixedPrivateOutputSource::Send(0),
            transaction_index: 0,
            output_index: 0,
            token_address: token,
            amount: recipient_note.value,
            role: MixedPrivateOutputRole::Recipient(MixedPrivateSendRole::Primary),
            note: recipient_note.clone(),
        },
        MixedPrivatePlannedOutput {
            source: MixedPrivateOutputSource::Send(0),
            transaction_index: 1,
            output_index: 0,
            token_address: token,
            amount: change_note.value,
            role: MixedPrivateOutputRole::Change,
            note: change_note.clone(),
        },
    ];
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, &chunks);

    let records = crate::poi_contexts::build_pending_mixed_output_poi_context_records(
        1,
        "wallet-1",
        123,
        &chunks,
        &outputs,
        &pre_transaction_pois,
        &poi_list_keys,
    )
    .expect("build mixed pending output contexts");

    assert_eq!(records.len(), 2);
    let recipient = records
        .iter()
        .find(|record| record.output_role == PendingOutputPoiRole::Recipient)
        .expect("recipient record");
    assert_eq!(recipient.chain_id, 1);
    assert_eq!(recipient.wallet_id, "wallet-1");
    assert_eq!(recipient.created_at, 123);
    assert_eq!(recipient.utxo_tree_in, u64::from(chunks[0].tree_number));
    assert_eq!(recipient.required_poi_list_keys, poi_list_keys);
    assert_eq!(
        recipient.output_commitment,
        FixedBytes::from(recipient_note.commitment().to_be_bytes::<32>())
    );
    assert_eq!(
        recipient.output_npk,
        FixedBytes::from(recipient_note.npk.to_be_bytes::<32>())
    );
    let change = records
        .iter()
        .find(|record| record.output_role == PendingOutputPoiRole::Change)
        .expect("change record");
    assert_eq!(change.utxo_tree_in, u64::from(chunks[1].tree_number));
    assert_ne!(change.railgun_txid, recipient.railgun_txid);
    assert_eq!(
        change.output_commitment,
        FixedBytes::from(change_note.commitment().to_be_bytes::<32>())
    );
}

#[test]
fn manual_send_pending_output_contexts_persist_without_tx_hash() {
    let root_dir = temp_db_root();
    let store = DbStore::open(DbConfig {
        root_dir: root_dir.clone(),
    })
    .expect("open db");
    let token = address(0x33);
    let recipient_note = sample_note(1, token, 5);
    let change_note = sample_note(2, token, 3);
    let chunk = sample_chunk(
        4,
        0x20,
        vec![recipient_note.clone(), change_note.clone()],
        false,
    );
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let count = crate::poi_contexts::persist_pending_send_output_poi_contexts_for_test(
        &store,
        1,
        &wallet_cache_key("wallet-1"),
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        false,
        false,
    )
    .expect("persist pending send output contexts");

    assert_eq!(count, 2);
    let records = store
        .list_pending_output_poi_contexts(1, &wallet_cache_key("wallet-1"))
        .expect("list pending output POI contexts");
    assert_eq!(records.len(), 2);
    let recipient = records
        .iter()
        .find(|record| record.output_role == PendingOutputPoiRole::Recipient)
        .expect("recipient context");
    assert_eq!(recipient.wallet_id, wallet_cache_key("wallet-1"));
    assert_eq!(
        recipient.output_commitment,
        FixedBytes::from(recipient_note.commitment().to_be_bytes::<32>())
    );
    assert!(recipient.source_operation_id.is_none());
    assert!(recipient.observation.is_none());
    assert_eq!(recipient.required_poi_list_keys, poi_list_keys);
    let change = records
        .iter()
        .find(|record| record.output_role == PendingOutputPoiRole::Change)
        .expect("change context");
    assert_eq!(
        change.output_commitment,
        FixedBytes::from(change_note.commitment().to_be_bytes::<32>())
    );

    drop(store);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn manual_unshield_pending_output_contexts_skip_public_output_without_tx_hash() {
    let root_dir = temp_db_root();
    let store = DbStore::open(DbConfig {
        root_dir: root_dir.clone(),
    })
    .expect("open db");
    let token = address(0x34);
    let change_note = sample_note(3, token, 7);
    let unshield_note = Note::new_unshield(address(0xaa), token, uint!(5_U256));
    let chunk = sample_chunk(
        5,
        0x30,
        vec![change_note.clone(), unshield_note.clone()],
        true,
    );
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let count = crate::poi_contexts::persist_pending_unshield_output_poi_contexts_for_test(
        &store,
        1,
        &wallet_cache_key("wallet-1"),
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        false,
        false,
    )
    .expect("persist pending unshield output contexts");

    assert_eq!(count, 1);
    let records = store
        .list_pending_output_poi_contexts(1, &wallet_cache_key("wallet-1"))
        .expect("list pending output POI contexts");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].output_role, PendingOutputPoiRole::Change);
    assert_eq!(
        records[0].output_commitment,
        FixedBytes::from(change_note.commitment().to_be_bytes::<32>())
    );
    assert_ne!(
        records[0].output_commitment,
        FixedBytes::from(unshield_note.commitment().to_be_bytes::<32>())
    );
    assert!(records[0].source_operation_id.is_none());
    assert!(records[0].observation.is_none());

    drop(store);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn public_broadcaster_pending_output_contexts_include_fee_outputs() {
    let root_dir = temp_db_root();
    let store = DbStore::open(DbConfig {
        root_dir: root_dir.clone(),
    })
    .expect("open db");
    let token = address(0x35);
    let fee_note = sample_note(4, token, 1);
    let recipient_note = sample_note(5, token, 8);
    let change_note = sample_note(6, token, 2);
    let chunk = sample_chunk(
        6,
        0x40,
        vec![fee_note.clone(), recipient_note, change_note],
        false,
    );
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let count = crate::poi_contexts::persist_pending_send_output_poi_contexts_for_test(
        &store,
        1,
        &wallet_cache_key("wallet-1"),
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        true,
        false,
    )
    .expect("persist public broadcaster send output contexts");

    assert_eq!(count, 3);
    let records = store
        .list_pending_output_poi_contexts(1, &wallet_cache_key("wallet-1"))
        .expect("list pending output POI contexts");
    assert_eq!(records.len(), 3);
    assert!(records.iter().any(|record| record.output_role
        == PendingOutputPoiRole::BroadcasterFee
        && record.output_commitment
            == FixedBytes::from(fee_note.commitment().to_be_bytes::<32>())));

    drop(store);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn public_broadcaster_unshield_pending_output_contexts_skip_public_output() {
    let root_dir = temp_db_root();
    let store = DbStore::open(DbConfig {
        root_dir: root_dir.clone(),
    })
    .expect("open db");
    let token = address(0x36);
    let fee_note = sample_note(7, token, 1);
    let change_note = sample_note(8, token, 4);
    let unshield_note = Note::new_unshield(address(0xbb), token, uint!(6_U256));
    let chunk = sample_chunk(
        7,
        0x50,
        vec![fee_note, change_note, unshield_note.clone()],
        true,
    );
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let count = crate::poi_contexts::persist_pending_unshield_output_poi_contexts_for_test(
        &store,
        1,
        &wallet_cache_key("wallet-1"),
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        true,
        false,
    )
    .expect("persist public broadcaster unshield output contexts");

    assert_eq!(count, 2);
    let records = store
        .list_pending_output_poi_contexts(1, &wallet_cache_key("wallet-1"))
        .expect("list pending output POI contexts");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::BroadcasterFee)
    );
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::Change)
    );
    assert!(records.iter().all(|record| record.output_commitment
        != FixedBytes::from(unshield_note.commitment().to_be_bytes::<32>())));

    drop(store);
    fs::remove_dir_all(root_dir).expect("remove temp db dir");
}

#[test]
fn separate_fee_token_send_pending_output_contexts_keep_fee_change_role() {
    let fee_token = address(0x39);
    let action_token = address(0x3a);
    let fee_note = sample_note(11, fee_token, 1);
    let fee_change_note = sample_note(12, fee_token, 4);
    let recipient_note = sample_note(13, action_token, 8);
    let action_change_note = sample_note(14, action_token, 2);
    let chunks = vec![
        sample_chunk(
            10,
            0x80,
            vec![fee_note.clone(), fee_change_note.clone()],
            false,
        ),
        sample_chunk(
            11,
            0x81,
            vec![recipient_note.clone(), action_change_note.clone()],
            false,
        ),
    ];
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, &chunks);

    let records = crate::build_pending_output_poi_context_records(
        1,
        "wallet-1",
        123,
        &chunks,
        &pre_transaction_pois,
        &poi_list_keys,
        &crate::pending_send_output_role_plans(true, true),
    )
    .expect("build separate fee send records");

    assert_eq!(records.len(), 4);
    assert!(records.iter().any(|record| record.output_role
        == PendingOutputPoiRole::BroadcasterFee
        && record.output_commitment
            == FixedBytes::from(fee_note.commitment().to_be_bytes::<32>())));
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::Change
                && record.output_commitment
                    == FixedBytes::from(fee_change_note.commitment().to_be_bytes::<32>()))
    );
    assert!(records.iter().any(
        |record| record.output_role == PendingOutputPoiRole::Recipient
            && record.output_commitment
                == FixedBytes::from(recipient_note.commitment().to_be_bytes::<32>())
    ));
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::Change
                && record.output_commitment
                    == FixedBytes::from(action_change_note.commitment().to_be_bytes::<32>()))
    );
}

#[test]
fn separate_fee_token_unshield_pending_output_contexts_skip_action_public_output() {
    let fee_token = address(0x3b);
    let action_token = address(0x3c);
    let fee_note = sample_note(15, fee_token, 1);
    let fee_change_note = sample_note(16, fee_token, 4);
    let action_change_note = sample_note(17, action_token, 2);
    let unshield_note = Note::new_unshield(address(0xdd), action_token, uint!(6_U256));
    let chunks = vec![
        sample_chunk(
            12,
            0x82,
            vec![fee_note.clone(), fee_change_note.clone()],
            false,
        ),
        sample_chunk(
            13,
            0x83,
            vec![action_change_note.clone(), unshield_note.clone()],
            true,
        ),
    ];
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, &chunks);

    let records = crate::build_pending_output_poi_context_records(
        1,
        "wallet-1",
        123,
        &chunks,
        &pre_transaction_pois,
        &poi_list_keys,
        &crate::pending_unshield_output_role_plans(true, true),
    )
    .expect("build separate fee unshield records");

    assert_eq!(records.len(), 3);
    assert!(records.iter().any(|record| record.output_role
        == PendingOutputPoiRole::BroadcasterFee
        && record.output_commitment
            == FixedBytes::from(fee_note.commitment().to_be_bytes::<32>())));
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::Change
                && record.output_commitment
                    == FixedBytes::from(fee_change_note.commitment().to_be_bytes::<32>()))
    );
    assert!(
        records
            .iter()
            .any(|record| record.output_role == PendingOutputPoiRole::Change
                && record.output_commitment
                    == FixedBytes::from(action_change_note.commitment().to_be_bytes::<32>()))
    );
    assert!(records.iter().all(|record| record.output_commitment
        != FixedBytes::from(unshield_note.commitment().to_be_bytes::<32>())));
}

#[test]
fn send_pending_output_role_plan_omits_absent_change() {
    let token = address(0x37);
    let recipient_note = sample_note(9, token, 11);
    let chunk = sample_chunk(8, 0x60, vec![recipient_note.clone()], false);
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let records = crate::build_pending_output_poi_context_records(
        1,
        "wallet-1",
        123,
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        &crate::pending_send_output_role_plans(false, false),
    )
    .expect("build pending send records");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].output_role, PendingOutputPoiRole::Recipient);
    assert_eq!(
        records[0].output_commitment,
        FixedBytes::from(recipient_note.commitment().to_be_bytes::<32>())
    );
}

#[test]
fn unshield_pending_output_role_plan_skips_public_output_without_change() {
    let token = address(0x38);
    let fee_note = sample_note(10, token, 2);
    let unshield_note = Note::new_unshield(address(0xcc), token, uint!(9_U256));
    let chunk = sample_chunk(9, 0x70, vec![fee_note.clone(), unshield_note.clone()], true);
    let poi_list_keys = default_active_poi_list_keys();
    let pre_transaction_pois = poi_map_for_chunks(&poi_list_keys, std::slice::from_ref(&chunk));

    let records = crate::build_pending_output_poi_context_records(
        1,
        "wallet-1",
        123,
        &[chunk],
        &pre_transaction_pois,
        &poi_list_keys,
        &crate::pending_unshield_output_role_plans(true, false),
    )
    .expect("build pending unshield records");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].output_role, PendingOutputPoiRole::BroadcasterFee);
    assert_eq!(
        records[0].output_commitment,
        FixedBytes::from(fee_note.commitment().to_be_bytes::<32>())
    );
    assert_ne!(
        records[0].output_commitment,
        FixedBytes::from(unshield_note.commitment().to_be_bytes::<32>())
    );
}

#[test]
fn self_broadcast_unshield_pending_pois_only_when_private_outputs_exist() {
    let token = address(0x3d);
    let unshield_note = Note::new_unshield(address(0xde), token, uint!(9_U256));
    let no_change_chunk = sample_chunk(14, 0x84, vec![unshield_note.clone()], true);

    assert!(!crate::unshield_chunks_require_pending_output_pois(
        std::slice::from_ref(&no_change_chunk)
    ));

    let change_note = sample_note(18, token, 1);
    let change_chunk = sample_chunk(15, 0x85, vec![change_note, unshield_note], true);

    assert!(crate::unshield_chunks_require_pending_output_pois(
        std::slice::from_ref(&change_chunk)
    ));
}

#[test]
fn self_broadcast_unshield_pending_pois_are_required_for_malformed_chunks() {
    let malformed_chunk = sample_chunk(16, 0x86, Vec::new(), true);

    assert!(crate::unshield_chunks_require_pending_output_pois(
        std::slice::from_ref(&malformed_chunk)
    ));
}
