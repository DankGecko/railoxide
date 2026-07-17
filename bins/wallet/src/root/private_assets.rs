use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::primitives::{Address, U256};
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Styled,
    Window, div, img, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{
    Disableable, Icon, IconName, Sizable, WindowExt, button::ButtonVariants,
    scroll::ScrollableElement,
};
use railgun_ui::{format_token_amount, format_usd_micro_value, short_address};
use ui::controls::{app_button, app_button_base, app_muted_text, app_strong_text};
use ui::theme::{self};
use wallet_ops::{
    ListUtxosOutput, TokenAnchorRateCache, TokenTotal,
    max_send_amount_from_outputs as planner_max_send_amount_from_outputs,
    max_unshield_amount_from_outputs as planner_max_unshield_amount_from_outputs,
    settings::EffectiveTokenRegistry,
};

use crate::assets::{RailgunActionIcon, WalletIconSource};

use super::chain_load::loading_summary;
use super::public_account::render_public_address_qr_dialog_content;
use super::utxo::{
    UtxoDisplayRow, blocked_shield_refund_action_available, blocked_shield_refund_origin_resolving,
    blocked_shield_rescue_display_rows, recoverable_poi_candidate_count, short_hash,
};
use super::{
    ChainUtxoState, PUBLIC_ADDRESS_QR_DIALOG_WIDTH, UnshieldAsset, WalletRoot, centered_message,
    dialog_content_max_height, dialog_max_height, parse_address, rgb_with_alpha,
    scrollable_dialog_content, secondary_dialog_content_width, token_display_metadata,
};

#[cfg(test)]
use super::UnshieldAssetKey;

#[derive(Clone)]
pub(super) struct FormattedTokenTotal {
    pub(super) chain_id: u64,
    pub(super) token: Option<Address>,
    pub(super) label: String,
    pub(super) amount: String,
    pub(super) usd_micro_amount: Option<U256>,
    pub(super) usd_amount: Option<String>,
    pub(super) pending_poi_amount: String,
    pub(super) pending_incoming_amount: String,
    pub(super) pending_outgoing_amount: String,
    pub(super) total: Option<U256>,
    pub(super) poi_verified_total: Option<U256>,
    pub(super) pending_poi_total: Option<U256>,
    pub(super) pending_incoming_total: Option<U256>,
    pub(super) pending_outgoing_total: Option<U256>,
    pub(super) decimals: Option<u8>,
    pub(super) icon_path: Option<WalletIconSource>,
}

#[derive(Clone)]
struct PrivatePendingAssetLine {
    label: String,
    amount: String,
}

#[derive(Clone)]
struct PrivatePendingSummary {
    blocked_shield_rows: Vec<UtxoDisplayRow>,
    pending_poi_assets: Vec<PrivatePendingAssetLine>,
    pending_incoming_assets: Vec<PrivatePendingAssetLine>,
    pending_outgoing_assets: Vec<PrivatePendingAssetLine>,
    pending_incoming_outputs: usize,
    pending_outgoing_outputs: usize,
    recoverable_poi_outputs: usize,
    poi_refreshing: bool,
    poi_refresh_session: Option<Arc<wallet_ops::WalletSession>>,
}

pub(super) fn max_unshield_amount_from_snapshot(
    snapshot: &ListUtxosOutput,
    token: Address,
) -> U256 {
    planner_max_unshield_amount_from_outputs(&snapshot.utxos, token)
}

pub(super) fn max_send_amount_from_snapshot(snapshot: &ListUtxosOutput, token: Address) -> U256 {
    planner_max_send_amount_from_outputs(&snapshot.utxos, token)
}

pub(super) fn format_private_asset_rows_from_snapshot(
    snapshot: &ListUtxosOutput,
    registry: Option<&EffectiveTokenRegistry>,
    anchor_cache: Option<&TokenAnchorRateCache>,
) -> Vec<FormattedTokenTotal> {
    let mut rows =
        format_private_asset_rows(snapshot.chain_id, &snapshot.totals, registry, anchor_cache);
    apply_pending_asset_amounts(snapshot, registry, anchor_cache, &mut rows);
    rows.sort_by_key(|b| std::cmp::Reverse(b.usd_micro_amount));
    rows
}

pub(super) fn format_private_asset_rows(
    chain_id: u64,
    totals: &[TokenTotal],
    registry: Option<&EffectiveTokenRegistry>,
    anchor_cache: Option<&TokenAnchorRateCache>,
) -> Vec<FormattedTokenTotal> {
    totals
        .iter()
        .map(|total| format_total_parts(chain_id, total, registry, anchor_cache))
        .collect()
}

pub(super) fn private_asset_display_amounts(
    asset: &FormattedTokenTotal,
) -> (String, Option<String>) {
    match asset.usd_amount.as_ref() {
        Some(usd_amount) => (
            usd_amount.clone(),
            Some(format!("{} {}", asset.amount, asset.label)),
        ),
        None => (asset.amount.clone(), None),
    }
}

pub(super) const fn private_send_action_tooltip(
    can_send: bool,
    actions_available: bool,
    syncing: bool,
    unavailable_balance_message: &'static str,
) -> &'static str {
    private_action_tooltip(
        can_send,
        actions_available,
        syncing,
        "Prepare private send calldata",
        "Open private send form while wallet sync finishes",
        unavailable_balance_message,
    )
}

pub(super) const fn private_unshield_action_tooltip(
    can_unshield: bool,
    actions_available: bool,
    syncing: bool,
    unavailable_balance_message: &'static str,
) -> &'static str {
    private_action_tooltip(
        can_unshield,
        actions_available,
        syncing,
        "Prepare unshield calldata",
        "Open unshield form while wallet sync finishes",
        unavailable_balance_message,
    )
}

