use super::helpers::*;

#[test]
fn blocked_shield_rescue_eligibility_accepts_matched_origin_account() {
    let token = address(0x11);
    let origin = address(0xaa);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let id = rescue_utxo_id(&blocked);
    let candidate = crate::blocked_shield_rescue_candidate_from_records(
        &[blocked],
        &WalletPendingOverlay::default(),
        &id,
    );
    let eligibility = crate::blocked_shield_rescue_eligibility_for_origin(
        Some(origin),
        &[public_account(
            "pub-1",
            origin,
            crate::vault::PublicAccountStatus::Active,
        )],
    );

    assert!(candidate.is_some());
    assert!(eligibility.eligible);
    assert_eq!(eligibility.origin_address, Some(origin));
    assert_eq!(eligibility.public_account_uuid.as_deref(), Some("pub-1"));
    assert!(eligibility.disabled_reason.is_none());
}

#[test]
fn blocked_shield_rescue_eligibility_requires_origin_account() {
    let origin = address(0xaa);

    let missing = crate::blocked_shield_rescue_eligibility_for_origin(Some(origin), &[]);
    let inactive = crate::blocked_shield_rescue_eligibility_for_origin(
        Some(origin),
        &[public_account(
            "pub-1",
            origin,
            crate::vault::PublicAccountStatus::Inactive,
        )],
    );

    assert!(!missing.eligible);
    assert_eq!(missing.origin_address, Some(origin));
    assert_eq!(
        missing.disabled_reason.as_deref(),
        Some("The Shield origin Public account must be added or activated before refund.")
    );
    assert!(!inactive.eligible);
}

#[test]
fn blocked_shield_rescue_eligibility_reports_unresolved_origin() {
    let eligibility = crate::blocked_shield_rescue_eligibility_for_origin(None, &[]);

    assert!(!eligibility.eligible);
    assert_eq!(eligibility.origin_address, None);
    assert_eq!(
        eligibility.disabled_reason.as_deref(),
        Some(
            "Source transaction origin could not be resolved. Retry after checking RPC connectivity."
        )
    );
}

#[test]
fn blocked_shield_rescue_candidate_rejects_ineligible_utxos() {
    let token = address(0x11);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let blocked_id = rescue_utxo_id(&blocked);
    let transact = utxo(token, 5, 0, 2);
    let transact_id = rescue_utxo_id(&transact);
    let shield = utxo_with_kind(token, 5, 0, 3, UtxoCommitmentKind::Shield);
    let shield_id = rescue_utxo_id(&shield);
    let mut spent = blocked_shield_utxo(token, 5, 0, 4);
    let spent_id = rescue_utxo_id(&spent);
    spent.spent = Some(source(9));
    let pending_overlay = WalletPendingOverlay {
        pending_spent: vec![WalletPendingSpent {
            tree: blocked.utxo.tree,
            position: blocked.utxo.position,
            stable_identity: None,
            tx_hash: Some(FixedBytes::from([0x99; 32])),
            block_number: Some(20),
            block_timestamp: Some(1_700_000_020),
        }],
        ..WalletPendingOverlay::default()
    };

    assert!(
        crate::blocked_shield_rescue_candidate_from_records(
            std::slice::from_ref(&blocked),
            &WalletPendingOverlay::default(),
            &blocked_id,
        )
        .is_some()
    );
    assert!(
        crate::blocked_shield_rescue_candidate_from_records(
            &[blocked],
            &pending_overlay,
            &blocked_id,
        )
        .is_none()
    );
    assert!(
        crate::blocked_shield_rescue_candidate_from_records(
            &[transact],
            &WalletPendingOverlay::default(),
            &transact_id,
        )
        .is_none()
    );
    assert!(
        crate::blocked_shield_rescue_candidate_from_records(
            &[shield],
            &WalletPendingOverlay::default(),
            &shield_id,
        )
        .is_none()
    );
    assert!(
        crate::blocked_shield_rescue_candidate_from_records(
            &[spent],
            &WalletPendingOverlay::default(),
            &spent_id,
        )
        .is_none()
    );
}

#[test]
fn blocked_shield_rescue_plan_accepts_exact_single_utxo_unshield() {
    let token = address(0x11);
    let origin = address(0xaa);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let id = rescue_utxo_id(&blocked);
    let plan = rescue_plan_for_test(&blocked.utxo, token, uint!(5_U256), origin, None, None);

    crate::validate_blocked_shield_rescue_plan(&plan, &id, token, uint!(5_U256), origin)
        .expect("valid rescue plan");
}

#[test]
fn blocked_shield_rescue_plan_rejects_additional_private_inputs() {
    let token = address(0x11);
    let origin = address(0xaa);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let extra = blocked_shield_utxo(token, 1, 0, 2);
    let id = rescue_utxo_id(&blocked);
    let plan = rescue_plan_for_test(
        &blocked.utxo,
        token,
        uint!(5_U256),
        origin,
        Some(extra.utxo),
        None,
    );

    assert!(
        crate::validate_blocked_shield_rescue_plan(&plan, &id, token, uint!(5_U256), origin)
            .is_err()
    );
}

#[test]
fn blocked_shield_rescue_plan_rejects_partial_amount() {
    let token = address(0x11);
    let origin = address(0xaa);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let id = rescue_utxo_id(&blocked);
    let plan = rescue_plan_for_test(&blocked.utxo, token, uint!(4_U256), origin, None, None);

    assert!(
        crate::validate_blocked_shield_rescue_plan(&plan, &id, token, uint!(5_U256), origin)
            .is_err()
    );
}

#[test]
fn blocked_shield_rescue_plan_rejects_private_change_outputs() {
    let token = address(0x11);
    let origin = address(0xaa);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let id = rescue_utxo_id(&blocked);
    let plan = rescue_plan_for_test(
        &blocked.utxo,
        token,
        uint!(5_U256),
        origin,
        None,
        Some(uint!(1_U256)),
    );

    assert!(
        crate::validate_blocked_shield_rescue_plan(&plan, &id, token, uint!(5_U256), origin)
            .is_err()
    );
}

#[test]
fn blocked_shield_rescue_rejects_mismatched_gas_payer() {
    assert_eq!(
        crate::matched_blocked_shield_rescue_public_account_uuid(Some("origin"), None)
            .expect("matched account"),
        "origin"
    );
    assert_eq!(
        crate::matched_blocked_shield_rescue_public_account_uuid(Some("origin"), Some("origin"))
            .expect("matched account"),
        "origin"
    );
    assert!(
        crate::matched_blocked_shield_rescue_public_account_uuid(Some("origin"), Some("other"))
            .is_err()
    );
}

#[test]
fn normal_spend_selection_excludes_shield_blocked_utxos() {
    let token = address(0x11);
    let blocked = blocked_shield_utxo(token, 5, 0, 1);
    let selected = crate::poi_verified_unspent_utxos_from_records(
        std::slice::from_ref(&blocked),
        &WalletPendingOverlay::default(),
    );
    let (outputs, _) = utxo_outputs_from_utxos(vec![blocked]);

    assert!(selected.is_empty());
    assert_eq!(max_send_amount_from_outputs(&outputs, token), U256::ZERO);
    assert_eq!(
        max_unshield_amount_from_outputs(&outputs, token),
        U256::ZERO
    );
}
