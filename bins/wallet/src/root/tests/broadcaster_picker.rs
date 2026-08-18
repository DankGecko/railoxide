use super::*;
use crate::root::broadcaster_picker::{
    BROADCASTER_PICKER_ENTRY_HEIGHT, BroadcasterPickerContent, BroadcasterPickerGroup,
    broadcaster_picker_dialog_vertical_geometry, broadcaster_picker_section_divider_before,
    broadcaster_picker_status_tooltip_revision, broadcaster_picker_status_tooltip_width,
    clear_pending_content_if_current, invalidate_broadcaster_picker_live_update,
    take_pending_broadcaster_picker_live_update, update_broadcaster_picker_group_expansion,
};

fn candidate_with_fee(fee: U256, anchor: Option<U256>) -> PublicBroadcasterCandidate {
    candidate_with_fee_policy(fee, anchor, BroadcasterFeePolicy::default())
}

fn candidate_with_fee_policy(
    fee: U256,
    anchor: Option<U256>,
    policy: BroadcasterFeePolicy,
) -> PublicBroadcasterCandidate {
    let token = Address::from([0x91; 20]);
    let mut row = fee_row(1, token, "picker-status");
    row.fee = fee;
    public_broadcaster_candidates_for_asset(&[row], 1, token, None, policy, anchor)
        .expect("picker candidate")
        .remove(0)
}

fn picker_row(
    id: &str,
    fee: u64,
    status: BroadcasterPickerFeeStatus,
    sort_order: usize,
) -> BroadcasterPickerRow {
    let amount = U256::from(fee * 10);
    let premium_bps = match status {
        BroadcasterPickerFeeStatus::InRange => Some(500),
        BroadcasterPickerFeeStatus::NoPremium => Some(0),
        BroadcasterPickerFeeStatus::LowIncentive => Some(-500),
        BroadcasterPickerFeeStatus::VeryLowIncentive => Some(-2_000),
        BroadcasterPickerFeeStatus::HighFee => Some(6_000),
        BroadcasterPickerFeeStatus::NotAssessed => None,
    };
    BroadcasterPickerRow {
        railgun_address: id.to_string(),
        label: id.to_string(),
        advertised_fee: U256::from(fee),
        premium_bps,
        sort_order,
        estimated_fee_amount: Some(amount),
        estimated_fee_label: format!("{amount} TKN"),
        estimated_fee_usd_micro: Some(amount),
        estimated_fee_usd_label: Some(format!("${amount}")),
        fee_status: status,
        fee_tier: status.tier(),
        show_uncompensated_badge: false,
        fee_status_detail: format!("{} detail", status.tier().label()),
        fee_warning: matches!(
            status,
            BroadcasterPickerFeeStatus::VeryLowIncentive | BroadcasterPickerFeeStatus::HighFee
        )
        .then(|| "Fee outside allowed range".to_string()),
        favorite: false,
        selected: false,
        child_of: None,
    }
}

fn grouped(rows: &[BroadcasterPickerRow]) -> Vec<BroadcasterPickerEntry> {
    project_broadcaster_picker_entries(
        rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &BTreeSet::new(),
        &BTreeMap::new(),
    )
}

fn picker_group(
    entries: &[BroadcasterPickerEntry],
    key: BroadcasterPickerGroupKey,
) -> BroadcasterPickerGroup {
    entries
        .iter()
        .find_map(|entry| match entry {
            BroadcasterPickerEntry::Group(group) if group.key == key => Some(group.clone()),
            _ => None,
        })
        .expect("picker group")
}

fn picker_content(query: &str) -> BroadcasterPickerContent {
    BroadcasterPickerContent {
        entries: Vec::new(),
        empty_message: SharedString::from("No broadcasters"),
        generating: false,
        show_all_broadcasters: false,
        query: query.to_string(),
        selected_address: None,
        view_mode: BroadcasterPickerViewMode::Grouped,
        expanded_groups: BTreeSet::new(),
        collapsed_selected_children: BTreeMap::new(),
    }
}