const fn private_action_tooltip(
    can_act: bool,
    actions_available: bool,
    syncing: bool,
    ready_message: &'static str,
    syncing_message: &'static str,
    unavailable_balance_message: &'static str,
) -> &'static str {
    if can_act && syncing {
        syncing_message
    } else if can_act {
        ready_message
    } else if actions_available {
        unavailable_balance_message
    } else {
        "Available after wallet session starts"
    }
}

pub(super) fn total_private_balance_usd_amount(rows: &[FormattedTokenTotal]) -> Option<String> {
    let mut total: Option<U256> = None;
    for amount in rows.iter().filter_map(|row| row.usd_micro_amount) {
        total = Some(match total {
            Some(total) => total.checked_add(amount)?,
            None => amount,
        });
    }
    total.map(format_usd_micro_value)
}

#[cfg(test)]
pub(super) fn format_total(chain_id: u64, total: &TokenTotal) -> String {
    let formatted = format_total_parts(chain_id, total, None, None);
    format!("{} {}", formatted.label, formatted.amount)
}

fn format_total_parts(
    chain_id: u64,
    total: &TokenTotal,
    registry: Option<&EffectiveTokenRegistry>,
    anchor_cache: Option<&TokenAnchorRateCache>,
) -> FormattedTokenTotal {
    let total_raw = U256::from_str_radix(&total.total, 10).ok();
    let poi_verified_total_raw = U256::from_str_radix(&total.poi_verified_total, 10).ok();
    let pending_poi_total = pending_poi_total(total_raw, poi_verified_total_raw);
    let Some(address) = parse_address(&total.token) else {
        return FormattedTokenTotal {
            chain_id,
            token: None,
            label: total.token.clone(),
            amount: total.total.clone(),
            usd_micro_amount: None,
            usd_amount: None,
            pending_poi_amount: format_pending_poi_amount(pending_poi_total, None),
            pending_incoming_amount: "0".to_string(),
            pending_outgoing_amount: "0".to_string(),
            total: total_raw,
            poi_verified_total: poi_verified_total_raw,
            pending_poi_total,
            pending_incoming_total: Some(U256::ZERO),
            pending_outgoing_total: Some(U256::ZERO),
            decimals: None,
            icon_path: None,
        };
    };
    let Some(token) = token_display_metadata(registry, chain_id, &address) else {
        return FormattedTokenTotal {
            chain_id,
            token: Some(address),
            label: short_address(&address),
            amount: total.total.clone(),
            usd_micro_amount: None,
            usd_amount: None,
            pending_poi_amount: format_pending_poi_amount(pending_poi_total, None),
            pending_incoming_amount: "0".to_string(),
            pending_outgoing_amount: "0".to_string(),
            total: total_raw,
            poi_verified_total: poi_verified_total_raw,
            pending_poi_total,
            pending_incoming_total: Some(U256::ZERO),
            pending_outgoing_total: Some(U256::ZERO),
            decimals: None,
            icon_path: None,
        };
    };
    let amount = total_raw.map_or_else(
        || total.total.clone(),
        |value| format_token_amount(value, token.decimals),
    );
    let usd_micro_amount = total_raw
        .and_then(|value| private_asset_usd_micro_amount(chain_id, address, value, anchor_cache));
    let usd_amount = usd_micro_amount.map(format_usd_micro_value);
    FormattedTokenTotal {
        chain_id,
        token: Some(address),
        label: token.symbol,
        amount,
        usd_micro_amount,
        usd_amount,
        pending_poi_amount: format_pending_poi_amount(pending_poi_total, Some(token.decimals)),
        pending_incoming_amount: "0".to_string(),
        pending_outgoing_amount: "0".to_string(),
        total: total_raw,
        poi_verified_total: poi_verified_total_raw,
        pending_poi_total,
        pending_incoming_total: Some(U256::ZERO),
        pending_outgoing_total: Some(U256::ZERO),
        decimals: Some(token.decimals),
        icon_path: token.icon_path,
    }
}

fn apply_pending_asset_amounts(
    snapshot: &ListUtxosOutput,
    registry: Option<&EffectiveTokenRegistry>,
    anchor_cache: Option<&TokenAnchorRateCache>,
    rows: &mut Vec<FormattedTokenTotal>,
) {
    let mut pending: BTreeMap<Address, (U256, U256)> = BTreeMap::new();
    for row in &snapshot.utxos {
        if !row.pending_new && !row.pending_spent && !row.local_pending_spent {
            continue;
        }
        let Some(token) = parse_address(&row.token) else {
            continue;
        };
        let Ok(value) = U256::from_str_radix(&row.value, 10) else {
            continue;
        };
        let entry = pending.entry(token).or_default();
        if row.pending_new {
            entry.0 += value;
        }
        if row.pending_spent || row.local_pending_spent {
            entry.1 += value;
        }
    }

    for (token, (incoming, outgoing)) in pending {
        let index = rows.iter().position(|row| row.token == Some(token));
        let row = if let Some(index) = index {
            &mut rows[index]
        } else {
            rows.push(format_total_parts(
                snapshot.chain_id,
                &TokenTotal {
                    token: token.to_checksum(None),
                    total: "0".to_string(),
                    poi_verified_total: "0".to_string(),
                },
                registry,
                anchor_cache,
            ));
            rows.last_mut().expect("pending row inserted")
        };
        row.pending_incoming_total = Some(incoming);
        row.pending_outgoing_total = Some(outgoing);
        row.pending_incoming_amount = format_pending_amount(incoming, row.decimals);
        row.pending_outgoing_amount = format_pending_amount(outgoing, row.decimals);
    }
}

