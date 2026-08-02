use super::helpers::*;

#[test]
fn self_broadcast_top_up_preflight_message_explains_current_gas_requirement() {
    let error = self_broadcast_insufficient_native_gas_error(U256::from(7_u64), U256::from(9_u64));

    let message = self_broadcast_preflight_error_message(&error, true);

    assert!(message.contains("insufficient native gas for self-broadcast"));
    assert!(message.contains("cannot pay for the current outer transaction"));
}

#[test]
fn self_broadcast_transaction_request_sets_outer_evm_fields() {
    let from = address(0x11);
    let to = address(0x22);
    let calldata = Bytes::from_static(&[0xaa, 0xbb, 0xcc]);

    let tx_req = self_broadcast_transaction_request(5, from, to, calldata.clone(), 42, 0, 7);

    assert_eq!(tx_req.chain_id, Some(5));
    assert_eq!(tx_req.from, Some(from));
    assert_eq!(tx_req.to, Some(to.into()));
    assert_eq!(tx_req.max_fee_per_gas, Some(42));
    assert_eq!(tx_req.max_priority_fee_per_gas, Some(0));
    assert_eq!(tx_req.nonce, Some(7));
    assert_eq!(
        tx_req.input.input().expect("self-broadcast input"),
        calldata.as_ref()
    );
}

#[test]
fn self_broadcast_auto_gas_fee_uses_rpc_gas_price_with_min_tip() {
    let quote = SelfBroadcastGasFeeQuote::from_rpc_gas_price(100);
    let resolved = resolve_self_broadcast_gas_fee(SelfBroadcastGasFeeSelection::Auto, quote)
        .expect("resolve auto gas fee");

    assert_eq!(quote.suggested_max_fee_per_gas, 120);
    assert_eq!(quote.suggested_max_priority_fee_per_gas, 1);
    assert_eq!(resolved.rpc_gas_price, 100);
    assert_eq!(resolved.max_fee_per_gas, 120);
    assert_eq!(resolved.max_priority_fee_per_gas, 1);
}

#[test]
fn self_broadcast_fee_samples_ignore_zero_tips_when_non_zero_exists() {
    let samples = [
        SelfBroadcastFeeSample {
            rpc_gas_price: Some(100),
            max_priority_fee_per_gas: Some(0),
            current_base_fee_per_gas: Some(70),
            next_base_fee_per_gas: Some(80),
            priority_fee_rewards: vec![0, 0, 0],
        },
        SelfBroadcastFeeSample {
            rpc_gas_price: Some(110),
            max_priority_fee_per_gas: Some(0),
            current_base_fee_per_gas: Some(75),
            next_base_fee_per_gas: Some(90),
            priority_fee_rewards: vec![0, 5, 7],
        },
    ];

    let quote = self_broadcast_quote_from_fee_samples(&samples).expect("fee quote");

    assert_eq!(quote.suggested_max_priority_fee_per_gas, 5);
    assert_eq!(quote.rpc_gas_price, 110);
    assert_eq!(quote.current_base_fee_per_gas, Some(75));
    assert_eq!(quote.suggested_max_fee_per_gas, 132);
}

#[test]
fn self_broadcast_fee_samples_use_lower_quartile_priority_suggestion() {
    let samples = [10, 20, 30, 40, 50].map(|tip| SelfBroadcastFeeSample {
        rpc_gas_price: Some(100),
        max_priority_fee_per_gas: Some(tip),
        current_base_fee_per_gas: Some(70),
        next_base_fee_per_gas: Some(80),
        priority_fee_rewards: Vec::new(),
    });

    let quote = self_broadcast_quote_from_fee_samples(&samples).expect("fee quote");

    assert_eq!(quote.suggested_max_priority_fee_per_gas, 20);
    assert_eq!(quote.suggested_max_fee_per_gas, 120);
}

#[test]
fn self_broadcast_fee_samples_can_use_rpc_gas_price_as_tip_fallback() {
    let samples = [SelfBroadcastFeeSample {
        rpc_gas_price: Some(100),
        max_priority_fee_per_gas: Some(0),
        current_base_fee_per_gas: None,
        next_base_fee_per_gas: None,
        priority_fee_rewards: vec![0],
    }];

    let default_quote = self_broadcast_quote_from_fee_samples(&samples).expect("fee quote");
    let rpc_fallback_quote = self_broadcast_quote_from_fee_samples_with_tip_fallback(
        &samples,
        SelfBroadcastTipFallback::RpcGasPrice,
    )
    .expect("fee quote with rpc gas price fallback");

    assert_eq!(default_quote.suggested_max_priority_fee_per_gas, 1);
    assert_eq!(rpc_fallback_quote.suggested_max_fee_per_gas, 120);
    assert_eq!(rpc_fallback_quote.suggested_max_priority_fee_per_gas, 100);
}