#[test]
fn picker_fee_status_maps_policy_without_changing_eligibility() {
    let policy = BroadcasterFeePolicy::default();
    let in_range = candidate_with_fee(U256::from(105), Some(U256::from(100)));
    let no_premium = candidate_with_fee(U256::from(100), Some(U256::from(100)));
    let low = candidate_with_fee(U256::from(95), Some(U256::from(100)));
    let very_low = candidate_with_fee(U256::from(80), Some(U256::from(100)));
    let high = candidate_with_fee(U256::from(160), Some(U256::from(100)));
    let not_assessed = candidate_with_fee(U256::from(100), None);

    assert_eq!(
        broadcaster_picker_fee_status(&in_range, policy),
        BroadcasterPickerFeeStatus::InRange
    );
    assert_eq!(
        broadcaster_picker_fee_status(&no_premium, policy),
        BroadcasterPickerFeeStatus::NoPremium
    );
    assert_eq!(
        broadcaster_picker_fee_status(&low, policy),
        BroadcasterPickerFeeStatus::LowIncentive
    );
    assert_eq!(
        broadcaster_picker_fee_status(&very_low, policy),
        BroadcasterPickerFeeStatus::VeryLowIncentive
    );
    assert_eq!(
        broadcaster_picker_fee_status(&high, policy),
        BroadcasterPickerFeeStatus::HighFee
    );
    assert_eq!(
        broadcaster_picker_fee_status(&not_assessed, policy),
        BroadcasterPickerFeeStatus::NotAssessed
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::InRange.tier(),
        BroadcasterPickerTier::Incentivised
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::NoPremium.tier(),
        BroadcasterPickerTier::Uncompensated
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::LowIncentive.tier(),
        BroadcasterPickerTier::Uncompensated
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::VeryLowIncentive.tier(),
        BroadcasterPickerTier::OutsideRange
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::HighFee.tier(),
        BroadcasterPickerTier::OutsideRange
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::NotAssessed.tier(),
        BroadcasterPickerTier::NotAssessed
    );
    assert_eq!(
        BroadcasterPickerTier::Incentivised.badge_label(false),
        Some("Incentivised")
    );
    assert_eq!(
        BroadcasterPickerTier::Uncompensated.badge_label(false),
        None
    );
    assert_eq!(
        BroadcasterPickerTier::Uncompensated.badge_label(true),
        Some("No fee")
    );
    assert!(BroadcasterPickerTier::Uncompensated.is_muted());
    assert!(BroadcasterPickerTier::OutsideRange.is_muted());
    assert!(BroadcasterPickerTier::NotAssessed.is_muted());
    assert!(!BroadcasterPickerTier::Incentivised.is_muted());

    assert!(no_premium.is_allowed_by_fee_policy(policy));
    assert!(low.is_allowed_by_fee_policy(policy));
    assert!(!very_low.is_allowed_by_fee_policy(policy));
    assert!(!high.is_allowed_by_fee_policy(policy));
    assert!(not_assessed.is_allowed_by_fee_policy(policy));
    assert!(very_low.is_allowed_by_fee_policy(policy.with_allow_suspicious_broadcasters(true)));
}

#[test]
fn picker_selection_admission_respects_override_and_rejects_stale_candidate() {
    let policy = BroadcasterFeePolicy::default();
    let candidate = candidate_with_fee(U256::from(160), Some(U256::from(100)));
    let choice = BroadcasterChoice::Specific {
        railgun_address: candidate.railgun_address.clone(),
    };

    assert!(!broadcaster_choice_supported_by_candidates(
        &choice,
        std::slice::from_ref(&candidate),
        policy,
    ));
    let override_policy = policy.with_allow_suspicious_broadcasters(true);
    assert!(broadcaster_choice_supported_by_candidates(
        &choice,
        std::slice::from_ref(&candidate),
        override_policy,
    ));

    let stale_choice = BroadcasterChoice::Specific {
        railgun_address: "missing-broadcaster".to_string(),
    };
    assert!(!broadcaster_choice_supported_by_candidates(
        &stale_choice,
        &[candidate],
        override_policy,
    ));
}

#[test]
fn picker_fee_status_details_use_plain_gas_cost_language() {
    let policy = BroadcasterFeePolicy::default();
    let in_range = candidate_with_fee(U256::from(105), Some(U256::from(100)));
    let no_premium = candidate_with_fee(U256::from(100), Some(U256::from(100)));
    let low = candidate_with_fee(U256::from(95), Some(U256::from(100)));
    let very_low = candidate_with_fee(U256::from(80), Some(U256::from(100)));
    let high = candidate_with_fee(U256::from(160), Some(U256::from(100)));
    let unknown = candidate_with_fee(U256::from(123), None);
    let unrepresentably_high = candidate_with_fee(U256::MAX, Some(U256::ONE));

    assert_eq!(
        broadcaster_picker_fee_status_detail(&in_range, policy),
        "Charges more than the gas it spends, so submitting your transaction earns them something."
    );
    let no_premium_detail = broadcaster_picker_fee_status_detail(&no_premium, policy);
    assert!(no_premium_detail.contains("gas cost or less"));
    assert!(no_premium_detail.contains("earns nothing"));
    assert!(!no_premium_detail.contains("fee anchor"));
    let tiny_positive = candidate_with_fee(U256::from(1_000_001), Some(U256::from(1_000_000)));
    assert_eq!(
        broadcaster_picker_fee_status(&tiny_positive, policy),
        BroadcasterPickerFeeStatus::InRange
    );
    let tiny_positive_detail = broadcaster_picker_fee_status_detail(&tiny_positive, policy);
    assert!(tiny_positive_detail.contains("earns them something"));
    let tiny_negative = candidate_with_fee(U256::from(999_999), Some(U256::from(1_000_000)));
    assert_eq!(
        broadcaster_picker_fee_status(&tiny_negative, policy),
        BroadcasterPickerFeeStatus::LowIncentive
    );
    assert_eq!(
        BroadcasterPickerFeeStatus::LowIncentive.tier(),
        BroadcasterPickerTier::Uncompensated
    );
    let low_detail = broadcaster_picker_fee_status_detail(&low, policy);
    assert!(low_detail.contains("gas cost or less"));
    assert!(!low_detail.contains('%'));
    assert_eq!(
        broadcaster_picker_fee_status_detail(&very_low, policy),
        "This fee is below the allowed range."
    );
    assert_eq!(
        broadcaster_picker_fee_status_detail(&high, policy),
        "This fee is above the allowed range."
    );
    assert_eq!(
        broadcaster_picker_fee_status(&unrepresentably_high, policy),
        BroadcasterPickerFeeStatus::HighFee
    );
    let unrepresentable_detail =
        broadcaster_picker_fee_status_detail(&unrepresentably_high, policy);
    assert!(unrepresentable_detail.contains("outside the allowed range"));
    assert!(unrepresentable_detail.contains("gas-cost comparison is unavailable"));
    assert!(!unrepresentable_detail.contains("above"));
    assert!(!unrepresentable_detail.contains("raw token units"));
    assert!(broadcaster_picker_fee_status_detail(&unknown, policy).contains("123 raw token units"));
    assert!(
        broadcaster_picker_fee_status_detail(&unknown, policy)
            .contains("gas-cost comparison is unavailable")
    );
}