fn private_asset_usd_micro_amount(
    chain_id: u64,
    token: Address,
    total: U256,
    anchor_cache: Option<&TokenAnchorRateCache>,
) -> Option<U256> {
    anchor_cache.and_then(|cache| cache.cached_token_usd_micro_value(chain_id, token, total))
}

fn pending_poi_total(total: Option<U256>, poi_verified_total: Option<U256>) -> Option<U256> {
    total
        .zip(poi_verified_total)
        .map(|(total, poi_verified_total)| total.saturating_sub(poi_verified_total))
}

fn format_pending_poi_amount(pending_poi_total: Option<U256>, decimals: Option<u8>) -> String {
    pending_poi_total.as_ref().map_or_else(
        || "0".to_string(),
        |value| {
            if let Some(decimals) = decimals {
                format_token_amount(*value, decimals)
            } else {
                value.to_string()
            }
        },
    )
}

fn format_pending_amount(value: U256, decimals: Option<u8>) -> String {
    decimals.map_or_else(
        || value.to_string(),
        |decimals| format_token_amount(value, decimals),
    )
}

pub(super) fn should_show_pending_poi_amount(pending_poi_total: Option<U256>) -> bool {
    pending_poi_total.is_some_and(|amount| !amount.is_zero())
}

pub(super) fn should_show_pending_amount(pending_total: Option<U256>) -> bool {
    pending_total.is_some_and(|amount| !amount.is_zero())
}

fn private_pending_summary(
    assets: &[FormattedTokenTotal],
    snapshot: &ListUtxosOutput,
    blocked_shield_rows: Vec<UtxoDisplayRow>,
    poi_refreshing: bool,
    poi_refresh_session: Option<Arc<wallet_ops::WalletSession>>,
) -> Option<PrivatePendingSummary> {
    let pending_poi_assets = assets
        .iter()
        .filter(|asset| should_show_pending_poi_amount(asset.pending_poi_total))
        .map(|asset| PrivatePendingAssetLine {
            label: asset.label.clone(),
            amount: asset.pending_poi_amount.clone(),
        })
        .collect::<Vec<_>>();
    let pending_incoming_assets = assets
        .iter()
        .filter(|asset| should_show_pending_amount(asset.pending_incoming_total))
        .map(|asset| PrivatePendingAssetLine {
            label: asset.label.clone(),
            amount: asset.pending_incoming_amount.clone(),
        })
        .collect::<Vec<_>>();
    let pending_outgoing_assets = assets
        .iter()
        .filter(|asset| should_show_pending_amount(asset.pending_outgoing_total))
        .map(|asset| PrivatePendingAssetLine {
            label: asset.label.clone(),
            amount: asset.pending_outgoing_amount.clone(),
        })
        .collect::<Vec<_>>();
    let pending_incoming_outputs = snapshot.utxos.iter().filter(|row| row.pending_new).count();
    let pending_outgoing_outputs = snapshot
        .utxos
        .iter()
        .filter(|row| row.pending_spent || row.local_pending_spent)
        .count();
    let recoverable_poi_outputs = recoverable_poi_candidate_count(snapshot);

    if pending_poi_assets.is_empty()
        && pending_incoming_assets.is_empty()
        && pending_outgoing_assets.is_empty()
        && blocked_shield_rows.is_empty()
        && recoverable_poi_outputs == 0
    {
        return None;
    }

    Some(PrivatePendingSummary {
        blocked_shield_rows,
        pending_poi_assets,
        pending_incoming_assets,
        pending_outgoing_assets,
        pending_incoming_outputs,
        pending_outgoing_outputs,
        recoverable_poi_outputs,
        poi_refreshing,
        poi_refresh_session,
    })
}

const fn private_pending_summary_detail(summary: &PrivatePendingSummary) -> &'static str {
    if !summary.blocked_shield_rows.is_empty() {
        if summary.blocked_shield_rows.len() == 1 {
            "A blocked Shield can be reviewed and refunded."
        } else {
            "Blocked Shields can be reviewed and refunded."
        }
    } else if summary.recoverable_poi_outputs > 0 {
        "Some private outputs need PPOI recovery before they become spendable."
    } else if !summary.pending_poi_assets.is_empty() {
        "Some private outputs are waiting for PPOI validation before they become spendable."
    } else if summary.pending_incoming_outputs > 0 && summary.pending_outgoing_outputs > 0 {
        "Incoming private outputs and outgoing private spends are waiting for confirmation."
    } else if summary.pending_incoming_outputs > 0 {
        "Incoming private outputs are waiting for confirmation."
    } else {
        "Outgoing private spends are waiting for confirmation."
    }
}

const fn private_pending_summary_title(summary: &PrivatePendingSummary) -> &'static str {
    if summary.blocked_shield_rows.is_empty() {
        "Private balance update pending"
    } else {
        "Private assets need attention"
    }
}

pub(super) fn build_unshield_asset(
    snapshot: &ListUtxosOutput,
    asset: &FormattedTokenTotal,
) -> Option<UnshieldAsset> {
    let token = asset.token?;
    let total = asset.total?;
    let poi_verified_total = asset.poi_verified_total?;
    let max_batched = max_unshield_amount_from_snapshot(snapshot, token);
    if max_batched.is_zero() {
        return None;
    }
    Some(UnshieldAsset {
        chain_id: asset.chain_id,
        token,
        label: asset.label.clone(),
        decimals: asset.decimals,
        total,
        poi_verified_total,
        max_batched,
        icon_path: asset.icon_path.clone(),
    })
}

