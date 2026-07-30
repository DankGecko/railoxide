use alloy::primitives::{Bytes, U256, address, keccak256};
use wallet_ops::PublicTransactionIntent;
use wallet_ops::vault::PublicAccountSource;

use super::*;

#[test]
fn advanced_public_send_parser_accepts_calls_creation_and_optional_prefix() {
    let to = address!("0x2222222222222222222222222222222222222222");
    let call = parse_advanced_public_send_intent(
        PublicSendKind::ContractCall,
        &to.to_checksum(None),
        "1.5",
        "0x12345678aa",
        Some(18),
    )
    .expect("advanced call");
    assert_eq!(
        call,
        PublicTransactionIntent::Raw {
            to: Some(to),
            value: U256::from(1_500_000_000_000_000_000_u64),
            data: Bytes::from_static(&[0x12, 0x34, 0x56, 0x78, 0xaa]),
        }
    );

    let creation =
        parse_advanced_public_send_intent(PublicSendKind::Deploy, "", "", "60006000", Some(18))
            .expect("advanced creation");
    assert!(matches!(
        creation,
        PublicTransactionIntent::Raw {
            to: None,
            value: U256::ZERO,
            ..
        }
    ));

    let odd = parse_advanced_public_send_intent(
        PublicSendKind::ContractCall,
        &to.to_checksum(None),
        "",
        "0x123",
        Some(18),
    )
    .expect_err("odd hex must fail");
    assert_eq!(odd.field, AdvancedPublicSendField::Data);
    let invalid_to = parse_advanced_public_send_intent(
        PublicSendKind::ContractCall,
        "not-an-address",
        "1",
        "",
        Some(18),
    )
    .expect_err("invalid destination must fail");
    assert_eq!(invalid_to.field, AdvancedPublicSendField::Destination);

    let missing_to =
        parse_advanced_public_send_intent(PublicSendKind::ContractCall, "", "", "", Some(18))
            .expect_err("contract call destination must be explicit");
    assert_eq!(missing_to.field, AdvancedPublicSendField::Destination);

    let missing_init_code = parse_advanced_public_send_intent(
        PublicSendKind::Deploy,
        &to.to_checksum(None),
        "",
        "",
        Some(18),
    )
    .expect_err("deploy init code must be explicit");
    assert_eq!(missing_init_code.field, AdvancedPublicSendField::Data);
}

#[test]
fn advanced_public_send_metadata_and_count_formatting_are_deterministic() {
    let to = address!("0x2222222222222222222222222222222222222222");
    let data = Bytes::from_static(&[0x12, 0x34, 0x56, 0x78, 0xaa]);
    let metadata = advanced_public_send_review_metadata(&PublicTransactionIntent::Raw {
        to: Some(to),
        value: U256::from(7_u64),
        data: data.clone(),
    })
    .expect("advanced metadata");

    assert_eq!(metadata.action_type, "Contract call");
    assert_eq!(metadata.destination, to.to_checksum(None));
    assert_eq!(metadata.data_length, 5);
    assert_eq!(metadata.selector.as_deref(), Some("0x12345678"));
    assert_eq!(
        metadata.data_hash,
        alloy::hex::encode_prefixed(keccak256(&data))
    );
    assert_eq!(metadata.full_data, "0x12345678aa");
    assert_eq!(format_advanced_data_length(1), "1 byte");
    assert_eq!(format_advanced_data_length(2), "2 bytes");
    assert_eq!(format_gas_limit(999), "999");
    assert_eq!(format_gas_limit(1_000), "1,000");
}

#[test]
fn advanced_public_send_hardware_warning_is_added() {
    let software_warnings = advanced_public_send_warnings(PublicAccountSource::Derived);
    assert_eq!(software_warnings.len(), 1);
    assert!(software_warnings[0].contains("Arbitrary transaction data"));

    let hardware_warnings = advanced_public_send_warnings(PublicAccountSource::HardwareDerived);
    assert_eq!(hardware_warnings.len(), 2);
    assert!(hardware_warnings[1].contains("blind signing"));
}

#[test]
fn explicit_review_summaries_cannot_use_cached_password() {
    let ordinary = SpendAuthorizationSummary::new("Send", "Review", Vec::new());
    assert!(spend_authorization_can_use_cached_password(&ordinary));

    let advanced = ordinary.requiring_explicit_review();
    assert!(!spend_authorization_can_use_cached_password(&advanced));
}