#[test]
fn suspicious_picker_status_uses_policy_boundaries_instead_of_premium_sign() {
    let above_anchor_window = BroadcasterFeePolicy {
        min_anchor_bps: 12_000,
        max_anchor_bps: 15_000,
        allow_suspicious_broadcasters: true,
    };
    let positive_but_below =
        candidate_with_fee_policy(U256::from(110), Some(U256::from(100)), above_anchor_window);
    assert_eq!(
        broadcaster_picker_fee_status(&positive_but_below, above_anchor_window),
        BroadcasterPickerFeeStatus::VeryLowIncentive
    );
    let below_detail =
        broadcaster_picker_fee_status_detail(&positive_but_below, above_anchor_window);
    assert_eq!(below_detail, "This fee is below the allowed range.");

    let below_anchor_window = BroadcasterFeePolicy {
        min_anchor_bps: 5_000,
        max_anchor_bps: 8_000,
        allow_suspicious_broadcasters: true,
    };
    let negative_but_above =
        candidate_with_fee_policy(U256::from(90), Some(U256::from(100)), below_anchor_window);
    assert_eq!(
        broadcaster_picker_fee_status(&negative_but_above, below_anchor_window),
        BroadcasterPickerFeeStatus::HighFee
    );
    let above_detail =
        broadcaster_picker_fee_status_detail(&negative_but_above, below_anchor_window);
    assert_eq!(above_detail, "This fee is above the allowed range.");
}

#[test]
fn picker_fee_text_colors_preserve_each_cell_hierarchy() {
    assert_eq!(
        broadcaster_picker_fee_text_colors(false),
        (ui::theme::TEXT, ui::theme::TEXT_MUTED)
    );
    assert_eq!(
        broadcaster_picker_fee_text_colors(true),
        (ui::theme::TEXT_MUTED, ui::theme::TEXT_SUBTLE)
    );
}