pub(super) fn build_send_asset(
    snapshot: &ListUtxosOutput,
    asset: &FormattedTokenTotal,
) -> Option<UnshieldAsset> {
    let token = asset.token?;
    let total = asset.total?;
    let poi_verified_total = asset.poi_verified_total?;
    let max_batched = max_send_amount_from_snapshot(snapshot, token);
    if max_batched.is_zero() {
        return None;
    }
    Some(UnshieldAsset {
        chain_id: asset.chain_id,
        token,
        label: asset.label.clone(),
        decimals: asset.decimals,
        total,
        poi_verified_total,
        max_batched,
        icon_path: asset.icon_path.clone(),
    })
}

impl WalletRoot {
    fn open_private_receive_address_dialog(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(address) = self
            .view_session
            .as_ref()
            .and_then(|session| session.receive_address().ok())
        else {
            return;
        };
        window.close_all_dialogs(cx);
        let dialog_width =
            (window.viewport_size().width * 0.92).min(PUBLIC_ADDRESS_QR_DIALOG_WIDTH);
        let dialog_max_height = dialog_max_height(window);
        let content_max_height = dialog_content_max_height(window);
        let content_width = secondary_dialog_content_width(dialog_width);
        let address_text = SharedString::from(address);
        let copy_id = SharedString::from("wallet-private-receive-address-copy");
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .w(dialog_width)
                .max_h(dialog_max_height)
                .title(app_strong_text("Private receive address"))
                .child(scrollable_dialog_content(
                    content_max_height,
                    render_public_address_qr_dialog_content(
                        None,
                        address_text.clone(),
                        None,
                        copy_id.clone(),
                        content_width,
                    ),
                ))
        });
    }

    pub(super) fn render_private_assets_body(&self, root: &Entity<Self>) -> gpui::AnyElement {
        if self.view_session.is_none() {
            return centered_message("Choose a wallet to continue").into_any_element();
        }
        match self.chain_states.get(&self.selected_chain) {
            Some(ChainUtxoState::Error { message, .. }) => self
                .render_chain_error_body(root, message.as_ref())
                .into_any_element(),
            Some(ChainUtxoState::Loading { progress }) => {
                centered_message(loading_summary(*progress)).into_any_element()
            }
            Some(
                state @ ChainUtxoState::Syncing {
                    snapshot, progress, ..
                },
            ) => self.render_private_asset_snapshot(
                root,
                snapshot,
                state.private_action_forms_available(),
                true,
                *progress,
            ),
            Some(state @ ChainUtxoState::Ready { snapshot, .. }) => self
                .render_private_asset_snapshot(
                    root,
                    snapshot,
                    state.private_action_forms_available(),
                    false,
                    None,
                ),
            Some(ChainUtxoState::Idle) | None => {
                centered_message("Select a chain to load private balances").into_any_element()
            }
        }
    }

    fn render_private_asset_snapshot(
        &self,
        root: &Entity<Self>,
        snapshot: &ListUtxosOutput,
        actions_available: bool,
        syncing: bool,
        progress: Option<wallet_ops::SyncProgressUpdate>,
    ) -> gpui::AnyElement {
        let assets = format_private_asset_rows_from_snapshot(
            snapshot,
            Some(&self.effective_token_registry),
            Some(&self.public_broadcaster_anchor_cache),
        );
        let total_balance = total_private_balance_usd_amount(&assets);
        let receive_available = self
            .view_session
            .as_ref()
            .and_then(|session| session.receive_address().ok())
            .is_some();
        let send_asset = assets
            .iter()
            .find_map(|asset| build_send_asset(snapshot, asset));
        let unshield_asset = assets
            .iter()
            .find_map(|asset| build_unshield_asset(snapshot, asset));
        let pending_summary = self.private_pending_summary_from_snapshot(snapshot, &assets);
        if assets.is_empty() {
            let message = if syncing {
                loading_summary(progress)
            } else {
                "No private assets found".to_string()
            };
            if receive_available {
                return Self::render_private_empty_state(root.clone(), message, receive_available)
                    .into_any_element();
            }
            return centered_message(message).into_any_element();
        }
        let has_total_balance = total_balance.is_some();

        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scrollbar()
            .child(
                div()
                    .w(super::PRIVATE_ASSET_LIST_WIDTH)
                    .max_w_full()
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .py(px(16.0))
                    .when_some(total_balance, |column, total_balance| {
                        column.child(Self::render_private_balance_hero(
                            root.clone(),
                            total_balance,
                            receive_available,
                            send_asset,
                            unshield_asset,
                            actions_available,
                            syncing,
                        ))
                    })
                    .when(!has_total_balance && receive_available, |column| {
                        column.child(Self::render_private_receive_action(
                            root.clone(),
                            receive_available,
                        ))
                    })
                    .when_some(pending_summary, |column, summary| {
                        column.child(Self::render_private_pending_status_card(root, &summary))
                    })
                    .children(assets.into_iter().enumerate().map(|(ix, asset)| {
                        Self::render_private_asset_row(
                            root.clone(),
                            ix,
                            asset,
                            snapshot,
                            actions_available,
                            syncing,
                        )
                        .into_any_element()
                    })),
            )
            .into_any_element()
    }

    fn render_private_empty_state(
        root: Entity<Self>,
        message: String,
        receive_available: bool,
    ) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .child(app_muted_text(message))
                    .child(Self::render_private_receive_button(
                        root,
                        "wallet-private-empty-receive",
                        receive_available,
                    )),
            )
    }

    fn render_private_receive_action(root: Entity<Self>, receive_available: bool) -> gpui::Div {
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(Self::render_private_receive_button(
                root,
                "wallet-private-standalone-receive",
                receive_available,
            ))
    }

    fn render_private_receive_button(
        root: Entity<Self>,
        id: &'static str,
        receive_available: bool,
    ) -> impl IntoElement {
        app_button(id, "Receive")
            .child(Icon::new(RailgunActionIcon::QrCode).small())
            .outline()
            .disabled(!receive_available)
            .tooltip("Show private receive address")
            .on_click(move |_event, window, cx| {
                root.update(cx, |root, cx| {
                    root.open_private_receive_address_dialog(window, cx);
                });
            })
    }

    pub(super) fn open_private_pending_status_dialog(
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        window.close_all_dialogs(cx);
        let root = cx.entity();
        let dialog_width = (window.viewport_size().width * 0.92).min(px(520.0));
        let dialog_max_height = dialog_max_height(window);
        let content_max_height = dialog_content_max_height(window);
        let content_width = secondary_dialog_content_width(dialog_width);
        window.open_dialog(cx, move |dialog, _window, cx| {
            let content_root = root.clone();
            let content = content_root
                .read(cx)
                .current_private_pending_summary()
                .map_or_else(
                    || private_pending_status_empty(content_width),
                    |summary| {
                        Self::render_private_pending_status_dialog_content(
                            &content_root,
                            &summary,
                            content_width,
                        )
                    },
                );
            dialog
                .w(dialog_width)
                .max_h(dialog_max_height)
                .title(app_strong_text("Private asset status"))
                .child(scrollable_dialog_content(content_max_height, content))
        });
    }

    fn current_private_pending_summary(&self) -> Option<PrivatePendingSummary> {
        let snapshot = self.chain_states.get(&self.selected_chain)?.snapshot()?;
        let assets = format_private_asset_rows_from_snapshot(
            snapshot,
            Some(&self.effective_token_registry),
            Some(&self.public_broadcaster_anchor_cache),
        );
        self.private_pending_summary_from_snapshot(snapshot, &assets)
    }

    fn private_pending_summary_from_snapshot(
        &self,
        snapshot: &ListUtxosOutput,
        assets: &[FormattedTokenTotal],
    ) -> Option<PrivatePendingSummary> {
        let chain_state = self.chain_states.get(&self.selected_chain);
        let blocked_shield_rows = blocked_shield_rescue_display_rows(
            snapshot,
            &self.blocked_shield_rescue_rows,
            &self.blocked_shield_refunds_in_flight,
        );
        private_pending_summary(
            assets,
            snapshot,
            blocked_shield_rows,
            chain_state.is_some_and(ChainUtxoState::poi_refreshing),
            chain_state.and_then(ChainUtxoState::poi_refresh_session),
        )
    }

    fn render_private_pending_status_card(
        root: &Entity<Self>,
        summary: &PrivatePendingSummary,
    ) -> gpui::Div {
        let details_root = root.clone();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb_with_alpha(theme::WARNING, 0.08))
            .p(px(14.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        Icon::new(RailgunActionIcon::Clock)
                            .small()
                            .text_color(rgb(theme::WARNING)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(app_strong_text(private_pending_summary_title(summary)))
                            .child(
                                app_muted_text(private_pending_summary_detail(summary))
                                    .line_height(px(18.0))
                                    .whitespace_normal(),
                            ),
                    )
                    .child(
                        app_button_base("wallet-private-pending-details")
                            .ghost()
                            .xsmall()
                            .compact()
                            .child("Details")
                            .on_click(move |_event, window, cx| {
                                details_root.update(cx, |_root, cx| {
                                    Self::open_private_pending_status_dialog(window, cx);
                                });
                            }),
                    ),
            )
    }

    fn render_private_pending_status_dialog_content(
        root: &Entity<Self>,
        summary: &PrivatePendingSummary,
        content_width: Pixels,
    ) -> gpui::Div {
        let retry_session = summary.poi_refresh_session.clone();
        let retrying = summary.poi_refreshing;
        let recoverable = summary.recoverable_poi_outputs;
        let show_recovery = recoverable > 0;

        div()
            .w(content_width)
            .flex()
            .flex_col()
            .gap_3()
            .child(private_pending_dialog_intro())
            .when(!summary.blocked_shield_rows.is_empty(), |this| {
                this.child(private_blocked_shield_detail_section(
                    root,
                    &summary.blocked_shield_rows,
                ))
            })
            .when(!summary.pending_incoming_assets.is_empty(), |this| {
                this.child(private_pending_detail_section(
                    "Pending incoming",
                    pending_output_count_label(summary.pending_incoming_outputs),
                    "Detected private outputs waiting for chain confirmation and safe-head finality.",
                    &summary.pending_incoming_assets,
                    "+",
                    theme::WARNING,
                    RailgunActionIcon::Clock,
                    None,
                ))
            })
            .when(!summary.pending_outgoing_assets.is_empty(), |this| {
                this.child(private_pending_detail_section(
                    "Pending outgoing",
                    pending_output_count_label(summary.pending_outgoing_outputs),
                    "Detected or locally submitted private spends waiting for confirmation.",
                    &summary.pending_outgoing_assets,
                    "-",
                    theme::WARNING,
                    RailgunActionIcon::Clock,
                    None,
                ))
            })
            .when(!summary.pending_poi_assets.is_empty(), |this| {
                this.child(private_pending_detail_section(
                    "PPOI pending",
                    pending_asset_count_label(summary.pending_poi_assets.len()),
                    "Balances are detected but not PPOI-verified yet, so they are not spendable.",
                    &summary.pending_poi_assets,
                    "",
                    theme::WARNING,
                    RailgunActionIcon::Clock,
                    (recoverable == 0).then_some("No recoverable PPOI outputs found yet."),
                ))
            })
            .when(show_recovery, |this| {
                this.child(private_pending_recovery_section(recoverable))
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        app_button("wallet-private-pending-dialog-close", "Close")
                            .outline()
                            .small()
                            .on_click(move |_event, window, cx| {
                                window.close_dialog(cx);
                            }),
                    )
                    .when(recoverable > 0, |actions| {
                        actions.child(
                            app_button(
                                "wallet-private-pending-dialog-retry-poi",
                                retry_poi_label(recoverable),
                            )
                            .primary()
                            .small()
                            .loading(retrying)
                            .disabled(retrying || retry_session.is_none())
                            .on_click(move |_event, _window, cx| {
                                Self::retry_poi_recovery(retry_session.clone(), cx);
                            }),
                        )
                    }),
            )
    }

    fn render_private_balance_hero(
        root: Entity<Self>,
        total_balance: String,
        receive_available: bool,
        send_asset: Option<UnshieldAsset>,
        unshield_asset: Option<UnshieldAsset>,
        actions_available: bool,
        syncing: bool,
    ) -> gpui::Div {
        let receive_root = root.clone();
        let send_root = root.clone();
        let unshield_root = root;
        let can_send = actions_available && send_asset.is_some();
        let can_unshield = actions_available && unshield_asset.is_some();

        div()
            .w_full()
            .min_h(px(210.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .px(px(24.0))
            .py(px(34.0))
            .child(
                div()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .text_size(theme::ASSET_SYMBOL_TEXT_SIZE)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Total private balance"),
            )
            .child(
                div()
                    .text_color(rgb(theme::WARNING))
                    .text_size(px(44.0))
                    .line_height(px(48.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(total_balance)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Self::render_private_receive_button(
                        receive_root,
                        "wallet-private-hero-receive",
                        receive_available,
                    ))
                    .child(
                        app_button("wallet-private-hero-send", "Send")
                            .child(Icon::new(RailgunActionIcon::Send).small())
                            .outline()
                            .disabled(!can_send)
                            .tooltip(private_send_action_tooltip(
                                can_send,
                                actions_available,
                                syncing,
                                "No sendable private asset available",
                            ))
                            .on_click(move |_event, window, cx| {
                                let Some(asset) = send_asset.clone() else {
                                    return;
                                };
                                send_root.update(cx, |root, cx| {
                                    root.open_send_form(asset, window, cx);
                                });
                            }),
                    )
                    .child(
                        app_button("wallet-private-hero-unshield", "Unshield")
                            .child(Icon::new(IconName::Globe).small())
                            .outline()
                            .disabled(!can_unshield)
                            .tooltip(private_unshield_action_tooltip(
                                can_unshield,
                                actions_available,
                                syncing,
                                "No unshieldable private asset available",
                            ))
                            .on_click(move |_event, window, cx| {
                                let Some(asset) = unshield_asset.clone() else {
                                    return;
                                };
                                unshield_root.update(cx, |root, cx| {
                                    root.open_unshield_form(asset, window, cx);
                                });
                            }),
                    ),
            )
    }

    fn render_private_asset_row(
        root: Entity<Self>,
        ix: usize,
        asset: FormattedTokenTotal,
        snapshot: &ListUtxosOutput,
        actions_available: bool,
        syncing: bool,
    ) -> gpui::Div {
        let send_asset = build_send_asset(snapshot, &asset);
        let can_send = actions_available && send_asset.is_some();
        let unshield_asset = build_unshield_asset(snapshot, &asset);
        let can_unshield = actions_available && unshield_asset.is_some();
        let send_tooltip = private_send_action_tooltip(
            can_send,
            actions_available,
            syncing,
            "No spendable private balance for this token",
        );
        let unshield_tooltip = private_unshield_action_tooltip(
            can_unshield,
            actions_available,
            syncing,
            "No unshieldable private balance for this token",
        );
        let send_opacity = if can_send { 1.0 } else { 0.5 };
        let unshield_opacity = if can_unshield { 1.0 } else { 0.5 };
        let show_pending_poi = should_show_pending_poi_amount(asset.pending_poi_total);
        let pending_poi_amount = asset.pending_poi_amount.clone();
        let show_pending_incoming = should_show_pending_amount(asset.pending_incoming_total);
        let show_pending_outgoing = should_show_pending_amount(asset.pending_outgoing_total);
        let pending_incoming_amount = asset.pending_incoming_amount.clone();
        let pending_outgoing_amount = asset.pending_outgoing_amount.clone();
        let (primary_amount, secondary_amount) = private_asset_display_amounts(&asset);
        let row_group = SharedString::from(format!("wallet-private-asset-row-{ix}"));
        let send_root = root.clone();
        let unshield_root = root;

        div()
            .group(row_group.clone())
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .p(px(16.0))
            .rounded_lg()
            .bg(rgb(theme::SURFACE))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .text_size(theme::ASSET_SYMBOL_TEXT_SIZE)
                    .text_color(rgb(theme::TEXT))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(private_asset_label_row(
                        SharedString::from(asset.label.clone()),
                        asset.icon_path,
                    )),
            )
            .child(
                div()
                    .group(row_group.clone())
                    .flex()
                    .items_center()
                    .gap_2()
                    .opacity(0.0)
                    .group_hover(row_group, |this| this.opacity(1.0))
                    .hover(|this| this.opacity(1.0))
                    .child(
                        app_button(
                            SharedString::from(format!("wallet-asset-send-{ix}")),
                            "Send",
                        )
                        .child(Icon::new(RailgunActionIcon::Send).small())
                        .outline()
                        .disabled(!can_send)
                        .opacity(send_opacity)
                        .tooltip(send_tooltip)
                        .on_click(move |_event, window, cx| {
                            let Some(asset) = send_asset.clone() else {
                                return;
                            };
                            send_root.update(cx, |root, cx| {
                                root.open_send_form(asset, window, cx);
                            });
                        }),
                    )
                    .child(
                        app_button(
                            SharedString::from(format!("wallet-asset-unshield-{ix}")),
                            "Unshield",
                        )
                        .child(Icon::new(IconName::Globe).small())
                        .outline()
                        .disabled(!can_unshield)
                        .opacity(unshield_opacity)
                        .tooltip(unshield_tooltip)
                        .on_click(move |_event, window, cx| {
                            let Some(asset) = unshield_asset.clone() else {
                                return;
                            };
                            unshield_root.update(cx, |root, cx| {
                                root.open_unshield_form(asset, window, cx);
                            });
                        }),
                    ),
            )
            .child(
                div()
                    .min_w(px(150.0))
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .text_color(rgb(theme::WARNING))
                            .text_size(theme::BALANCE_TEXT_SIZE)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(primary_amount)),
                    )
                    .when_some(secondary_amount, |column, secondary_amount| {
                        column.child(
                            app_muted_text(secondary_amount)
                                .whitespace_nowrap()
                                .text_align(gpui::TextAlign::Right),
                        )
                    })
                    .when(show_pending_poi, |column| {
                        column.child(private_asset_pending_label(format!(
                            "Pending POI: {pending_poi_amount}"
                        )))
                    })
                    .when(show_pending_incoming, |column| {
                        column.child(private_asset_pending_label(format!(
                            "Pending: +{pending_incoming_amount}"
                        )))
                    })
                    .when(show_pending_outgoing, |column| {
                        column.child(private_asset_pending_label(format!(
                            "Pending: -{pending_outgoing_amount}"
                        )))
                    }),
            )
    }
}

