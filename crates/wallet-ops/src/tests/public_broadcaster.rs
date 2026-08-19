use super::helpers::*;

#[test]
fn public_broadcaster_candidates_filter_unsupported_rows_and_allow_valid_poi_required() {
    let token = address(0x21);
    let relay_adapt = address(0x44);
    let mut rows = vec![fee_row(1, token, 10, 0.9, "ok")];

    let mut invalid_signature = fee_row(1, token, 10, 0.9, "invalid-signature");
    invalid_signature.signature_valid = false;
    rows.push(invalid_signature);

    let mut expired = fee_row(1, token, 10, 0.9, "expired");
    expired.fee_expiration = SystemTime::now() - Duration::from_secs(1);
    rows.push(expired);

    let mut unavailable = fee_row(1, token, 10, 0.9, "unavailable");
    unavailable.available_wallets = 0;
    rows.push(unavailable);

    let mut unsupported_version = fee_row(1, token, 10, 0.9, "version");
    unsupported_version.version = Arc::from("9.0.0");
    rows.push(unsupported_version);

    let mut poi_required = fee_row_with_broadcaster_seed(1, token, 10, 0.9, "poi", 8);
    poi_required.required_poi_list_keys = vec![Arc::from("00".repeat(32))];
    rows.push(poi_required);

    rows.push(fee_row(2, token, 10, 0.9, "chain"));
    rows.push(fee_row(1, address(0x22), 10, 0.9, "token"));

    let mut relay_mismatch = fee_row(1, token, 10, 0.9, "relay");
    relay_mismatch.relay_adapt = address(0x55);
    rows.push(relay_mismatch);

    let candidates =
        eligible_public_broadcasters(&rows, 1, token, Some(relay_adapt), SystemTime::now());

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].fees_id, "ok");
    assert_eq!(candidates[1].fees_id, "poi");
}

#[test]
fn public_broadcaster_candidates_are_keyed_by_selected_fee_token() {
    let action_token = address(0x43);
    let fee_token = address(0x44);
    let candidates = public_broadcaster_candidates(
        &[
            fee_row(1, action_token, 10, 0.9, "action-token"),
            fee_row(1, fee_token, 11, 0.9, "fee-token"),
        ],
        1,
        fee_token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        None,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].fees_id, "fee-token");
    assert_eq!(candidates[0].token, fee_token);
}

#[test]
fn specific_public_broadcaster_fails_when_not_available_for_fee_token() {
    let action_token = address(0x45);
    let fee_token = address(0x46);
    let railgun_address = sample_railgun_address(51);
    let candidates = public_broadcaster_candidates(
        &[fee_row_with_broadcaster_seed(
            1,
            action_token,
            10,
            0.9,
            "action-only",
            51,
        )],
        1,
        fee_token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        None,
    );

    let error = select_public_broadcaster_with_policy(
        &candidates,
        &PublicBroadcasterSelection::Specific { railgun_address },
        BroadcasterFeePolicy::default(),
    )
    .expect_err("specific broadcaster should not match action-token row");

    assert!(error.to_string().contains("no longer eligible"));
}

#[test]
fn public_broadcaster_selection_sorts_by_fee_then_seeded_tie_breaker() {
    let token = address(0x23);
    let candidates = eligible_public_broadcasters(
        &[
            fee_row_with_broadcaster_seed(1, token, 20, 0.99, "expensive", 11),
            fee_row_with_broadcaster_seed(1, token, 10, 0.50, "cheap-low-rel", 12),
            fee_row_with_broadcaster_seed(1, token, 10, 0.90, "cheap-high-rel", 13),
        ],
        1,
        token,
        None,
        SystemTime::now(),
    );
    let sort_seed = (0_u8..=u8::MAX)
        .map(|seed| [seed; 32])
        .find(|seed| {
            sort_specific_public_broadcasters(candidates.clone(), seed)[0].fees_id
                == "cheap-low-rel"
        })
        .expect("test fixture should have a seed that places lower reliability first");

    let sorted = sort_specific_public_broadcasters(candidates.clone(), &sort_seed);
    let ids: Vec<_> = sorted
        .iter()
        .map(|candidate| candidate.fees_id.as_str())
        .collect();
    assert_eq!(ids, vec!["cheap-low-rel", "cheap-high-rel", "expensive"]);
    assert_eq!(
        ids,
        sort_specific_public_broadcasters(candidates.clone(), &sort_seed)
            .iter()
            .map(|candidate| candidate.fees_id.as_str())
            .collect::<Vec<_>>()
    );
    let cheap_low_rel_address = candidates
        .iter()
        .find(|candidate| candidate.fees_id == "cheap-low-rel")
        .expect("cheap-low-rel candidate")
        .railgun_address
        .clone();

    let selected = select_public_broadcaster(
        &candidates,
        &PublicBroadcasterSelection::Specific {
            railgun_address: cheap_low_rel_address,
        },
    )
    .expect("specific candidate");
    assert_eq!(selected.fees_id, "cheap-low-rel");
    assert!(select_public_broadcaster(&candidates, &PublicBroadcasterSelection::Random).is_ok());
}

#[test]
fn public_broadcaster_seeded_tie_breaker_replaces_lexical_address_order() {
    let token = address(0x24);
    let candidates = eligible_public_broadcasters(
        &[
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "candidate-a", 71),
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "candidate-b", 72),
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "candidate-c", 73),
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "candidate-d", 74),
        ],
        1,
        token,
        None,
        SystemTime::now(),
    );
    let mut lexical = candidates.clone();
    lexical.sort_by(|left, right| left.railgun_address.cmp(&right.railgun_address));
    let lexical_ids = lexical
        .iter()
        .map(|candidate| candidate.fees_id.as_str())
        .collect::<Vec<_>>();
    let sort_seed = (0_u8..=u8::MAX)
        .map(|seed| [seed; 32])
        .find(|seed| {
            sort_specific_public_broadcasters(candidates.clone(), seed)
                .iter()
                .map(|candidate| candidate.fees_id.as_str())
                .collect::<Vec<_>>()
                != lexical_ids
        })
        .expect("test fixture should have a seed that differs from lexical address order");

    let sorted = sort_specific_public_broadcasters(candidates, &sort_seed);
    let sorted_ids = sorted
        .iter()
        .map(|candidate| candidate.fees_id.as_str())
        .collect::<Vec<_>>();
    assert_ne!(sorted_ids, lexical_ids);
}