#[test]
fn grouped_projection_orders_tiers_and_groups_the_entire_uncompensated_population() {
    let rows = vec![
        picker_row("low", 1, BroadcasterPickerFeeStatus::LowIncentive, 0),
        picker_row("positive-b", 10, BroadcasterPickerFeeStatus::InRange, 1),
        picker_row("zero", 8, BroadcasterPickerFeeStatus::NoPremium, 2),
        picker_row("positive-a", 5, BroadcasterPickerFeeStatus::InRange, 3),
        picker_row(
            "below-a",
            2,
            BroadcasterPickerFeeStatus::VeryLowIncentive,
            4,
        ),
        picker_row(
            "below-b",
            3,
            BroadcasterPickerFeeStatus::VeryLowIncentive,
            5,
        ),
        picker_row("unknown", 4, BroadcasterPickerFeeStatus::NotAssessed, 6),
    ];
    let entries = grouped(&rows);
    let uncompensated_key = BroadcasterPickerGroupKey::Tier(BroadcasterPickerTier::Uncompensated);

    assert!(matches!(
        entries.first(),
        Some(BroadcasterPickerEntry::Broadcaster(row))
            if row.railgun_address == "positive-a"
    ));
    assert!(matches!(
        entries.get(1),
        Some(BroadcasterPickerEntry::Broadcaster(row))
            if row.railgun_address == "positive-b"
    ));
    let uncompensated = picker_group(&entries, uncompensated_key);
    assert_eq!(uncompensated.count, 2);
    assert_eq!(uncompensated.fee_tier, BroadcasterPickerTier::Uncompensated);
    assert_eq!(uncompensated.label, "2 broadcasters earning no fee");
    assert_eq!(uncompensated.estimated_fee_label, "from 10 TKN");
    assert_eq!(
        uncompensated.estimated_fee_usd_label.as_deref(),
        Some("from $10")
    );
    assert!(uncompensated.detail.contains("gas cost or less"));
    assert!(!uncompensated.detail.contains("fee anchor"));
    assert_eq!(
        entries.iter().position(|entry| matches!(
            entry,
            BroadcasterPickerEntry::Group(group) if group.key == uncompensated_key
        )),
        Some(2)
    );
    let expanded = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &BTreeSet::from([uncompensated_key]),
        &BTreeMap::new(),
    );
    assert_eq!(
        expanded
            .iter()
            .filter_map(|entry| match entry {
                BroadcasterPickerEntry::Broadcaster(row)
                    if row.child_of == Some(uncompensated_key) =>
                {
                    Some(row.railgun_address.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["low", "zero"]
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        BroadcasterPickerEntry::Group(group)
            if group.key == BroadcasterPickerGroupKey::Status(
                BroadcasterPickerFeeStatus::VeryLowIncentive
            ) && group.fee_tier == BroadcasterPickerTier::OutsideRange
                && group.label == "2 broadcasters outside the allowed range"
    )));
    assert!(matches!(
        entries.last(),
        Some(BroadcasterPickerEntry::Broadcaster(row))
            if row.railgun_address == "unknown"
    ));
}

#[test]
fn uncompensated_singleton_stays_grouped_and_positive_same_rates_stay_direct() {
    let rows = vec![
        picker_row("positive-a", 10, BroadcasterPickerFeeStatus::InRange, 0),
        picker_row("positive-b", 10, BroadcasterPickerFeeStatus::InRange, 1),
        picker_row("positive-c", 10, BroadcasterPickerFeeStatus::InRange, 2),
        picker_row("positive-d", 10, BroadcasterPickerFeeStatus::InRange, 3),
        picker_row("zero", 10, BroadcasterPickerFeeStatus::NoPremium, 4),
    ];
    let entries = grouped(&rows);
    let groups = entries
        .iter()
        .filter_map(|entry| match entry {
            BroadcasterPickerEntry::Group(group) => Some(group),
            BroadcasterPickerEntry::Broadcaster(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].key,
        BroadcasterPickerGroupKey::Tier(BroadcasterPickerTier::Uncompensated)
    );
    assert_eq!(groups[0].label, "1 broadcaster earning no fee");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, BroadcasterPickerEntry::Broadcaster(_)))
            .count(),
        4
    );
}

#[test]
fn outside_range_groups_keep_directional_plain_language() {
    let below = vec![
        picker_row("a", 5, BroadcasterPickerFeeStatus::VeryLowIncentive, 0),
        picker_row("b", 7, BroadcasterPickerFeeStatus::VeryLowIncentive, 1),
    ];
    let below_group = picker_group(
        &grouped(&below),
        BroadcasterPickerGroupKey::Status(BroadcasterPickerFeeStatus::VeryLowIncentive),
    );
    assert_eq!(
        below_group.detail,
        "These fees are below the allowed range."
    );
    assert_eq!(below_group.estimated_fee_label, "from 50 TKN");
    assert_eq!(
        below_group.estimated_fee_usd_label.as_deref(),
        Some("from $50")
    );

    let mut below_with_unavailable = below;
    below_with_unavailable[1].estimated_fee_amount = None;
    below_with_unavailable[1].estimated_fee_label = "Retrying...".to_string();
    below_with_unavailable[1].estimated_fee_usd_micro = None;
    below_with_unavailable[1].estimated_fee_usd_label = None;
    let unavailable_below_group = picker_group(
        &grouped(&below_with_unavailable),
        BroadcasterPickerGroupKey::Status(BroadcasterPickerFeeStatus::VeryLowIncentive),
    );
    assert_eq!(unavailable_below_group.estimated_fee_label, "Retrying...");
    assert_eq!(unavailable_below_group.estimated_fee_usd_label, None);

    let mut high = vec![
        picker_row("a", 15, BroadcasterPickerFeeStatus::HighFee, 0),
        picker_row("b", 20, BroadcasterPickerFeeStatus::HighFee, 1),
    ];
    high[1].premium_bps = None;
    let key = BroadcasterPickerGroupKey::Status(BroadcasterPickerFeeStatus::HighFee);
    let mixed_detail = picker_group(&grouped(&high), key).detail;
    assert!(mixed_detail.contains("above the allowed range"));
    assert!(mixed_detail.contains("some comparisons are unavailable"));
    assert!(!mixed_detail.contains('%'));

    high[0].premium_bps = None;
    let unavailable_detail = picker_group(&grouped(&high), key).detail;
    assert!(unavailable_detail.contains("gas-cost comparisons are unavailable"));
    assert!(!unavailable_detail.contains("above the allowed range"));
}

