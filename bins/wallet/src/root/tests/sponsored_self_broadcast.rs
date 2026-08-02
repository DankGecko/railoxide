use super::*;
use crate::root::private_action::{
    BLOCK_BUILDER_SPONSORSHIP_LABEL, SelfBroadcastFundingMode, SponsoredAuthorizationDisplay,
    append_sponsorship_authorization_rows, apply_sponsored_authorization_review,
    effective_self_broadcast_funding_mode, sponsored_authorization_display,
    sponsored_funding_choice_visible, sponsored_incentive_from_text,
    sponsored_self_broadcast_availability_reason,
};
use crate::root::spend_authorization::SpendAuthorizationSummaryRow;
use alloy::primitives::FixedBytes;
use wallet_ops::{
    SponsoredActionKind, SponsoredIncentive,
    settings::{build_effective_chain_configs, build_effective_token_registry},
    sponsored_authorization_limit,
};

#[test]
fn sponsored_funding_choice_requires_a_configured_relay() {
    assert!(!sponsored_funding_choice_visible(None));
    assert_eq!(
        effective_self_broadcast_funding_mode(None, SelfBroadcastFundingMode::PrivateSponsorship),
        SelfBroadcastFundingMode::PublicBalance,
    );

    let settings = WalletSettings::default();
    let mut chain = build_effective_chain_configs(&settings)
        .expect("effective settings")
        .remove(&1)
        .expect("Ethereum config");
    assert!(sponsored_funding_choice_visible(Some(&chain)));
    assert_eq!(
        effective_self_broadcast_funding_mode(
            Some(&chain),
            SelfBroadcastFundingMode::PrivateSponsorship,
        ),
        SelfBroadcastFundingMode::PrivateSponsorship,
    );

    chain.sponsored_bundle_relays.clear();
    assert!(!sponsored_funding_choice_visible(Some(&chain)));
    assert_eq!(
        effective_self_broadcast_funding_mode(
            Some(&chain),
            SelfBroadcastFundingMode::PrivateSponsorship,
        ),
        SelfBroadcastFundingMode::PublicBalance,
    );
}

#[test]
fn sponsored_funding_reports_each_static_prerequisite() {
    assert!(sponsored_self_broadcast_availability_reason(None).is_some());

    let settings = WalletSettings::default();
    let mut chain = build_effective_chain_configs(&settings)
        .expect("effective settings")
        .remove(&1)
        .expect("Ethereum config");
    assert_eq!(
        sponsored_self_broadcast_availability_reason(Some(&chain)),
        None
    );

    chain.sponsored_bundle_relays.clear();
    assert!(sponsored_self_broadcast_availability_reason(Some(&chain)).is_some());
    chain = build_effective_chain_configs(&settings)
        .expect("effective settings")
        .remove(&1)
        .expect("Ethereum config");
    chain.wrapped_native_token = None;
    assert!(sponsored_self_broadcast_availability_reason(Some(&chain)).is_some());
    chain.wrapped_native_token = Some("0x0000000000000000000000000000000000000001".into());
    chain.coinbase_payer = None;
    assert!(sponsored_self_broadcast_availability_reason(Some(&chain)).is_some());
}

#[test]
fn custom_incentive_control_accepts_only_integer_percent_bounds() {
    assert_eq!(
        sponsored_incentive_from_text(SponsoredIncentive::Standard, "invalid"),
        Ok(SponsoredIncentive::Standard)
    );
    assert_eq!(
        sponsored_incentive_from_text(SponsoredIncentive::Custom(25), "1"),
        Ok(SponsoredIncentive::Custom(1))
    );
    assert_eq!(
        sponsored_incentive_from_text(SponsoredIncentive::Custom(25), "100"),
        Ok(SponsoredIncentive::Custom(100))
    );
    assert!(sponsored_incentive_from_text(SponsoredIncentive::Custom(25), "0").is_err());
    assert!(sponsored_incentive_from_text(SponsoredIncentive::Custom(25), "101").is_err());
    assert!(sponsored_incentive_from_text(SponsoredIncentive::Custom(25), "1.5").is_err());
}

#[test]
fn sponsorship_summary_requires_fresh_explicit_review_without_warning_notes() {
    let summary = apply_sponsored_authorization_review(
        SpendAuthorizationSummary::new("Provisional", "Review sponsorship", Vec::new()),
        SelfBroadcastFundingMode::PrivateSponsorship,
    );

    assert!(summary.warnings_for_test().is_empty());
    assert!(!spend_authorization_can_use_cached_password(&summary));
}

#[test]
fn sponsorship_summary_contains_only_user_relevant_formatted_maxima() {
    let display = SponsoredAuthorizationDisplay {
        gross_wrapped_native_spend: "Up to 0.0386 WETH · $100.00".to_owned(),
        max_fee_per_gas: "1.02677161 gwei".to_owned(),
        max_priority_fee_per_gas: "0.831059755 gwei".to_owned(),
    };
    let mut rows = Vec::new();

    append_sponsorship_authorization_rows(
        &mut rows,
        SelfBroadcastFundingMode::PrivateSponsorship,
        SponsoredIncentive::Standard,
        Some(&display),
        Some("Account #6"),
    );
    let rows = rows
        .iter()
        .map(SpendAuthorizationSummaryRow::values_for_test)
        .collect::<Vec<_>>();

    assert_eq!(
        rows,
        vec![
            (
                "Gas funding".to_owned(),
                BLOCK_BUILDER_SPONSORSHIP_LABEL.to_owned()
            ),
            ("Builder incentive".to_owned(), "5%".to_owned()),
            ("Transaction signer".to_owned(), "Account #6".to_owned()),
            (
                "Gross sponsorship wrapped-native spend".to_owned(),
                display.gross_wrapped_native_spend,
            ),
            ("Maximum fee per gas".to_owned(), display.max_fee_per_gas),
            (
                "Maximum priority fee per gas".to_owned(),
                display.max_priority_fee_per_gas,
            ),
        ]
    );
}

#[test]
fn sponsorship_display_scales_token_usd_and_gwei_values() {
    let settings = WalletSettings::default();
    let registry = build_effective_token_registry(&settings).expect("effective token registry");
    let cache = TokenAnchorRateCache::new();
    cache.store_native_usd_rate(1, U256::from(3_000_000_000_u64));
    let weth: Address = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
        .parse()
        .expect("WETH address");
    let limit = sponsored_authorization_limit(
        FixedBytes::from([0x30; 32]),
        1_000_000,
        SponsoredActionKind::Send,
        weth,
        Address::from([0x32; 20]),
        Address::from([0x34; 20]),
        1_250_000_000,
        100_000_000,
        U256::ZERO,
        SponsoredIncentive::Standard,
        Address::from([0x33; 20]),
        U256::ZERO,
    )
    .expect("authorization limit");

    let display = sponsored_authorization_display(1, limit, &registry, &cache);

    assert_eq!(
        display.gross_wrapped_native_spend,
        "Up to 0.001344 WETH · $4.03"
    );
    assert_eq!(display.max_fee_per_gas, "1.25 gwei");
    assert_eq!(display.max_priority_fee_per_gas, "0.1 gwei");
}