fn private_asset_label_row(label: SharedString, icon_path: Option<WalletIconSource>) -> gpui::Div {
    let mut row = div().flex().items_center().gap_2();
    if let Some(path) = icon_path {
        row = row.child(img(path).size(px(32.0)).rounded_full().flex_none());
    }
    row.child(label)
}

fn private_asset_pending_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap_1()
        .text_color(rgb(theme::TEXT_MUTED))
        .text_size(px(12.0))
        .child(Icon::new(RailgunActionIcon::Clock).xsmall())
        .child(
            app_muted_text(label)
                .text_size(px(12.0))
                .whitespace_nowrap()
                .text_align(gpui::TextAlign::Right),
        )
}

fn retry_poi_label(count: usize) -> String {
    if count == 1 {
        "Retry PPOI recovery".to_string()
    } else {
        format!("Retry PPOI recovery ({count})")
    }
}

fn private_pending_dialog_intro() -> gpui::Div {
    div()
        .text_size(px(13.0))
        .line_height(px(19.0))
        .text_color(rgb(theme::TEXT_MUTED))
        .child("Private balances update automatically from scanned Railgun events.")
}

fn private_pending_status_empty(content_width: Pixels) -> gpui::Div {
    div()
        .w(content_width)
        .text_size(px(13.0))
        .line_height(px(19.0))
        .text_color(rgb(theme::TEXT_MUTED))
        .child("No private asset status currently needs attention.")
}