#[test]
fn selected_group_collapse_remains_collapsed_for_same_revision() {
    let mut rows = vec![
        picker_row("a", 8, BroadcasterPickerFeeStatus::NoPremium, 0),
        picker_row("b", 9, BroadcasterPickerFeeStatus::LowIncentive, 1),
        picker_row("c", 10, BroadcasterPickerFeeStatus::NoPremium, 2),
    ];
    rows[2].selected = true;
    let entries = grouped(&rows);
    let group = entries
        .iter()
        .find_map(|entry| match entry {
            BroadcasterPickerEntry::Group(group) => Some(group),
            BroadcasterPickerEntry::Broadcaster(_) => None,
        })
        .expect("uncompensated group");
    assert!(group.expanded);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        BroadcasterPickerEntry::Broadcaster(row)
            if row.railgun_address == "c"
                && row.child_of == Some(group.key)
                && row.selected
    )));

    let mut expanded_groups = BTreeSet::new();
    let mut collapsed_selected_children = BTreeMap::new();
    update_broadcaster_picker_group_expansion(
        &mut expanded_groups,
        &mut collapsed_selected_children,
        group.key,
        true,
        Some("c".to_string()),
        group.revision.clone(),
    );
    let collapsed = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &expanded_groups,
        &collapsed_selected_children,
    );
    assert_eq!(
        collapsed
            .iter()
            .filter(|entry| matches!(entry, BroadcasterPickerEntry::Broadcaster(_)))
            .count(),
        0
    );
}

#[test]
fn selected_group_reexpands_after_estimate_status_or_membership_revision() {
    let mut rows = vec![
        picker_row("a", 5, BroadcasterPickerFeeStatus::LowIncentive, 0),
        picker_row("b", 7, BroadcasterPickerFeeStatus::LowIncentive, 1),
    ];
    rows[1].selected = true;
    let key = BroadcasterPickerGroupKey::Tier(BroadcasterPickerTier::Uncompensated);
    let initial_entries = grouped(&rows);
    let initial_group = picker_group(&initial_entries, key);
    let mut expanded_groups = BTreeSet::new();
    let mut collapsed_selected_children = BTreeMap::new();
    update_broadcaster_picker_group_expansion(
        &mut expanded_groups,
        &mut collapsed_selected_children,
        key,
        true,
        Some("b".to_string()),
        initial_group.revision.clone(),
    );

    let assert_reexpanded = |revised_rows: &[BroadcasterPickerRow]| {
        let entries = project_broadcaster_picker_entries(
            revised_rows,
            BroadcasterPickerViewMode::Grouped,
            false,
            &expanded_groups,
            &collapsed_selected_children,
        );
        let group = picker_group(&entries, key);
        assert_ne!(group.revision, initial_group.revision);
        assert!(group.expanded);
        assert!(entries.iter().any(|entry| matches!(
            entry,
            BroadcasterPickerEntry::Broadcaster(row)
                if row.railgun_address == "b" && row.selected
        )));
    };

    let mut estimate_changed = rows.clone();
    estimate_changed[0].estimated_fee_amount = Some(U256::from(999));
    estimate_changed[0].estimated_fee_label = "999 TKN".to_string();
    assert_reexpanded(&estimate_changed);

    let mut premium_changed = rows.clone();
    premium_changed[0].premium_bps = Some(-600);
    assert_reexpanded(&premium_changed);

    let mut status_presentation_changed = rows.clone();
    status_presentation_changed[0].fee_status_detail = "Revised status detail".to_string();
    status_presentation_changed[0].fee_warning = Some("Revised warning".to_string());
    assert_reexpanded(&status_presentation_changed);

    let mut assessment_changed = rows.clone();
    assessment_changed[0].fee_status = BroadcasterPickerFeeStatus::NoPremium;
    assert_reexpanded(&assessment_changed);

    let mut favorite_changed = rows.clone();
    favorite_changed[0].favorite = true;
    assert_reexpanded(&favorite_changed);

    let mut membership_changed = rows.clone();
    membership_changed.push(picker_row(
        "c",
        9,
        BroadcasterPickerFeeStatus::LowIncentive,
        2,
    ));
    assert_reexpanded(&membership_changed);

    for row in &mut rows {
        row.fee_status = BroadcasterPickerFeeStatus::VeryLowIncentive;
        row.fee_tier = BroadcasterPickerTier::OutsideRange;
        row.fee_status_detail = "Below allowed range".to_string();
        row.fee_warning = Some("Fee outside allowed range".to_string());
    }
    let revised_key =
        BroadcasterPickerGroupKey::Status(BroadcasterPickerFeeStatus::VeryLowIncentive);
    let status_changed = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &expanded_groups,
        &collapsed_selected_children,
    );
    let status_changed_group = picker_group(&status_changed, revised_key);
    assert_ne!(status_changed_group.revision, initial_group.revision);
    assert!(status_changed_group.expanded);
    assert!(status_changed.iter().any(|entry| matches!(
        entry,
        BroadcasterPickerEntry::Broadcaster(row)
            if row.railgun_address == "b" && row.selected
    )));
}