#[test]
fn random_public_broadcaster_pool_includes_valid_poi_candidates_only_after_filters() {
    let token = address(0x27);
    let mut poi_required = fee_row_with_broadcaster_seed(1, token, 10, 0.9, "poi", 31);
    poi_required.required_poi_list_keys = vec![Arc::from("11".repeat(32))];
    let mut malformed = fee_row_with_broadcaster_seed(1, token, 10, 0.9, "malformed", 33);
    malformed.required_poi_list_keys = vec![Arc::from("not-a-list-key")];
    let out_of_policy = fee_row_with_broadcaster_seed(1, token, 20, 0.9, "out-of-policy", 34);
    let banned = fee_row_with_broadcaster_seed(1, token, 10, 0.9, "banned", 35);
    let mut candidates = public_broadcaster_candidates(
        &[
            poi_required,
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "no-poi", 32),
            malformed,
            out_of_policy,
            banned,
        ],
        1,
        token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        Some(U256::from(10)),
    );
    let malformed_candidate = candidates
        .iter()
        .find(|candidate| candidate.fees_id == "no-poi")
        .cloned()
        .expect("no-POI candidate");
    let mut malformed_candidate = malformed_candidate;
    malformed_candidate.required_poi_list_keys = vec!["malformed".to_string()];
    candidates.push(malformed_candidate);

    let trust_filter = PublicBroadcasterTrustFilter {
        preferences: vault::BroadcasterPreferences {
            favorites: Vec::new(),
            banned: vec![broadcaster_preference_entry(35)],
        },
        favorites_only: false,
    };
    let pool = random_eligible_public_broadcasters(
        &candidates,
        BroadcasterFeePolicy::default(),
        &trust_filter,
    );

    assert_eq!(
        pool.iter()
            .map(|candidate| candidate.fees_id.as_str())
            .collect::<Vec<_>>(),
        vec!["poi", "no-poi"]
    );
}

#[test]
fn public_broadcaster_deduplication_keeps_freshest_parsed_identity() {
    let token = address(0x28);
    let now = SystemTime::now();
    let mut older = fee_row_with_broadcaster_seed(1, token, 10, 0.9, "older-fees", 36);
    older.identifier = Some(Arc::from("older-identifier"));
    older.fee_expiration = now + Duration::from_secs(10);
    let mut fresher = older.clone();
    fresher.fees_id = Arc::from("fresh-fees");
    fresher.identifier = Some(Arc::from("fresh-identifier"));
    fresher.fee_expiration = now + Duration::from_secs(20);

    let candidates = eligible_public_broadcasters(&[older, fresher], 1, token, None, now);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].fees_id, "fresh-fees");
    assert_eq!(
        candidates[0].identifier.as_deref(),
        Some("fresh-identifier")
    );

    let mut older_candidate = candidates[0].clone();
    older_candidate.fees_id = "older-direct-fees".to_string();
    older_candidate.identifier = Some("older-direct-identifier".to_string());
    older_candidate.fee_expiration = now + Duration::from_secs(30);
    let mut fresher_candidate = older_candidate.clone();
    fresher_candidate.fees_id = "fresh-direct-fees".to_string();
    fresher_candidate.identifier = Some("fresh-direct-identifier".to_string());
    fresher_candidate.fee_expiration = now + Duration::from_secs(40);

    let pool = random_eligible_public_broadcasters(
        &[older_candidate, fresher_candidate],
        BroadcasterFeePolicy::default(),
        &PublicBroadcasterTrustFilter::default(),
    );
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].fees_id, "fresh-direct-fees");
    assert_eq!(
        pool[0].identifier.as_deref(),
        Some("fresh-direct-identifier")
    );
}

#[test]
fn public_broadcaster_specific_selection_survives_fees_id_refresh() {
    let token = address(0x24);
    let railgun_address = sample_railgun_address(21);
    let candidates = eligible_public_broadcasters(
        &[fee_row_with_broadcaster_seed(
            1,
            token,
            10,
            0.9,
            "fresh-fees-id",
            21,
        )],
        1,
        token,
        None,
        SystemTime::now(),
    );

    let selected = select_public_broadcaster(
        &candidates,
        &PublicBroadcasterSelection::Specific { railgun_address },
    )
    .expect("specific candidate by stable address");

    assert_eq!(selected.fees_id, "fresh-fees-id");
}

#[test]
fn public_broadcaster_trust_filter_gives_banned_precedence() {
    let token = address(0x47);
    let candidates = public_broadcaster_candidates(
        &[
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "favorite-and-banned", 61),
            fee_row_with_broadcaster_seed(1, token, 11, 0.9, "neutral", 62),
        ],
        1,
        token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        None,
    );
    let trust_filter = PublicBroadcasterTrustFilter {
        preferences: vault::BroadcasterPreferences {
            favorites: vec![broadcaster_preference_entry(61)],
            banned: vec![broadcaster_preference_entry(61)],
        },
        favorites_only: false,
    };

    let trusted = filter_public_broadcasters_by_trust(&candidates, &trust_filter);

    assert_eq!(trusted.len(), 1);
    assert_eq!(trusted[0].fees_id, "neutral");
}