fn private_pending_detail_section(
    title: &'static str,
    count_label: String,
    detail: &'static str,
    assets: &[PrivatePendingAssetLine],
    prefix: &'static str,
    accent_color: u32,
    icon: RailgunActionIcon,
    footer_note: Option<&'static str>,
) -> gpui::Div {
    let content = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(private_pending_section_header(
            title,
            count_label,
            accent_color,
            icon,
        ))
        .child(
            app_muted_text(detail)
                .text_size(px(12.0))
                .line_height(px(17.0))
                .whitespace_normal(),
        )
        .children(
            assets
                .iter()
                .map(|asset| private_pending_asset_amount_row(asset, prefix)),
        )
        .when_some(footer_note, |content, note| {
            content.child(
                app_muted_text(note)
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .whitespace_normal(),
            )
        });
    private_pending_section_card(accent_color, content)
}

fn private_pending_section_card(accent_color: u32, content: gpui::Div) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_3()
        .p(px(10.0))
        .child(private_pending_section_accent(accent_color))
        .child(content.flex_1().min_w(px(0.0)))
}

fn private_pending_section_accent(accent_color: u32) -> gpui::Div {
    div()
        .w(px(3.0))
        .min_h(px(48.0))
        .h_full()
        .flex_none()
        .rounded_full()
        .bg(rgb_with_alpha(accent_color, 0.82))
}