#[test]
fn ordinary_group_collapse_does_not_hide_a_later_selected_child() {
    let mut rows = vec![
        picker_row("a", 10, BroadcasterPickerFeeStatus::NoPremium, 0),
        picker_row("b", 11, BroadcasterPickerFeeStatus::LowIncentive, 1),
    ];
    let key = BroadcasterPickerGroupKey::Tier(BroadcasterPickerTier::Uncompensated);
    let group_revision = picker_group(&grouped(&rows), key).revision;
    let mut expanded_groups = BTreeSet::from([key]);
    let mut collapsed_selected_children = BTreeMap::new();

    update_broadcaster_picker_group_expansion(
        &mut expanded_groups,
        &mut collapsed_selected_children,
        key,
        true,
        None,
        group_revision,
    );
    assert!(!expanded_groups.contains(&key));
    assert!(!collapsed_selected_children.contains_key(&key));

    rows[1].selected = true;
    let entries = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &expanded_groups,
        &collapsed_selected_children,
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        BroadcasterPickerEntry::Group(group) if group.key == key && group.expanded
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        BroadcasterPickerEntry::Broadcaster(row) if row.railgun_address == "b" && row.selected
    )));
}

#[test]
fn search_flattens_groups_keeps_tier_order_and_hides_the_divider() {
    let rows = vec![
        picker_row(
            "uncompensated",
            1,
            BroadcasterPickerFeeStatus::LowIncentive,
            0,
        ),
        picker_row("incentivised", 10, BroadcasterPickerFeeStatus::InRange, 1),
    ];
    let entries = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        true,
        &BTreeSet::new(),
        &BTreeMap::new(),
    );

    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, BroadcasterPickerEntry::Group(_)))
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, BroadcasterPickerEntry::Broadcaster(_)))
            .count(),
        rows.len()
    );
    assert!(matches!(
        entries.first(),
        Some(BroadcasterPickerEntry::Broadcaster(row))
            if row.railgun_address == "incentivised"
    ));
    assert!(
        !(0..entries.len()).any(|row| broadcaster_picker_section_divider_before(
            &entries,
            BroadcasterPickerViewMode::Grouped,
            row,
        ))
    );
}