#[test]
fn public_broadcaster_trust_filter_supports_favorites_only_selection() {
    let token = address(0x48);
    let candidates = public_broadcaster_candidates(
        &[
            fee_row_with_broadcaster_seed(1, token, 10, 0.9, "favorite", 63),
            fee_row_with_broadcaster_seed(1, token, 11, 0.9, "neutral", 64),
        ],
        1,
        token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        None,
    );
    let favorite_address = sample_railgun_address(63);
    let neutral_address = sample_railgun_address(64);
    let trust_filter = PublicBroadcasterTrustFilter {
        preferences: vault::BroadcasterPreferences {
            favorites: vec![broadcaster_preference_entry(63)],
            banned: Vec::new(),
        },
        favorites_only: true,
    };

    let favorite = select_public_broadcaster_with_policy_and_trust(
        &candidates,
        &PublicBroadcasterSelection::Specific {
            railgun_address: favorite_address,
        },
        BroadcasterFeePolicy::default(),
        &trust_filter,
    )
    .expect("favorite broadcaster remains selectable");
    let error = select_public_broadcaster_with_policy_and_trust(
        &candidates,
        &PublicBroadcasterSelection::Specific {
            railgun_address: neutral_address,
        },
        BroadcasterFeePolicy::default(),
        &trust_filter,
    )
    .expect_err("non-favorite should be rejected");

    assert_eq!(favorite.fees_id, "favorite");
    assert!(error.to_string().contains("preferences"));
}

#[test]
fn public_broadcaster_trust_filter_rejects_stale_estimated_broadcaster() {
    let token = address(0x49);
    let railgun_address = sample_railgun_address(65);
    let candidates = public_broadcaster_candidates(
        &[fee_row_with_broadcaster_seed(
            1, token, 10, 0.9, "stale", 65,
        )],
        1,
        token,
        None,
        SystemTime::now(),
        BroadcasterFeePolicy::default(),
        None,
    );
    let trust_filter = PublicBroadcasterTrustFilter {
        preferences: vault::BroadcasterPreferences {
            favorites: Vec::new(),
            banned: vec![broadcaster_preference_entry(65)],
        },
        favorites_only: false,
    };

    let error = select_public_broadcaster_with_policy_and_trust(
        &candidates,
        &PublicBroadcasterSelection::Specific { railgun_address },
        BroadcasterFeePolicy::default(),
        &trust_filter,
    )
    .expect_err("banned estimated broadcaster should be rejected");

    assert!(error.to_string().contains("preferences"));
}

#[test]
fn public_broadcaster_fee_policy_classifies_anchor_bounds() {
    let token = address(0x28);
    let policy = BroadcasterFeePolicy::default();
    let candidates = public_broadcaster_candidates(
        &[
            fee_row_with_broadcaster_seed(1, token, 89, 0.9, "below", 1),
            fee_row_with_broadcaster_seed(1, token, 90, 0.9, "lower-bound", 2),
            fee_row_with_broadcaster_seed(1, token, 150, 0.9, "upper-bound", 3),
            fee_row_with_broadcaster_seed(1, token, 151, 0.9, "above", 4),
        ],
        1,
        token,
        None,
        SystemTime::now(),
        policy,
        Some(uint!(100_U256)),
    );

    let eligible_ids = fee_policy_eligible_public_broadcasters(&candidates, policy)
        .into_iter()
        .map(|candidate| candidate.fees_id)
        .collect::<Vec<_>>();
    assert_eq!(eligible_ids, vec!["lower-bound", "upper-bound"]);
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.fees_id == "below"
                && matches!(
                    candidate.fee_policy_status,
                    BroadcasterFeePolicyStatus::Suspicious {
                        premium_bps: Some(-1100),
                        ..
                    }
                ))
    );
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.fees_id == "above"
                && matches!(
                    candidate.fee_policy_status,
                    BroadcasterFeePolicyStatus::Suspicious {
                        premium_bps: Some(5100),
                        ..
                    }
                ))
    );
}

#[test]
fn public_broadcaster_fee_policy_allows_unknown_anchor_rows() {
    let token = address(0x29);
    let policy = BroadcasterFeePolicy::default();
    let candidates = public_broadcaster_candidates(
        &[fee_row(1, token, 1_000_000, 0.9, "raw")],
        1,
        token,
        None,
        SystemTime::now(),
        policy,
        None,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].fee_policy_status,
        BroadcasterFeePolicyStatus::UnknownAnchor
    );
    assert_eq!(
        fee_policy_eligible_public_broadcasters(&candidates, policy).len(),
        1
    );
}

#[test]
fn public_broadcaster_policy_uses_fixed_anchor_without_cache() {
    let weth = wrapped_native_token_for_chain(1).expect("ethereum wrapped native");

    assert_eq!(
        fixed_token_anchor_rate(1, weth),
        Some(uint!(1_000_000_000_000_000_000_U256))
    );
    assert_eq!(
        public_broadcaster_anchor_rate_for_policy(None, 1, weth),
        Some(uint!(1_000_000_000_000_000_000_U256))
    );
}

#[test]
fn public_broadcaster_fee_breakdown_splits_gas_and_margin() {
    let breakdown = public_broadcaster_fee_breakdown(
        uint!(2_500_U256),
        10,
        100,
        Some(uint!(2_000_000_000_000_000_000_U256)),
    );

    assert_eq!(breakdown.native_gas_cost, uint!(1_010_U256));
    assert_eq!(breakdown.fee_token_gas_cost, Some(uint!(2_020_U256)));
    assert_eq!(
        breakdown.broadcaster_fee,
        Some(PublicBroadcasterFeeMargin::Positive(uint!(480_U256)))
    );
}