fn private_pending_section_header(
    title: &'static str,
    count_label: String,
    accent_color: u32,
    icon: RailgunActionIcon,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).xsmall().text_color(rgb(accent_color)))
                .child(app_strong_text(title)),
        )
        .child(app_muted_text(count_label).text_size(px(12.0)))
}

fn private_blocked_shield_detail_section(
    root: &Entity<WalletRoot>,
    rows: &[UtxoDisplayRow],
) -> gpui::Div {
    let content = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(private_pending_section_header(
            "Blocked Shields",
            pending_output_count_label(rows.len()),
            theme::DANGER,
            RailgunActionIcon::Shield,
        ))
        .child(
            app_muted_text("Blocked Shield outputs cannot be spent privately, but can be refunded to the original public source account when the origin is available.")
                .text_size(px(12.0))
                .line_height(px(17.0))
                .whitespace_normal(),
        )
        .children(
            rows.iter()
                .enumerate()
                .map(|(ix, row)| private_blocked_shield_row(root, ix, row)),
        );
    private_pending_section_card(theme::DANGER, content)
}

fn private_blocked_shield_row(
    root: &Entity<WalletRoot>,
    ix: usize,
    row: &UtxoDisplayRow,
) -> gpui::Div {
    let status = blocked_shield_status_label(row);
    let action_available = blocked_shield_refund_action_available(row);
    let resolving = blocked_shield_refund_origin_resolving(row);
    let action_label = blocked_shield_action_label(row);
    let action_row = row.clone();
    let action_root = root.clone();
    let mut action = app_button(
        SharedString::from(format!("wallet-private-blocked-shield-action-{ix}")),
        action_label,
    )
    .small();
    if row
        .blocked_shield_rescue
        .as_ref()
        .is_some_and(|rescue| rescue.eligible)
    {
        action = action.danger();
    } else {
        action = action.outline();
    }
    action = action.loading(resolving).disabled(resolving);
    if action_available {
        action = action.on_click(move |_event, window, cx| {
            let row = action_row.clone();
            action_root.update(cx, |root, cx| {
                root.begin_blocked_shield_refund(&row, window, cx);
            });
        });
    } else {
        action = action.disabled(true);
    }

    div()
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .text_size(px(12.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_color(rgb(theme::TEXT))
                        .child(SharedString::from(format!("{} {}", row.amount, row.token))),
                )
                .child(
                    app_muted_text(format!("{} · {}", status, short_hash(&row.source_tx_hash)))
                        .text_size(px(12.0)),
                ),
        )
        .child(action)
}