#[test]
fn list_mode_uses_global_fee_order_and_preserves_row_state() {
    let mut rows = vec![
        picker_row(
            "outside-cheapest",
            4,
            BroadcasterPickerFeeStatus::HighFee,
            0,
        ),
        picker_row(
            "uncompensated-tie",
            5,
            BroadcasterPickerFeeStatus::LowIncentive,
            1,
        ),
        picker_row("outside-tie", 5, BroadcasterPickerFeeStatus::HighFee, 2),
        picker_row(
            "incentivised-tie",
            5,
            BroadcasterPickerFeeStatus::InRange,
            3,
        ),
    ];
    rows[0].selected = true;
    rows[1].favorite = true;
    let entries = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::List,
        false,
        &BTreeSet::new(),
        &BTreeMap::new(),
    );
    let listed = entries
        .iter()
        .map(|entry| match entry {
            BroadcasterPickerEntry::Broadcaster(row) => row,
            BroadcasterPickerEntry::Group(_) => {
                panic!("list mode must only contain broadcasters")
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        listed
            .iter()
            .map(|row| row.railgun_address.as_str())
            .collect::<Vec<_>>(),
        vec![
            "outside-cheapest",
            "incentivised-tie",
            "uncompensated-tie",
            "outside-tie"
        ]
    );
    assert!(listed[0].selected);
    assert!(listed[0].fee_warning.is_some());
    assert!(!listed[0].show_uncompensated_badge);
    assert!(listed[2].show_uncompensated_badge);
    assert!(listed[2].favorite);
    assert!(
        listed
            .iter()
            .filter(|row| row.favorite)
            .all(|row| row.railgun_address == "uncompensated-tie")
    );
    assert_eq!(
        BroadcasterPickerViewMode::default(),
        BroadcasterPickerViewMode::Grouped
    );
}

#[test]
fn grouped_mode_restores_expansion_and_formats_available_ranges_only() {
    let rows = vec![
        picker_row("a", 5, BroadcasterPickerFeeStatus::LowIncentive, 0),
        picker_row("b", 7, BroadcasterPickerFeeStatus::LowIncentive, 1),
    ];
    let key = BroadcasterPickerGroupKey::Tier(BroadcasterPickerTier::Uncompensated);
    let entries = project_broadcaster_picker_entries(
        &rows,
        BroadcasterPickerViewMode::Grouped,
        false,
        &BTreeSet::from([key]),
        &BTreeMap::new(),
    );
    let group = entries
        .iter()
        .find_map(|entry| match entry {
            BroadcasterPickerEntry::Group(group) => Some(group),
            BroadcasterPickerEntry::Broadcaster(_) => None,
        })
        .expect("expanded status group");
    assert!(group.expanded);
    assert_eq!(group.estimated_fee_label, "from 50 TKN");
    assert_eq!(group.estimated_fee_usd_label.as_deref(), Some("from $50"));

    let mut unavailable = rows.clone();
    for row in &mut unavailable {
        row.estimated_fee_amount = None;
        row.estimated_fee_label = "Estimate unavailable".to_string();
        row.estimated_fee_usd_micro = None;
        row.estimated_fee_usd_label = None;
    }
    assert_eq!(
        group_minimum_estimated_fee_labels(&unavailable),
        ("Estimate unavailable".to_string(), None)
    );
    let mut partially_unavailable = rows.clone();
    partially_unavailable[1].estimated_fee_amount = None;
    partially_unavailable[1].estimated_fee_label = "Retrying...".to_string();
    partially_unavailable[1].estimated_fee_usd_micro = None;
    partially_unavailable[1].estimated_fee_usd_label = None;
    assert_eq!(
        group_minimum_estimated_fee_labels(&partially_unavailable),
        ("Retrying...".to_string(), None)
    );
    let mut no_usd = rows;
    for row in &mut no_usd {
        row.estimated_fee_usd_micro = None;
        row.estimated_fee_usd_label = None;
    }
    assert_eq!(
        group_minimum_estimated_fee_labels(&no_usd),
        ("from 50 TKN".to_string(), None)
    );
}

#[test]
fn picker_entries_use_uniform_list_height() {
    assert_eq!(BROADCASTER_PICKER_ENTRY_HEIGHT, px(84.0));
    let mut rows = vec![
        picker_row("a", 10, BroadcasterPickerFeeStatus::InRange, 0),
        picker_row("b", 8, BroadcasterPickerFeeStatus::NoPremium, 1),
        picker_row("c", 9, BroadcasterPickerFeeStatus::LowIncentive, 2),
    ];
    rows[1].selected = true;
    let projected = grouped(&rows);
    let entries = [
        projected
            .iter()
            .find(|entry| matches!(entry, BroadcasterPickerEntry::Group(_)))
            .expect("group entry")
            .clone(),
        projected
            .iter()
            .find(|entry| matches!(entry, BroadcasterPickerEntry::Broadcaster(_)))
            .expect("broadcaster entry")
            .clone(),
    ];

    assert!(
        entries
            .iter()
            .all(|_| BroadcasterPickerEntry::height() == BROADCASTER_PICKER_ENTRY_HEIGHT)
    );
}

#[test]
fn grouped_tier_divider_only_precedes_the_uncompensated_summary() {
    let mixed = grouped(&[
        picker_row("incentivised", 10, BroadcasterPickerFeeStatus::InRange, 0),
        picker_row("uncompensated", 5, BroadcasterPickerFeeStatus::NoPremium, 1),
        picker_row(
            "outside",
            20,
            BroadcasterPickerFeeStatus::VeryLowIncentive,
            2,
        ),
    ]);
    assert_eq!(mixed.len(), 3);
    assert!(!broadcaster_picker_section_divider_before(
        &mixed,
        BroadcasterPickerViewMode::Grouped,
        0,
    ));
    assert!(broadcaster_picker_section_divider_before(
        &mixed,
        BroadcasterPickerViewMode::Grouped,
        1,
    ));
    assert!(!broadcaster_picker_section_divider_before(
        &mixed,
        BroadcasterPickerViewMode::List,
        1,
    ));
    assert!(!broadcaster_picker_section_divider_before(
        &mixed,
        BroadcasterPickerViewMode::Grouped,
        2,
    ));

    let incentivised_only = grouped(&[picker_row(
        "positive",
        20,
        BroadcasterPickerFeeStatus::InRange,
        0,
    )]);
    let uncompensated_only = grouped(&[picker_row(
        "zero",
        10,
        BroadcasterPickerFeeStatus::NoPremium,
        0,
    )]);
    assert!(!(0..incentivised_only.len()).any(|row| {
        broadcaster_picker_section_divider_before(
            &incentivised_only,
            BroadcasterPickerViewMode::Grouped,
            row,
        )
    }));
    assert!(!(0..uncompensated_only.len()).any(|row| {
        broadcaster_picker_section_divider_before(
            &uncompensated_only,
            BroadcasterPickerViewMode::Grouped,
            row,
        )
    }));
}

#[test]
fn matching_live_content_clears_an_older_pending_update() {
    let current = picker_content("a");
    let queued = picker_content("b");
    let incoming = current.clone();
    let mut pending = Some(queued.clone());

    assert!(clear_pending_content_if_current(
        current == incoming,
        &mut pending
    ));
    assert!(pending.is_none());

    pending = Some(queued.clone());
    assert!(!clear_pending_content_if_current(false, &mut pending));
    assert!(pending.as_ref() == Some(&queued));
}

#[test]
fn picker_fee_estimate_retry_state_deduplicates_and_rejects_stale_timers() {
    let mut retry = BroadcasterPickerFeeEstimateRetryState::default();
    assert!(retry.should_schedule(false, false, false));
    assert_eq!(retry.mark_scheduled(7), Duration::from_secs(1));
    assert!(retry.is_scheduled());
    assert!(!retry.should_schedule(false, false, false));
    assert!(!retry.clear_if_current(6));
    assert!(retry.is_scheduled());
    assert!(retry.clear_if_current(7));
    assert!(!retry.is_scheduled());

    assert_eq!(retry.mark_scheduled(8), Duration::from_secs(2));
    retry.finish_attempt(false);
    assert_eq!(retry.mark_scheduled(9), Duration::from_secs(4));
    retry.finish_attempt(true);
    assert!(!retry.clear_if_current(9));
    assert!(retry.should_schedule(false, false, false));
    assert!(!retry.should_schedule(true, false, false));
    assert!(!retry.should_schedule(false, true, false));
    assert!(retry.should_schedule(false, true, true));
    assert_eq!(retry.mark_scheduled(10), Duration::from_secs(1));
}

#[test]
fn scroll_for_more_visibility_tracks_remaining_list_offset() {
    assert!(!broadcaster_picker_scroll_hint_visible(px(0.0), px(0.0)));
    assert!(broadcaster_picker_scroll_hint_visible(px(0.0), px(100.0)));
    assert!(broadcaster_picker_scroll_hint_visible(px(-50.0), px(100.0)));
    assert!(!broadcaster_picker_scroll_hint_visible(
        px(-99.5),
        px(100.0)
    ));
    assert!(!broadcaster_picker_scroll_hint_visible(
        px(-100.0),
        px(100.0)
    ));
}

#[test]
fn picker_dialog_height_preserves_symmetric_available_margins_without_a_cap() {
    for (available_height, expected_margin, expected_height) in
        [(800.0, 80.0, 640.0), (1_600.0, 160.0, 1_280.0)]
    {
        let available_height = px(available_height);
        let (margin, dialog_height) = broadcaster_picker_dialog_vertical_geometry(available_height);

        assert_eq!(margin, px(expected_margin));
        assert_eq!(dialog_height, px(expected_height));
        assert_eq!(margin * 2.0 + dialog_height, available_height);
    }
}

#[test]
fn status_tooltip_width_is_capped_and_yields_to_available_viewport_width() {
    assert_eq!(
        broadcaster_picker_status_tooltip_width(px(1_000.0), px(16.0)),
        Some(px(320.0))
    );
    assert_eq!(
        broadcaster_picker_status_tooltip_width(px(300.0), px(16.0)),
        Some(px(258.0))
    );
    assert_eq!(
        broadcaster_picker_status_tooltip_width(px(42.0), px(16.0)),
        None
    );
}

#[test]
fn status_tooltip_revision_changes_with_live_status_content() {
    let current = broadcaster_picker_status_tooltip_revision(
        BroadcasterPickerTier::Incentivised,
        "Current detail",
    );
    assert_eq!(
        current,
        broadcaster_picker_status_tooltip_revision(
            BroadcasterPickerTier::Incentivised,
            "Current detail"
        )
    );
    assert_ne!(
        current,
        broadcaster_picker_status_tooltip_revision(
            BroadcasterPickerTier::Incentivised,
            "Updated detail"
        )
    );
    assert_ne!(
        current,
        broadcaster_picker_status_tooltip_revision(
            BroadcasterPickerTier::OutsideRange,
            "Current detail"
        )
    );
}

#[test]
fn synchronous_live_update_invalidation_advances_epoch_and_clears_schedule() {
    let mut epoch = 4;
    let mut scheduled = true;

    invalidate_broadcaster_picker_live_update(&mut epoch, &mut scheduled);

    assert_eq!(epoch, 5);
    assert!(!scheduled);
}

#[test]
fn stale_live_update_callback_preserves_newer_pending_cycle() {
    let mut scheduled = true;
    let mut pending = Some("newer content");

    assert_eq!(
        take_pending_broadcaster_picker_live_update(4, 5, &mut scheduled, &mut pending),
        None
    );
    assert!(scheduled);
    assert_eq!(pending, Some("newer content"));
}

#[test]
fn current_live_update_callback_consumes_pending_content() {
    let mut scheduled = true;
    let mut pending = Some("current content");

    assert_eq!(
        take_pending_broadcaster_picker_live_update(5, 5, &mut scheduled, &mut pending),
        Some("current content")
    );
    assert!(!scheduled);
    assert!(pending.is_none());
}