#[test]
fn public_broadcaster_fee_breakdown_handles_negative_and_missing_anchor() {
    let negative = public_broadcaster_fee_breakdown(
        uint!(1_000_U256),
        10,
        100,
        Some(uint!(2_000_000_000_000_000_000_U256)),
    );
    let missing = public_broadcaster_fee_breakdown(uint!(1_000_U256), 10, 100, None);

    assert_eq!(
        negative.broadcaster_fee,
        Some(PublicBroadcasterFeeMargin::Negative(uint!(1_020_U256)))
    );
    assert_eq!(missing.native_gas_cost, uint!(1_010_U256));
    assert_eq!(missing.fee_token_gas_cost, None);
    assert_eq!(missing.broadcaster_fee, None);
}

#[test]
fn public_broadcaster_fee_policy_override_includes_suspicious_rows() {
    let token = address(0x2a);
    let policy = BroadcasterFeePolicy::default();
    let allow_policy = policy.with_allow_suspicious_broadcasters(true);
    let candidates = public_broadcaster_candidates(
        &[fee_row(1, token, 151, 0.9, "above")],
        1,
        token,
        None,
        SystemTime::now(),
        policy,
        Some(uint!(100_U256)),
    );

    assert!(
        select_public_broadcaster_with_policy(
            &candidates,
            &PublicBroadcasterSelection::Random,
            policy
        )
        .is_err()
    );
    assert!(
        select_public_broadcaster_with_policy(
            &candidates,
            &PublicBroadcasterSelection::Random,
            allow_policy
        )
        .is_ok()
    );
}

#[test]
fn specific_public_broadcaster_drift_rechecks_latest_fee_policy() {
    let token = address(0x2b);
    let railgun_address = sample_railgun_address(41);
    let policy = BroadcasterFeePolicy::default();
    let initial = public_broadcaster_candidates(
        &[fee_row_with_broadcaster_seed(
            1, token, 100, 0.9, "initial", 41,
        )],
        1,
        token,
        None,
        SystemTime::now(),
        policy,
        Some(uint!(100_U256)),
    );
    let selection = PublicBroadcasterSelection::Specific { railgun_address };
    assert!(select_public_broadcaster_with_policy(&initial, &selection, policy).is_ok());

    let drifted = public_broadcaster_candidates(
        &[fee_row_with_broadcaster_seed(
            1, token, 151, 0.9, "drifted", 41,
        )],
        1,
        token,
        None,
        SystemTime::now(),
        policy,
        Some(uint!(100_U256)),
    );
    let error = select_public_broadcaster_with_policy(&drifted, &selection, policy)
        .expect_err("drifted broadcaster should be blocked");
    assert!(error.to_string().contains("outside the allowed range"));
    assert!(
        select_public_broadcaster_with_policy(
            &drifted,
            &selection,
            policy.with_allow_suspicious_broadcasters(true)
        )
        .is_ok()
    );
}

#[test]
fn broadcaster_fee_amount_uses_same_token_fee_rate() {
    let fee = broadcaster_fee_amount(
        uint!(2_000_000_000_000_000_000_U256),
        150_000,
        20_000_000_000,
    );

    assert_eq!(fee, uint!(6_000_000_000_000_000_U256));
}

#[test]
fn railgun_protocol_fee_uses_hardcoded_unshield_bps() {
    let amount = uint!(1_000_000_U256);

    assert_eq!(
        crate::railgun_protocol_fee_amount(amount, RAILGUN_PROTOCOL_FEE_BPS),
        uint!(2_500_U256)
    );
    assert_eq!(
        crate::railgun_protocol_fee_amount(amount, U256::ZERO),
        U256::ZERO
    );
}

#[test]
fn unshield_fee_handling_handles_protocol_fee_for_same_and_different_fee_tokens() {
    let entered = uint!(1_000_000_U256);
    let broadcaster_fee = uint!(400_U256);
    let gross =
        crate::railgun_protocol_gross_amount_for_recipient(entered, RAILGUN_PROTOCOL_FEE_BPS)
            .expect("gross protocol amount");
    let protocol_fee = crate::railgun_protocol_fee_amount(gross, RAILGUN_PROTOCOL_FEE_BPS);

    assert_eq!(gross, uint!(1_002_506_U256));
    assert_eq!(gross - protocol_fee, entered);
    assert_eq!(
        crate::unshield_receiver_amount_for_fee_mode(entered, FeeHandlingMode::DeductFromAmount)
            .expect("deduct unshield receiver amount"),
        entered
    );
    assert_eq!(
        crate::unshield_receiver_amount_for_fee_mode(entered, FeeHandlingMode::AddToAmount)
            .expect("add unshield receiver amount"),
        gross
    );
    assert_eq!(
        crate::unshield_protocol_fee_amount_for_fee_mode(
            entered,
            FeeHandlingMode::DeductFromAmount,
        )
        .expect("deduct protocol fee"),
        uint!(2_500_U256)
    );
    assert_eq!(
        crate::unshield_protocol_fee_amount_for_fee_mode(entered, FeeHandlingMode::AddToAmount,)
            .expect("add protocol fee"),
        gross - entered
    );

    let same_token_add = public_broadcaster_amount_split_for_tokens_and_protocol(
        entered,
        broadcaster_fee,
        FeeHandlingMode::AddToAmount,
        true,
        RAILGUN_PROTOCOL_FEE_BPS,
    )
    .expect("same-token add split");
    assert_eq!(same_token_add.receiver_amount, gross);
    assert_eq!(same_token_add.total_private_spend, gross + broadcaster_fee);
    assert_eq!(same_token_add.fee_mode, FeeHandlingMode::AddToAmount);

    let same_token_deduct = public_broadcaster_amount_split_for_tokens_and_protocol(
        entered,
        broadcaster_fee,
        FeeHandlingMode::DeductFromAmount,
        true,
        RAILGUN_PROTOCOL_FEE_BPS,
    )
    .expect("same-token deduct split");
    assert_eq!(same_token_deduct.receiver_amount, entered - broadcaster_fee);
    assert_eq!(same_token_deduct.total_private_spend, entered);
    assert_eq!(
        same_token_deduct.fee_mode,
        FeeHandlingMode::DeductFromAmount
    );

    let different_token_add = public_broadcaster_amount_split_for_tokens_and_protocol(
        entered,
        broadcaster_fee,
        FeeHandlingMode::AddToAmount,
        false,
        RAILGUN_PROTOCOL_FEE_BPS,
    )
    .expect("different-token add split");
    assert_eq!(different_token_add.receiver_amount, gross);
    assert_eq!(different_token_add.total_private_spend, gross);
    assert_eq!(different_token_add.fee_mode, FeeHandlingMode::AddToAmount);

    let different_token_deduct = public_broadcaster_amount_split_for_tokens_and_protocol(
        entered,
        broadcaster_fee,
        FeeHandlingMode::DeductFromAmount,
        false,
        RAILGUN_PROTOCOL_FEE_BPS,
    )
    .expect("different-token deduct split");
    assert_eq!(different_token_deduct.receiver_amount, entered);
    assert_eq!(different_token_deduct.total_private_spend, entered);
    assert_eq!(
        different_token_deduct.fee_mode,
        FeeHandlingMode::DeductFromAmount
    );

    let max_receiver = uint!(2_000_000_U256);
    assert_eq!(
        public_broadcaster_max_entered_amount_for_tokens_and_protocol(
            max_receiver,
            broadcaster_fee,
            FeeHandlingMode::AddToAmount,
            true,
            RAILGUN_PROTOCOL_FEE_BPS,
        ),
        uint!(1_995_000_U256)
    );
    assert_eq!(
        public_broadcaster_max_entered_amount_for_tokens_and_protocol(
            max_receiver,
            broadcaster_fee,
            FeeHandlingMode::DeductFromAmount,
            true,
            RAILGUN_PROTOCOL_FEE_BPS,
        ),
        max_receiver + broadcaster_fee
    );
    assert_eq!(
        public_broadcaster_max_entered_amount_for_tokens_and_protocol(
            max_receiver,
            broadcaster_fee,
            FeeHandlingMode::DeductFromAmount,
            false,
            RAILGUN_PROTOCOL_FEE_BPS,
        ),
        max_receiver
    );
}