fn blocked_shield_action_label(row: &UtxoDisplayRow) -> &'static str {
    if blocked_shield_refund_action_available(row) || blocked_shield_refund_origin_resolving(row) {
        "Refund"
    } else {
        "Unavailable"
    }
}

fn blocked_shield_status_label(row: &UtxoDisplayRow) -> String {
    let Some(rescue) = row.blocked_shield_rescue.as_ref() else {
        return "Refund unavailable".to_string();
    };
    if rescue.eligible {
        return "Refund available".to_string();
    }
    if blocked_shield_refund_action_available(row) {
        return "Origin check needed".to_string();
    }
    rescue
        .disabled_reason
        .clone()
        .unwrap_or_else(|| "Refund unavailable".to_string())
}

fn private_pending_asset_amount_row(
    asset: &PrivatePendingAssetLine,
    prefix: &'static str,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_size(px(12.0))
        .child(
            div()
                .min_w(px(0.0))
                .truncate()
                .text_color(rgb(theme::TEXT))
                .child(SharedString::from(asset.label.clone())),
        )
        .child(
            div()
                .flex_none()
                .text_color(rgb(theme::TEXT_MUTED))
                .child(SharedString::from(format!(
                    "{prefix}{} {}",
                    asset.amount, asset.label
                ))),
        )
}

fn private_pending_recovery_section(recoverable: usize) -> gpui::Div {
    let content = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(private_pending_section_header(
            "Recoverable PPOI outputs",
            pending_output_count_label(recoverable),
            theme::DANGER,
            RailgunActionIcon::Clock,
        ))
        .child(
            app_muted_text("Recoverable PPOI outputs can be retried without querying wallet-specific balances from a server.")
                .text_size(px(12.0))
                .line_height(px(17.0))
                .whitespace_normal(),
        );
    private_pending_section_card(theme::DANGER, content)
}

fn pending_output_count_label(count: usize) -> String {
    if count == 1 {
        "1 output".to_string()
    } else {
        format!("{count} outputs")
    }
}

fn pending_asset_count_label(count: usize) -> String {
    if count == 1 {
        "1 asset".to_string()
    } else {
        format!("{count} assets")
    }
}

pub(super) fn refresh_form_asset_from_snapshot(
    snapshot: &ListUtxosOutput,
    current: &UnshieldAsset,
    send: bool,
    registry: Option<&EffectiveTokenRegistry>,
) -> UnshieldAsset {
    let formatted = format_private_asset_rows(snapshot.chain_id, &snapshot.totals, registry, None)
        .into_iter()
        .find(|asset| asset.token == Some(current.token));
    let total = formatted
        .as_ref()
        .and_then(|asset| asset.total)
        .unwrap_or_default();
    let poi_verified_total = formatted
        .as_ref()
        .and_then(|asset| asset.poi_verified_total)
        .unwrap_or_default();
    let max_batched = if send {
        max_send_amount_from_snapshot(snapshot, current.token)
    } else {
        max_unshield_amount_from_snapshot(snapshot, current.token)
    };

    UnshieldAsset {
        chain_id: current.chain_id,
        token: current.token,
        label: formatted
            .as_ref()
            .map_or_else(|| current.label.clone(), |asset| asset.label.clone()),
        decimals: formatted
            .as_ref()
            .and_then(|asset| asset.decimals)
            .or(current.decimals),
        total,
        poi_verified_total,
        max_batched,
        icon_path: formatted
            .as_ref()
            .and_then(|asset| asset.icon_path.clone())
            .or_else(|| current.icon_path.clone()),
    }
}

#[cfg(test)]
pub(super) fn send_asset_key_from_formatted(
    asset: &FormattedTokenTotal,
) -> Option<UnshieldAssetKey> {
    unshield_asset_key_from_formatted(asset)
}

#[cfg(test)]
pub(super) fn send_key_matches_asset(key: UnshieldAssetKey, asset: &FormattedTokenTotal) -> bool {
    send_asset_key_from_formatted(asset) == Some(key)
}

#[cfg(test)]
pub(super) fn unshield_asset_key_from_formatted(
    asset: &FormattedTokenTotal,
) -> Option<UnshieldAssetKey> {
    asset
        .token
        .map(|token| UnshieldAssetKey::new(asset.chain_id, token))
}

#[cfg(test)]
pub(super) fn unshield_key_matches_asset(
    key: UnshieldAssetKey,
    asset: &FormattedTokenTotal,
) -> bool {
    unshield_asset_key_from_formatted(asset) == Some(key)
}