#[test]
fn self_broadcast_fee_samples_prefer_non_zero_tip_over_rpc_gas_price_fallback() {
    let samples = [SelfBroadcastFeeSample {
        rpc_gas_price: Some(100),
        max_priority_fee_per_gas: Some(5),
        current_base_fee_per_gas: None,
        next_base_fee_per_gas: None,
        priority_fee_rewards: vec![0],
    }];

    let quote = self_broadcast_quote_from_fee_samples_with_tip_fallback(
        &samples,
        SelfBroadcastTipFallback::RpcGasPrice,
    )
    .expect("fee quote");

    assert_eq!(quote.suggested_max_fee_per_gas, 120);
    assert_eq!(quote.suggested_max_priority_fee_per_gas, 5);
}

#[test]
fn self_broadcast_fee_samples_include_fee_history_base_fee_cap() {
    let samples = [SelfBroadcastFeeSample {
        rpc_gas_price: Some(100),
        max_priority_fee_per_gas: Some(1),
        current_base_fee_per_gas: Some(190),
        next_base_fee_per_gas: Some(200),
        priority_fee_rewards: vec![10],
    }];

    let quote = self_broadcast_quote_from_fee_samples(&samples).expect("fee quote");

    assert_eq!(quote.suggested_max_priority_fee_per_gas, 10);
    assert_eq!(quote.suggested_max_fee_per_gas, 250);
}

#[test]
fn eip1559_projection_uses_cushioned_current_base_and_caps_at_max_fee() {
    let quote = SelfBroadcastGasFeeQuote {
        rpc_gas_price: 100,
        current_base_fee_per_gas: Some(100),
        suggested_max_fee_per_gas: 120,
        suggested_max_priority_fee_per_gas: 2,
    };
    let projection = crate::eip1559_gas_cost_projection(10, quote, 1_000, 2);

    assert_eq!(projection.expected_fee_per_gas, 115);
    assert_eq!(projection.expected_cost, U256::from(1_150_u64));
    assert_eq!(projection.maximum_cost, U256::from(10_000_u64));
    assert_eq!(crate::expected_eip1559_fee_per_gas(quote, 110, 2), 110);
}

#[test]
fn eip1559_projection_falls_back_to_max_fee_without_fee_history() {
    let quote = SelfBroadcastGasFeeQuote::from_rpc_gas_price(100);

    assert_eq!(
        crate::expected_eip1559_fee_per_gas(quote, 1_000, 500),
        1_000
    );
    assert_eq!(crate::expected_eip1559_fee_per_gas(quote, 80, 1), 80);
}

#[test]
fn self_broadcast_already_known_classifier_excludes_nonce_errors() {
    for message in [
        "already known",
        "already in mempool",
        "known transaction: 0xabc",
        "transaction already imported",
        "Transaction already exists",
    ] {
        assert!(
            is_self_broadcast_tx_already_known_message(message),
            "expected {message:?} to be classified as already known"
        );
    }

    for message in [
        "nonce too low",
        "replacement transaction underpriced",
        "transaction gas price below minimum",
    ] {
        assert!(
            !is_self_broadcast_tx_already_known_message(message),
            "expected {message:?} to remain retryable"
        );
    }
}

#[test]
fn self_broadcast_custom_gas_fee_validates_caps() {
    assert!(validate_self_broadcast_gas_fee(1, 0).is_ok());
    assert!(validate_self_broadcast_gas_fee(1, 1).is_ok());
    assert!(validate_self_broadcast_gas_fee(0, 0).is_err());
    assert!(validate_self_broadcast_gas_fee(1, 2).is_err());
}

#[test]
fn self_broadcast_replacement_bump_uses_ceil_twelve_point_five_percent() {
    assert_eq!(crate::self_broadcast_replacement_bumped_fee(0), 0);
    assert_eq!(crate::self_broadcast_replacement_bumped_fee(1), 2);
    assert_eq!(crate::self_broadcast_replacement_bumped_fee(8), 9);
    assert_eq!(crate::self_broadcast_replacement_bumped_fee(100), 113);
}

#[test]
fn self_broadcast_gas_cost_uses_max_fee_cap() {
    assert_eq!(self_broadcast_gas_limit_with_buffer(21_000, 5_000), 26_000);
    assert_eq!(self_broadcast_gas_limit_with_buffer(u64::MAX, 1), u64::MAX);
    assert_eq!(
        self_broadcast_native_gas_cost(26_000, 2_000_000_000),
        U256::from(52_000_000_000_000_u128)
    );
}

#[test]
fn self_broadcast_insufficient_gas_error_is_terminal_and_formatted() {
    let error = self_broadcast_insufficient_native_gas_error(U256::from(7_u64), U256::from(9_u64));

    assert!(is_self_broadcast_insufficient_native_gas_error(&error));
    assert_eq!(
        error.to_string(),
        "insufficient native gas for self-broadcast: live balance 7, estimated cost 9"
    );
}

#[test]
fn self_broadcast_pending_spent_hash_parsing_accepts_submitted_tx_hash() {
    let hash = "0x1111111111111111111111111111111111111111111111111111111111111111";

    assert_eq!(
        parse_submitted_tx_hash(hash),
        Some(FixedBytes::from([0x11; 32]))
    );
    assert_eq!(parse_submitted_tx_hash("not-a-hash"), None);
}