#[test]
fn public_broadcaster_fee_stabilization_accepts_covering_fee() {
    let required = uint!(1_000_U256);

    assert!(broadcaster_fee_covers(required, required));
    assert!(broadcaster_fee_covers(required + uint!(1_U256), required));
    assert!(!broadcaster_fee_covers(required - uint!(1_U256), required));
}

#[test]
fn public_broadcaster_fee_stabilization_buffers_retries() {
    assert_eq!(
        buffered_public_broadcaster_fee(uint!(10_000_U256)),
        uint!(10_100_U256)
    );
    assert_eq!(
        buffered_public_broadcaster_fee(uint!(1_U256)),
        uint!(2_U256)
    );
}

#[test]
fn fee_handling_mode_deducts_or_adds_fee() {
    let entered = uint!(100_U256);
    let fee = uint!(7_U256);

    let deducted = public_broadcaster_amount_split(entered, fee, FeeHandlingMode::DeductFromAmount)
        .expect("deduct split");
    assert_eq!(deducted.receiver_amount, uint!(93_U256));
    assert_eq!(deducted.total_private_spend, entered);

    let added = public_broadcaster_amount_split(entered, fee, FeeHandlingMode::AddToAmount)
        .expect("add split");
    assert_eq!(added.receiver_amount, entered);
    assert_eq!(added.total_private_spend, uint!(107_U256));
}

#[test]
fn different_token_fee_handling_preserves_selected_mode() {
    let entered = uint!(100_U256);
    let fee = uint!(7_U256);

    let deducted = public_broadcaster_amount_split_for_tokens(
        entered,
        fee,
        FeeHandlingMode::DeductFromAmount,
        false,
    )
    .expect("different-token split");

    assert_eq!(deducted.entered_amount, entered);
    assert_eq!(deducted.receiver_amount, entered);
    assert_eq!(deducted.total_private_spend, entered);
    assert_eq!(deducted.fee_amount, fee);
    assert_eq!(deducted.fee_mode, FeeHandlingMode::DeductFromAmount);

    let added = public_broadcaster_amount_split_for_tokens(
        entered,
        fee,
        FeeHandlingMode::AddToAmount,
        false,
    )
    .expect("different-token add split");

    assert_eq!(added.entered_amount, entered);
    assert_eq!(added.receiver_amount, entered);
    assert_eq!(added.total_private_spend, entered);
    assert_eq!(added.fee_amount, fee);
    assert_eq!(added.fee_mode, FeeHandlingMode::AddToAmount);
    assert_eq!(
        public_broadcaster_max_entered_amount_for_tokens(
            uint!(123_U256),
            fee,
            FeeHandlingMode::DeductFromAmount,
            false,
        ),
        uint!(123_U256)
    );
}

#[test]
fn public_broadcaster_build_error_distinguishes_fee_token_balance() {
    let report = public_broadcaster_build_error(
        BuildError::InsufficientFeeTokenBalance(uint!(123_U256)),
        uint!(7_U256),
        FeeHandlingMode::AddToAmount,
        false,
        U256::ZERO,
    );

    assert_eq!(
        report.to_string(),
        "public broadcaster fee-token max spendable: 123; required fee: 7"
    );
}

#[test]
fn fee_handling_mode_rejects_deducting_full_amount() {
    assert!(
        public_broadcaster_amount_split(
            uint!(7_U256),
            uint!(7_U256),
            FeeHandlingMode::DeductFromAmount,
        )
        .is_err()
    );
}

#[test]
fn public_broadcaster_max_entered_amount_depends_on_fee_handling() {
    let max_receiver_amount = uint!(100_U256);
    let fee = uint!(7_U256);

    assert_eq!(
        public_broadcaster_max_entered_amount(
            max_receiver_amount,
            fee,
            FeeHandlingMode::DeductFromAmount,
        ),
        uint!(107_U256)
    );
    assert_eq!(
        public_broadcaster_max_entered_amount(
            max_receiver_amount,
            fee,
            FeeHandlingMode::AddToAmount,
        ),
        max_receiver_amount
    );
}

#[test]
fn public_broadcaster_estimate_preserves_fee_handling_amount_split() {
    let token = address(0x25);
    let broadcaster = eligible_public_broadcasters(
        &[fee_row(
            1,
            token,
            1_000_000_000_000_000_000,
            0.9,
            "fee-mode",
        )],
        1,
        token,
        None,
        SystemTime::now(),
    )
    .into_iter()
    .next()
    .expect("candidate");
    let entered = uint!(1_000_000_000_U256);
    let selected_total = uint!(2_000_000_000_U256);

    let deducted = approximate_public_broadcaster_cost(
        broadcaster.clone(),
        token,
        token,
        entered,
        FeeHandlingMode::DeductFromAmount,
        U256::ZERO,
        100,
        U256::ZERO,
        |_split| {
            let selection = selection_info(selected_total, 1, 1, 2, 0, selected_total);
            Ok(send_approximate_shape(&selection, selected_total))
        },
    )
    .expect("deduct estimate");
    assert_eq!(deducted.entered_amount, entered);
    assert_eq!(deducted.total_private_spend, entered);
    assert_eq!(deducted.receiver_amount + deducted.fee_amount, entered);
    assert_eq!(deducted.protocol_fee_amount, U256::ZERO);
    assert_eq!(deducted.recipient_amount, deducted.receiver_amount);
    assert_eq!(deducted.fee_mode, FeeHandlingMode::DeductFromAmount);

    let added = approximate_public_broadcaster_cost(
        broadcaster,
        token,
        token,
        entered,
        FeeHandlingMode::AddToAmount,
        U256::ZERO,
        100,
        U256::ZERO,
        |_split| {
            let selection = selection_info(selected_total, 1, 1, 2, 0, selected_total);
            Ok(send_approximate_shape(&selection, selected_total))
        },
    )
    .expect("add estimate");
    assert_eq!(added.entered_amount, entered);
    assert_eq!(added.receiver_amount, entered);
    assert_eq!(added.total_private_spend, entered + added.fee_amount);
    assert_eq!(added.protocol_fee_amount, U256::ZERO);
    assert_eq!(added.recipient_amount, added.receiver_amount);
    assert_eq!(added.fee_mode, FeeHandlingMode::AddToAmount);
}

#[test]
fn public_broadcaster_estimate_reports_separate_fee_token_amounts() {
    let action_token = address(0x41);
    let fee_token = address(0x42);
    let broadcaster = eligible_public_broadcasters(
        &[fee_row(
            1,
            fee_token,
            1_000_000_000_000_000_000,
            0.9,
            "separate-fee",
        )],
        1,
        fee_token,
        None,
        SystemTime::now(),
    )
    .into_iter()
    .next()
    .expect("candidate");
    let entered = uint!(1_000_000_000_U256);
    let max_receiver = uint!(2_000_000_000_U256);
    let seed_shape = ApproximateTransactionShape {
        transaction_count: 2,
        input_count: 2,
        private_output_count: 3,
        public_output_count: 0,
        max_receiver_amount: max_receiver,
        relay_call_count: 0,
        uses_relay_adapt: false,
        unwrap_count: 0,
        send: true,
    };
    let initial_fee_amount =
        initial_separate_token_public_broadcaster_fee(&broadcaster, 100, seed_shape);
    let mut observed_fee_amounts = Vec::new();

    let estimate = approximate_public_broadcaster_cost(
        broadcaster,
        action_token,
        fee_token,
        entered,
        FeeHandlingMode::DeductFromAmount,
        U256::ZERO,
        100,
        initial_fee_amount,
        |split| {
            observed_fee_amounts.push(split.fee_amount);
            let selection = selection_info(max_receiver, 2, 2, 3, 0, max_receiver);
            Ok(send_approximate_shape(&selection, max_receiver))
        },
    )
    .expect("separate-token estimate");

    assert_eq!(estimate.action_token, action_token);
    assert_eq!(estimate.fee_token, fee_token);
    assert_eq!(estimate.entered_amount, entered);
    assert_eq!(estimate.receiver_amount, entered);
    assert_eq!(estimate.total_private_spend, entered);
    assert_eq!(estimate.recipient_amount, entered);
    assert_eq!(estimate.fee_mode, FeeHandlingMode::DeductFromAmount);
    assert_eq!(estimate.max_receiver_amount, max_receiver);
    assert_eq!(estimate.max_entered_amount, max_receiver);
    assert_eq!(estimate.transaction_count, 2);
    assert!(!initial_fee_amount.is_zero());
    assert_eq!(observed_fee_amounts.first(), Some(&initial_fee_amount));
    assert!(observed_fee_amounts.iter().all(|fee| !fee.is_zero()));
}

#[test]
fn public_broadcaster_unshield_estimate_includes_protocol_fee() {
    let token = address(0x26);
    let broadcaster = eligible_public_broadcasters(
        &[fee_row(
            1,
            token,
            1_000_000_000_000_000_000,
            0.9,
            "unshield-fee",
        )],
        1,
        token,
        None,
        SystemTime::now(),
    )
    .into_iter()
    .next()
    .expect("candidate");
    let entered = uint!(1_000_000_U256);
    let selected_total = uint!(2_000_000_U256);

    let estimate = approximate_public_broadcaster_cost(
        broadcaster,
        token,
        token,
        entered,
        FeeHandlingMode::AddToAmount,
        RAILGUN_PROTOCOL_FEE_BPS,
        100,
        U256::ZERO,
        |_split| {
            let selection = selection_info(selected_total, 1, 1, 1, 1, selected_total);
            Ok(unshield_approximate_shape(
                &selection,
                selected_total,
                false,
            ))
        },
    )
    .expect("unshield estimate");

    let expected_fee = estimate.receiver_amount * uint!(25_U256) / uint!(10_000_U256);
    assert_eq!(estimate.protocol_fee_bps, RAILGUN_PROTOCOL_FEE_BPS);
    assert_eq!(estimate.receiver_amount, uint!(1_002_506_U256));
    assert_eq!(estimate.protocol_fee_amount, expected_fee);
    assert_eq!(estimate.recipient_amount, entered);
    assert_eq!(
        estimate.total_private_spend,
        estimate.receiver_amount + estimate.fee_amount
    );
}

#[test]
fn approximate_public_broadcaster_gas_tracks_transaction_shape() {
    let base = approximate_public_broadcaster_gas(ApproximateTransactionShape {
        transaction_count: 1,
        input_count: 1,
        private_output_count: 2,
        public_output_count: 0,
        max_receiver_amount: U256::ZERO,
        relay_call_count: 0,
        uses_relay_adapt: false,
        unwrap_count: 0,
        send: true,
    });
    let larger = approximate_public_broadcaster_gas(ApproximateTransactionShape {
        transaction_count: 2,
        input_count: 2,
        private_output_count: 3,
        public_output_count: 1,
        max_receiver_amount: U256::ZERO,
        relay_call_count: 1,
        uses_relay_adapt: true,
        unwrap_count: 1,
        send: false,
    });

    assert!(larger > base);
}

#[test]
fn approximate_public_broadcaster_gas_applies_safety_uplift() {
    let gas = approximate_public_broadcaster_gas(ApproximateTransactionShape {
        transaction_count: 2,
        input_count: 2,
        private_output_count: 4,
        public_output_count: 0,
        max_receiver_amount: U256::ZERO,
        relay_call_count: 0,
        uses_relay_adapt: false,
        unwrap_count: 0,
        send: true,
    });

    assert_eq!(gas, 1_803_200);
}

#[test]
fn public_broadcaster_gas_limit_uses_configured_buffer() {
    assert_eq!(
        public_broadcaster_gas_limit_with_buffer(210_000, 250_000),
        460_000
    );
}

#[test]
fn public_broadcaster_bound_min_gas_price_is_zero_on_arbitrum() {
    assert_eq!(public_broadcaster_bound_min_gas_price(42161, 21_000_000), 0);
    assert_eq!(public_broadcaster_bound_min_gas_price(42170, 21_000_000), 0);
    assert_eq!(
        public_broadcaster_bound_min_gas_price(421_614, 21_000_000),
        0
    );
    assert_eq!(
        public_broadcaster_bound_min_gas_price(1, 21_000_000),
        21_000_000
    );
}

#[test]
fn approximate_shapes_include_broadcaster_fee_output_and_change() {
    let send_selection = selection_info(uint!(15_U256), 2, 1, 3, 0, uint!(13_U256));
    let send = send_approximate_shape(&send_selection, uint!(13_U256));
    assert_eq!(send.input_count, 2);
    assert_eq!(send.transaction_count, 1);
    assert_eq!(send.private_output_count, 3);
    assert_eq!(send.public_output_count, 0);
    assert_eq!(send.relay_call_count, 0);
    assert!(!send.uses_relay_adapt);

    let unshield_selection = selection_info(uint!(12_U256), 1, 1, 1, 1, uint!(10_U256));
    let unshield = unshield_approximate_shape(&unshield_selection, uint!(10_U256), true);
    assert_eq!(unshield.input_count, 1);
    assert_eq!(unshield.transaction_count, 1);
    assert_eq!(unshield.private_output_count, 1);
    assert_eq!(unshield.public_output_count, 1);
    assert_eq!(unshield.relay_call_count, 1);
    assert!(unshield.uses_relay_adapt);
    assert_eq!(unshield.unwrap_count, 1);
}

#[test]
fn approximate_public_broadcaster_gas_counts_each_unwrap_workflow() {
    let shape = ApproximateTransactionShape {
        transaction_count: 2,
        input_count: 2,
        private_output_count: 2,
        public_output_count: 2,
        max_receiver_amount: U256::ZERO,
        relay_call_count: 4,
        uses_relay_adapt: true,
        unwrap_count: 1,
        send: false,
    };
    let one_unwrap = approximate_public_broadcaster_gas(shape);
    let two_unwraps = approximate_public_broadcaster_gas(ApproximateTransactionShape {
        unwrap_count: 2,
        ..shape
    });

    assert!(two_unwraps > one_unwrap);
}

#[test]
fn public_broadcaster_transact_envelope_roundtrips() {
    let (candidate, broadcaster) = sample_public_broadcaster_candidate(9);
    let params = public_broadcaster_transact_params(
        &candidate,
        address(0x33),
        Bytes::from(vec![1, 2, 3, 4]),
        20_000_000_000,
        BTreeMap::new(),
    );

    let encrypted = EncryptedTransactRequest::encrypt_with_seed(
        candidate.viewing_public_key,
        &params,
        [8u8; 32],
    )
    .expect("encrypt request");
    let payload = encrypted.to_transact_payload().expect("serialize envelope");
    let value: serde_json::Value = serde_json::from_slice(&payload).expect("json envelope");
    assert_eq!(value["method"], "transact");
    assert!(value["params"]["encryptedData"].is_array());
    assert_eq!(transact_topic(1), "/railgun/v2/0-1-transact/json");

    let decrypted = try_decrypt_transact_request(
        &broadcaster.viewing_private_key,
        encrypted.pubkey,
        &encrypted.encrypted_data,
    )
    .expect("decrypt request")
    .expect("request for broadcaster");
    assert_eq!(decrypted.params.fees_id.as_deref(), Some("fees-id"));
    assert_eq!(
        decrypted.params.min_gas_price,
        Some(uint!(20_000_000_000_U256))
    );
    assert!(
        decrypted
            .params
            .pre_transaction_pois_per_txid_leaf_per_list
            .is_empty()
    );
}

#[test]
fn public_broadcaster_transact_payload_includes_single_chunk_poi() {
    let (mut candidate, broadcaster) = sample_public_broadcaster_candidate(10);
    let list_key = FixedBytes::from([0x88; 32]);
    let txid_leaf = FixedBytes::from([0x99; 32]);
    candidate.required_poi_list_keys = vec![hex::encode(list_key)];
    let required_keys = candidate
        .parsed_required_poi_list_keys()
        .expect("required list keys");
    let params = public_broadcaster_transact_params(
        &candidate,
        address(0x33),
        Bytes::from(vec![1, 2, 3, 4]),
        20_000_000_000,
        sample_poi_map(&required_keys, &[txid_leaf]),
    );

    let encrypted = EncryptedTransactRequest::encrypt_with_seed(
        candidate.viewing_public_key,
        &params,
        [8u8; 32],
    )
    .expect("encrypt request");
    let decrypted = try_decrypt_transact_request(
        &broadcaster.viewing_private_key,
        encrypted.pubkey,
        &encrypted.encrypted_data,
    )
    .expect("decrypt request")
    .expect("request for broadcaster");

    let per_leaf = decrypted
        .params
        .pre_transaction_pois_per_txid_leaf_per_list
        .get(&list_key)
        .expect("list key");
    assert_eq!(per_leaf.len(), 1);
    assert!(per_leaf.contains_key(&txid_leaf));
}

#[test]
fn public_broadcaster_transact_payload_includes_batched_poi() {
    let (mut candidate, broadcaster) = sample_public_broadcaster_candidate(11);
    let list_keys = [FixedBytes::from([0x81; 32]), FixedBytes::from([0x82; 32])];
    let leaves = [FixedBytes::from([0x91; 32]), FixedBytes::from([0x92; 32])];
    candidate.required_poi_list_keys = list_keys.iter().map(hex::encode).collect();
    let required_keys = candidate
        .parsed_required_poi_list_keys()
        .expect("required list keys");
    let params = public_broadcaster_transact_params(
        &candidate,
        address(0x33),
        Bytes::from(vec![1, 2, 3, 4]),
        20_000_000_000,
        sample_poi_map(&required_keys, &leaves),
    );

    let encrypted = EncryptedTransactRequest::encrypt_with_seed(
        candidate.viewing_public_key,
        &params,
        [8u8; 32],
    )
    .expect("encrypt request");
    let decrypted = try_decrypt_transact_request(
        &broadcaster.viewing_private_key,
        encrypted.pubkey,
        &encrypted.encrypted_data,
    )
    .expect("decrypt request")
    .expect("request for broadcaster");

    let poi_map = decrypted.params.pre_transaction_pois_per_txid_leaf_per_list;
    assert_eq!(poi_map.len(), 2);
    for list_key in list_keys {
        let per_leaf = poi_map.get(&list_key).expect("list key");
        assert_eq!(per_leaf.len(), 2);
        for leaf in leaves {
            assert!(per_leaf.contains_key(&leaf));
        }
    }
}

#[test]
fn public_broadcaster_invalid_poi_list_key_fails_preparation() {
    let (mut candidate, _) = sample_public_broadcaster_candidate(12);
    candidate.required_poi_list_keys = vec!["poi-list".to_string()];

    let error = candidate
        .parsed_required_poi_list_keys()
        .expect_err("invalid POI list key should fail");

    assert!(error.to_string().contains("invalid required POI list key"));
}

#[test]
fn public_broadcaster_response_decodes_tx_hash() {
    let shared_key = [7u8; 32];
    let tx_hash = TxHash::from([3u8; 32]);
    let response = DecryptedTransactResponse::encrypted_tx_hash_message(None, &shared_key, tx_hash)
        .expect("response payload");

    let decoded = decode_public_broadcaster_response(&shared_key, &response)
        .expect("decode response")
        .expect("decryptable response");

    assert_eq!(
        decoded,
        PublicBroadcasterResultKind::Submitted {
            tx_hash: tx_hash.to_string()
        }
    );
}

#[test]
fn public_broadcaster_republish_loop_retries_until_stopped() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build runtime");

    runtime.block_on(async {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (attempt_tx, mut attempt_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(public_broadcaster_republish_loop(
            stop_rx,
            Duration::from_millis(10),
            move |attempt| {
                let attempt_tx = attempt_tx.clone();
                async move {
                    attempt_tx.send(attempt).expect("record attempt");
                    Ok(())
                }
            },
        ));

        let first = tokio::time::timeout(Duration::from_secs(1), attempt_rx.recv())
            .await
            .expect("first retry timed out")
            .expect("first retry attempt");
        let second = tokio::time::timeout(Duration::from_secs(1), attempt_rx.recv())
            .await
            .expect("second retry timed out")
            .expect("second retry attempt");
        let _ = stop_tx.send(());
        handle.await.expect("republish loop joined");

        assert_eq!(first, 2);
        assert_eq!(second, 3);
    });
}

#[test]
fn public_broadcaster_republish_loop_stops_before_first_retry() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build runtime");

    runtime.block_on(async {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (attempt_tx, mut attempt_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(public_broadcaster_republish_loop(
            stop_rx,
            Duration::from_millis(50),
            move |attempt| {
                let attempt_tx = attempt_tx.clone();
                async move {
                    attempt_tx.send(attempt).expect("record attempt");
                    Ok(())
                }
            },
        ));
        let _ = stop_tx.send(());
        handle.await.expect("republish loop joined");

        assert!(attempt_rx.try_recv().is_err());
    });
}
