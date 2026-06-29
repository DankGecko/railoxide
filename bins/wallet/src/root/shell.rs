use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::hex;
use broadcaster_monitor::{EventRx, EventTx, Shared};
use gpui::ObjectFit;
use gpui::{
    Animation, AnimationExt as _, App, AppContext, Bounds, Context, Entity, Focusable,
    InteractiveElement, IntoElement, ParentElement, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowBounds, WindowOptions,
    bounce, div, ease_in_out, img, prelude::FluentBuilder as _, px, rgb, size,
};
use gpui_component::{
    Disableable, Icon, IconName, Root, Sizable, TitleBar,
    badge::Badge,
    button::ButtonVariants,
    progress::Progress as UiProgress,
    resizable::{resizable_panel, v_resizable},
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
};
use tokio::runtime::Handle;
use ui::clipboard::{clipboard_with_toast, copy_to_clipboard_with_custom_toast};
use ui::controls::{app_button, app_button_base};
use ui::icons;
use ui::logs::LogStore;
use ui::theme::{self, APP_FONT_FAMILY, APP_MONO_FONT_FAMILY, APP_TEXT_SIZE};
use wallet_ops::{
    PoiArtifactCacheListProgress, PoiArtifactCachePhase, PoiArtifactCacheProgress,
    WalletIndexedCatchUpSource, WalletIndexedCatchUpStatus, WalletNetworkMode, WalletSyncTip,
};

use crate::assets::{
    HEMATITE_HERO_PATH, HERO_WORDMARK_PATH, LOGO_ICON_PATH, RailgunSocialIcon, WARM_GLOW_PATH,
};

use super::actions::register_wallet_shortcut_root;
use super::chain_load::{
    BalanceSyncIssue, PresenceStatus, SyncStatusContext, SyncStatusLabels, WalletStatusCounts,
    balance_sync_issue, balances_presence_status, ppoi_presence_status, ready_status_bar,
    sync_status_bar, sync_status_labels,
};
use super::private_assets::{
    format_private_asset_rows_from_snapshot, should_show_pending_poi_amount,
};
use super::utxo::{
    blocked_shield_rescue_display_rows, recoverable_poi_candidate_count, should_focus_utxo_table,
};
use super::{
    Activity, ChainUtxoState, HERO_CARD_MAX_WIDTH, HERO_MEDIUM_BREAKPOINT, HERO_STAGE_MAX_WIDTH,
    HERO_WIDE_BREAKPOINT, LOGS_DRAWER_HEIGHT, LOGS_DRAWER_MAX_HEIGHT, LOGS_DRAWER_MIN_HEIGHT,
    SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, SIDEBAR_AUTO_COLLAPSE_WIDTH, VaultState,
    WalletRoot, WalletStartupRoot, app_status_tag, chain_load_overrides, count_label,
    rgb_with_alpha,
};

pub(super) const COPY_URL_TOOLTIP: &str = "Click to copy URL to clipboard";
pub(super) const LINK_COPIED_MESSAGE: &str = "Link copied to clipboard!";
pub(super) const RAILOXIDE_REPOSITORY_URL: &str = "https://github.com/triamazikamno/railoxide";
pub(super) const TELEGRAM_URL: &str = "https://t.me/railoxide";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum WalletTab {
    #[default]
    Private,
    Public,
    Activity,
}

impl WalletTab {
    pub(super) const ALL: [Self; 3] = [Self::Private, Self::Public, Self::Activity];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Private => "Private",
            Self::Public => "Public",
            Self::Activity => "Activity",
        }
    }

    pub(super) const fn icon_path(self) -> &'static str {
        match self {
            Self::Private => icons::shield_check_icon_path(),
            Self::Public => icons::globe_icon_path(),
            Self::Activity => icons::activity_icon_path(),
        }
    }

    pub(super) const fn shows_utxos(self) -> bool {
        matches!(self, Self::Activity)
    }
}

#[derive(Clone)]
pub(crate) struct WalletAppOptions {
    pub(super) db_path: PathBuf,
}

impl TryFrom<crate::cli::Options> for WalletAppOptions {
    type Error = eyre::Report;

    fn try_from(value: crate::cli::Options) -> Result<Self, Self::Error> {
        Ok(Self {
            db_path: value.db_path.unwrap_or_else(crate::cli::default_db_path),
        })
    }
}

pub(crate) fn open_wallet_window(
    app: &mut App,
    options: WalletAppOptions,
    runtime: Handle,
    monitor: Shared,
    event_tx: EventTx,
    event_rx: EventRx,
    chain_ids: &[u64],
    logs: LogStore,
) {
    wallet_ops::vault::enable_best_effort_runtime_hardening();
    let chain_ids = chain_ids.to_vec();
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(1_360.0), px(860.0)),
        })),
        titlebar: Some(wallet_titlebar_options()),
        window_decorations: Some(gpui::WindowDecorations::Client),
        ..Default::default()
    };

    if let Err(error) = app.open_window(window_options, |window, cx| {
        let root = cx.new(|cx| {
            WalletStartupRoot::new(
                options, runtime, monitor, event_tx, event_rx, &chain_ids, logs, window, cx,
            )
        });
        register_wallet_shortcut_root(window, &root, cx);
        cx.new(|cx| Root::new(root, window, cx))
    }) {
        tracing::error!(%error, "failed to open wallet window");
    }
}

impl WalletRoot {
    fn select_wallet_tab(&mut self, tab: WalletTab, cx: &mut Context<'_, Self>) {
        if self.active_wallet_tab == tab {
            return;
        }
        self.active_wallet_tab = tab;
        self.focus_utxo_table_on_render = should_focus_utxo_table(
            self.active_activity,
            self.active_wallet_tab,
            self.chain_states.get(&self.selected_chain),
        );
        if tab == WalletTab::Public {
            self.focus_public_account_search_on_render = true;
            self.schedule_public_balance_refresh(cx);
        }
        cx.notify();
    }

    pub(super) fn focus_public_account_search_if_requested(
        &mut self,
        window: &mut Window,
        cx: &Context<'_, Self>,
    ) {
        if !self.focus_public_account_search_on_render
            || self.active_activity != Activity::Wallet
            || self.active_wallet_tab != WalletTab::Public
        {
            return;
        }

        self.public_form
            .search_input
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        self.focus_public_account_search_on_render = false;
    }
}

impl Render for WalletRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.apply_public_broadcaster_error_amount_adjustments(window, cx);
        self.sync_walletconnect_attention_for_window(window);
        self.ensure_prover_cache_build_monitor(cx);
        self.focus_vault_input_if_requested(window, cx);
        self.focus_utxo_table_if_requested(window, cx);
        self.focus_public_account_search_if_requested(window, cx);

        let root = cx.entity();
        if !matches!(self.vault_state, VaultState::ViewUnlocked) {
            return self.render_locked_vault_screen(root, window);
        }
        self.open_next_walletconnect_request_dialog_if_idle(window, cx);
        let sidebar_is_narrow = window.viewport_size().width < SIDEBAR_AUTO_COLLAPSE_WIDTH;
        if !sidebar_is_narrow {
            self.sidebar_narrow_expanded = false;
        }
        let sidebar_collapsed = if sidebar_is_narrow {
            !self.sidebar_narrow_expanded
        } else {
            self.sidebar_manually_collapsed
        };

        div()
            .relative()
            .size_full()
            .flex()
            .bg(rgb(theme::SURFACE_ELEVATED))
            .text_color(rgb(theme::TEXT))
            .font_family(APP_FONT_FAMILY)
            .text_size(APP_TEXT_SIZE)
            .child(self.render_sidebar(root.clone(), sidebar_collapsed, sidebar_is_narrow))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(self.render_workspace(root, window)),
            )
    }
}

fn wallet_titlebar_options() -> gpui::TitlebarOptions {
    let mut options = TitleBar::title_bar_options();
    options.title = Some(SharedString::from("RailOxide"));
    options
}

pub(super) fn render_wallet_window_frame(
    content: gpui::AnyElement,
    window: &Window,
    titlebar_color: u32,
) -> gpui::Div {
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgb(theme::SURFACE_ELEVATED))
        .text_color(rgb(theme::TEXT))
        .font_family(APP_FONT_FAMILY)
        .text_size(APP_TEXT_SIZE)
        .when(should_render_wallet_title_bar(window), |this| {
            this.child(render_wallet_title_bar(titlebar_color))
        })
        .child(div().flex_1().min_w(px(0.0)).min_h(px(0.0)).child(content))
}

fn should_render_wallet_title_bar(window: &Window) -> bool {
    !cfg!(any(target_os = "linux", target_os = "freebsd"))
        || matches!(
            window.window_decorations(),
            gpui::Decorations::Client { .. }
        )
}

fn render_wallet_title_bar(titlebar_color: u32) -> TitleBar {
    TitleBar::new()
        .bg(rgb(titlebar_color))
        .border_color(rgb(titlebar_color))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w(px(0.0))
                .child(img(LOGO_ICON_PATH).size(px(16.0)))
                .child(
                    div()
                        .text_color(rgb(theme::TEXT))
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("RailOxide"),
                ),
        )
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WalletHeroLayout {
    Wide,
    Medium,
    Narrow,
}

fn wallet_hero_layout(window: &Window) -> WalletHeroLayout {
    let viewport = window.viewport_size();
    if viewport.width >= HERO_WIDE_BREAKPOINT && viewport.width >= viewport.height * 1.4 {
        WalletHeroLayout::Wide
    } else if viewport.width >= HERO_MEDIUM_BREAKPOINT {
        WalletHeroLayout::Medium
    } else {
        WalletHeroLayout::Narrow
    }
}

pub(super) fn render_wallet_hero_screen(window: &Window, card: gpui::AnyElement) -> gpui::Div {
    let viewport = window.viewport_size();
    let layout = wallet_hero_layout(window);
    let stage_width = (viewport.width - px(96.0))
        .max(px(0.0))
        .min(HERO_STAGE_MAX_WIDTH);
    let card_width = (viewport.width - px(48.0))
        .max(px(0.0))
        .min(HERO_CARD_MAX_WIDTH);
    let vertical_padding = match layout {
        WalletHeroLayout::Wide => px(32.0),
        WalletHeroLayout::Medium => px(40.0),
        WalletHeroLayout::Narrow => px(24.0),
    };
    let scroll_content_min_height = (viewport.height - vertical_padding * 2.0).max(px(0.0));

    let stage = if layout == WalletHeroLayout::Wide {
        div()
            .w(stage_width)
            .flex()
            .items_center()
            .gap_6()
            .child(
                render_wallet_brand_block(window, layout)
                    .w(px(560.0))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .justify_end()
                    .child(div().w(card_width).child(card)),
            )
    } else {
        div()
            .w(card_width)
            .flex()
            .flex_col()
            .items_center()
            .gap_6()
            .child(render_wallet_brand_block(window, layout).w_full())
            .child(div().w_full().child(card))
    };

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .bg(rgb(theme::BACKGROUND))
        .text_color(rgb(theme::TEXT))
        .font_family(APP_FONT_FAMILY)
        .text_size(APP_TEXT_SIZE)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .w_full()
                        .min_h(scroll_content_min_height)
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(24.0))
                        .py(vertical_padding)
                        .child(stage),
                ),
        )
}

fn render_wallet_brand_block(window: &Window, layout: WalletHeroLayout) -> gpui::Div {
    let viewport = window.viewport_size();
    let show_mineral = layout != WalletHeroLayout::Narrow;
    let mineral_size = match layout {
        WalletHeroLayout::Wide => (viewport.height * 0.42).min(px(500.0)).max(px(360.0)),
        WalletHeroLayout::Medium => (viewport.width * 0.24).min(px(320.0)).max(px(210.0)),
        WalletHeroLayout::Narrow => px(0.0),
    };
    let wordmark_width = match layout {
        WalletHeroLayout::Wide => px(400.0),
        WalletHeroLayout::Medium => (viewport.width * 0.44).min(px(360.0)).max(px(260.0)),
        WalletHeroLayout::Narrow => (viewport.width * 0.66).min(px(360.0)).max(px(220.0)),
    };
    let wordmark_height = wordmark_width * (23.0 / 166.0);
    let art_size = mineral_size * 1.5;
    let horizontal_mineral_offset = (art_size - mineral_size) / 2.0;
    let vertical_glow_offset = (mineral_size - art_size) / 2.0;

    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_6()
        .when(show_mineral, |this| {
            this.child(
                div()
                    .relative()
                    .w(art_size)
                    .h(mineral_size)
                    .child(
                        img(WARM_GLOW_PATH)
                            .absolute()
                            .top(vertical_glow_offset)
                            .left_0()
                            .size(art_size)
                            .object_fit(ObjectFit::Fill),
                    )
                    .child(
                        img(HEMATITE_HERO_PATH)
                            .absolute()
                            .top_0()
                            .left(horizontal_mineral_offset)
                            .size(mineral_size)
                            .object_fit(ObjectFit::Contain),
                    ),
            )
        })
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    img(HERO_WORDMARK_PATH)
                        .w(wordmark_width)
                        .h(wordmark_height)
                        .object_fit(ObjectFit::Contain),
                )
                .child(render_wallet_build_metadata()),
        )
}

fn render_wallet_build_metadata() -> gpui::Div {
    let build_label = wallet_build_label();

    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .font_family(APP_MONO_FONT_FAMILY)
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(build_label.clone()),
                )
                .child(clipboard_with_toast(
                    "wallet-hero-build-info-copy",
                    build_label,
                )),
        )
        .child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .gap_1()
                .child(render_wallet_social_copy_button(
                    "wallet-hero-repository-url-copy",
                    Icon::new(IconName::GitHub).size_4(),
                    RAILOXIDE_REPOSITORY_URL,
                ))
                .child(render_wallet_social_copy_button(
                    "wallet-hero-telegram-url-copy",
                    Icon::new(RailgunSocialIcon::Telegram).size_4(),
                    TELEGRAM_URL,
                )),
        )
}

fn render_wallet_social_copy_button(
    id: &'static str,
    icon: impl IntoElement,
    url: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(theme::TEXT_MUTED))
        .cursor_pointer()
        .hover(|this| {
            this.bg(rgb_with_alpha(theme::SURFACE_HOVER, 0.24))
                .text_color(rgb(theme::TEXT))
        })
        .tooltip(|window, cx| Tooltip::new(COPY_URL_TOOLTIP).build(window, cx))
        .on_click(move |_event, window, cx| {
            copy_to_clipboard_with_custom_toast(url, LINK_COPIED_MESSAGE, window, cx);
        })
        .child(icon)
}

pub(super) fn wallet_build_label() -> SharedString {
    SharedString::from(format!(
        "v{} {}",
        env!("CARGO_PKG_VERSION"),
        option_env!("RAILOXIDE_GIT_SHORT_HASH").unwrap_or("unknown")
    ))
}

impl WalletRoot {
    pub(super) fn render_workspace(&self, root: Entity<Self>, window: &Window) -> impl IntoElement {
        if self.logs_open {
            div().size_full().min_w(px(0.0)).min_h(px(0.0)).child(
                v_resizable("wallet-logs-drawer")
                    .with_state(&self.drawer_split)
                    .child(
                        resizable_panel().child(
                            div()
                                .size_full()
                                .min_w(px(0.0))
                                .min_h(px(0.0))
                                .child(self.render_active_content(&root, window)),
                        ),
                    )
                    .child(
                        resizable_panel()
                            .size(LOGS_DRAWER_HEIGHT)
                            .size_range(LOGS_DRAWER_MIN_HEIGHT..LOGS_DRAWER_MAX_HEIGHT)
                            .child(
                                div()
                                    .size_full()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .child(self.render_logs_drawer(root)),
                            ),
                    ),
            )
        } else {
            div()
                .size_full()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(self.render_active_content(&root, window))
        }
    }

    fn render_active_content(&self, root: &Entity<Self>, window: &Window) -> gpui::AnyElement {
        match self.active_activity {
            Activity::Wallet => self.render_wallet_view(root, window).into_any_element(),
            Activity::Broadcaster => self.render_broadcaster_view(root).into_any_element(),
            Activity::AddressBook => self.render_address_book_view(root),
            Activity::Settings => self.render_settings_view().into_any_element(),
        }
    }

    fn render_settings_view(&self) -> impl IntoElement {
        let content = if let Some(editor) = self.settings_editor.as_ref() {
            div().size_full().child(editor.clone()).into_any_element()
        } else {
            div()
                .p(px(24.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(SharedString::from(
                    self.settings_error.as_ref().map_or_else(
                        || "Settings are unavailable".to_string(),
                        ToString::to_string,
                    ),
                ))
                .into_any_element()
        };
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(rgb(theme::SURFACE))
            .p(px(16.0))
            .child(content)
    }

    fn render_wallet_view(&self, root: &Entity<Self>, window: &Window) -> impl IntoElement {
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(rgb(theme::SURFACE_ELEVATED))
            .child(self.render_wallet_header(root))
            .child(self.render_wallet_tabs(root))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .p(px(12.0))
                    .child(self.render_wallet_content(root, window)),
            )
            .children(self.render_wallet_status_bar(root))
    }

    fn render_wallet_status_bar(&self, root: &Entity<Self>) -> Option<gpui::AnyElement> {
        let state = self.chain_states.get(&self.selected_chain)?;
        let counts = self.wallet_status_counts(state.snapshot().map(AsRef::as_ref));
        let syncing = state.is_syncing();
        if !state.renders_table() {
            return None;
        }

        let chips = self.render_wallet_status_chips(root, state, counts);
        if syncing {
            let context = match state {
                ChainUtxoState::Loading { .. } => SyncStatusContext::Loading,
                ChainUtxoState::Syncing { .. } => SyncStatusContext::Syncing,
                ChainUtxoState::Idle
                | ChainUtxoState::Ready { .. }
                | ChainUtxoState::Error { .. } => return None,
            };
            Some(sync_status_bar(context, state.progress(), chips).into_any_element())
        } else {
            Some(ready_status_bar(counts, chips).into_any_element())
        }
    }

    fn wallet_status_counts(
        &self,
        snapshot: Option<&wallet_ops::ListUtxosOutput>,
    ) -> WalletStatusCounts {
        let Some(snapshot) = snapshot else {
            return WalletStatusCounts::default();
        };
        let assets = format_private_asset_rows_from_snapshot(
            snapshot,
            Some(&self.effective_token_registry),
            Some(&self.public_broadcaster_anchor_cache),
        );
        WalletStatusCounts {
            pending_incoming_outputs: snapshot.utxos.iter().filter(|row| row.pending_new).count(),
            pending_outgoing_outputs: snapshot
                .utxos
                .iter()
                .filter(|row| row.pending_spent || row.local_pending_spent)
                .count(),
            pending_poi_assets: assets
                .iter()
                .filter(|asset| should_show_pending_poi_amount(asset.pending_poi_total))
                .count(),
            recoverable_poi_outputs: recoverable_poi_candidate_count(snapshot),
            blocked_shield_outputs: blocked_shield_rescue_display_rows(
                snapshot,
                &self.blocked_shield_rescue_rows,
                &self.blocked_shield_refunds_in_flight,
            )
            .len(),
        }
    }

    fn ppoi_status_for_state(
        &self,
        state: &ChainUtxoState,
        counts: WalletStatusCounts,
    ) -> PresenceStatus {
        ppoi_presence_status(
            state.poi_refreshing(),
            state.poi_refresh_session().is_some(),
            self.poi_cache_service.is_some(),
            self.selected_chain_poi_artifact_progress(),
            counts,
        )
    }

    fn balances_status_for_state(&self, state: &ChainUtxoState) -> PresenceStatus {
        balances_presence_status(
            state.is_syncing(),
            matches!(state, ChainUtxoState::Ready { .. }),
            state.sync_tip(),
            self.selected_chain,
            now_epoch_secs(),
        )
    }

    fn retry_selected_poi_artifact_cache_refresh(&mut self, cx: &mut Context<'_, Self>) {
        let chain_id = self.selected_chain;
        if !self.poi_artifact_cache_retrying_chains.insert(chain_id) {
            return;
        }
        cx.notify();

        let Some(service) = self.poi_cache_service.clone() else {
            self.poi_artifact_cache_retrying_chains.remove(&chain_id);
            cx.notify();
            return;
        };
        cx.spawn(async move |root, cx| {
            let started = service.retry_poi_artifact_cache_refresh(chain_id).await;
            if !started {
                tracing::debug!(
                    chain_id,
                    "skipping POI artifact cache retry without active cache service"
                );
                let _ = root.update(cx, |root, cx| {
                    root.poi_artifact_cache_retrying_chains.remove(&chain_id);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn render_wallet_status_chips(
        &self,
        root: &Entity<Self>,
        state: &ChainUtxoState,
        counts: WalletStatusCounts,
    ) -> Vec<gpui::AnyElement> {
        let ppoi_status = self.ppoi_status_for_state(state, counts);
        let balances_status = self.balances_status_for_state(state);
        let mut chips = Vec::new();

        if counts.ppoi_attention_count() > 0 {
            chips.push(self.render_ppoi_status_indicator(root, ppoi_status, counts));
        } else {
            chips.push(
                render_ppoi_status_hover_target(root, "wallet-status-ppoi")
                    .child(status_presence_text("PPOI", ppoi_status))
                    .into_any_element(),
            );
        }
        chips.push(
            render_balances_status_hover_target(root, "wallet-status-balances")
                .child(status_presence_text("Balances", balances_status))
                .into_any_element(),
        );
        chips
    }

    fn render_ppoi_status_indicator(
        &self,
        root: &Entity<Self>,
        status: PresenceStatus,
        counts: WalletStatusCounts,
    ) -> gpui::AnyElement {
        render_ppoi_status_hover_target(root, "wallet-status-ppoi-hover")
            .child(
                Badge::new()
                    .count(counts.ppoi_attention_count())
                    .color(rgb(ppoi_attention_badge_color(counts)))
                    .child(
                        status_presence_text("PPOI", status)
                            .pr(px(12.0))
                            .into_any_element(),
                    ),
            )
            .into_any_element()
    }

    fn render_wallet_tabs(&self, root: &Entity<Self>) -> impl IntoElement {
        let selected_index = WalletTab::ALL
            .iter()
            .position(|tab| *tab == self.active_wallet_tab)
            .unwrap_or(0);
        let tab_root = root.clone();
        let pending_walletconnect_requests = self.walletconnect_pending_request_count();

        TabBar::new("wallet-tabs")
            .underline()
            .w_full()
            .flex_none()
            .px(px(14.0))
            .selected_index(selected_index)
            .on_click(move |index, _window, cx| {
                let Some(tab) = WalletTab::ALL.get(*index).copied() else {
                    return;
                };
                tab_root.update(cx, |root, cx| {
                    root.select_wallet_tab(tab, cx);
                });
            })
            .children(WalletTab::ALL.into_iter().map(|tab| {
                Tab::new()
                    .min_w(px(92.0))
                    .label(tab.label())
                    .prefix(
                        Icon::empty()
                            .path(tab.icon_path())
                            .with_size(px(18.0))
                            .text_color(rgb(theme::TEXT)),
                    )
                    .when(
                        tab == WalletTab::Public
                            && self.active_wallet_tab != WalletTab::Public
                            && pending_walletconnect_requests > 0,
                        |tab| {
                            tab.suffix(walletconnect_tab_attention_badge(
                                pending_walletconnect_requests,
                            ))
                        },
                    )
            }))
    }

    fn render_wallet_content(&self, root: &Entity<Self>, window: &Window) -> gpui::AnyElement {
        match self.active_wallet_tab {
            WalletTab::Private => self.render_private_assets_body(root),
            WalletTab::Public => self.render_public_wallet_body(root),
            WalletTab::Activity => self.render_utxo_body(root, window).into_any_element(),
        }
    }

    pub(super) fn render_chain_error_body(&self, root: &Entity<Self>, message: &str) -> gpui::Div {
        let can_retry =
            matches!(self.vault_state, VaultState::ViewUnlocked) && self.view_session.is_some();
        let retry_root = root.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .max_w(px(520.0))
                    .text_color(rgb(theme::DANGER))
                    .text_align(gpui::TextAlign::Center)
                    .child(SharedString::from(message.to_owned())),
            )
            .when(can_retry, |this| {
                this.child(
                    app_button("wallet-chain-retry-sync", "Retry sync")
                        .outline()
                        .small()
                        .on_click(move |_event, _window, cx| {
                            retry_root.update(cx, |root, cx| {
                                if root.view_session.is_none() {
                                    return;
                                }
                                let chain_id = root.selected_chain;
                                let overrides = chain_load_overrides();
                                root.start_chain_load(chain_id, &overrides, true, cx);
                            });
                        }),
                )
            })
    }

    fn render_logs_drawer(&self, root: Entity<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(rgb(theme::SURFACE_ELEVATED))
            .border_t_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .px(px(12.0))
                    .bg(rgb(theme::SURFACE))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(img(icons::logs_icon_path()).size(px(16.0)).flex_none())
                    .child(
                        div()
                            .ml(px(8.0))
                            .text_color(rgb(theme::TEXT))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Logs"),
                    )
                    .child(div().flex_1())
                    .child(
                        app_button_base("close-wallet-logs-drawer")
                            .ghost()
                            .xsmall()
                            .tooltip("Hide logs")
                            .icon(IconName::Close)
                            .on_click(move |_event, _window, cx| {
                                root.update(cx, |root, cx| {
                                    root.logs_open = false;
                                    cx.notify();
                                });
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(self.logs.clone()),
            )
    }
}

fn status_presence_text(label: &'static str, status: PresenceStatus) -> gpui::Div {
    div()
        .h(px(24.0))
        .px_1()
        .flex()
        .items_center()
        .gap_1()
        .text_color(rgb(theme::TEXT))
        .child(status_presence_dot(status))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(label),
        )
}

fn render_balances_status_hover_target(
    root: &Entity<WalletRoot>,
    id: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let tooltip_root = root.clone();
    div().id(id).hoverable_tooltip(move |_window, cx| {
        let root = tooltip_root.clone();
        cx.new(|cx| BalancesStatusHoverCard::new(root, cx)).into()
    })
}

struct BalancesStatusHoverCard {
    root: Entity<WalletRoot>,
}

impl BalancesStatusHoverCard {
    fn new(root: Entity<WalletRoot>, cx: &mut Context<'_, Self>) -> Self {
        cx.observe(&root, |_this, _root, cx| cx.notify()).detach();
        Self { root }
    }
}

impl Render for BalancesStatusHoverCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let now = now_epoch_secs();
        let (status, labels, sync_tip, indexed_catch_up, issue, counts, network_mode) = {
            let root = self.root.read(cx);
            let chain_id = root.selected_chain;
            let state = root.chain_states.get(&chain_id);
            let counts = root
                .wallet_status_counts(state.and_then(ChainUtxoState::snapshot).map(AsRef::as_ref));
            let labels = state.and_then(|state| {
                let context = match state {
                    ChainUtxoState::Loading { .. } => SyncStatusContext::Loading,
                    ChainUtxoState::Syncing { .. } => SyncStatusContext::Syncing,
                    ChainUtxoState::Idle
                    | ChainUtxoState::Ready { .. }
                    | ChainUtxoState::Error { .. } => return None,
                };
                Some(sync_status_labels(context, state.progress()))
            });
            let status = state.map_or(PresenceStatus::Unknown, |state| {
                root.balances_status_for_state(state)
            });
            let sync_tip = state.and_then(ChainUtxoState::sync_tip);
            let indexed_catch_up = sync_tip.and_then(|tip| tip.indexed_catch_up);
            let issue = state
                .filter(|state| matches!(state, ChainUtxoState::Ready { .. }))
                .and_then(|_| balance_sync_issue(sync_tip, chain_id, now));
            (
                status,
                labels,
                sync_tip,
                indexed_catch_up,
                issue,
                counts,
                root.http.network_mode(),
            )
        };
        let color = presence_status_color(status);

        div()
            .w(px(360.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::SURFACE))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap_3()
            .text_size(APP_TEXT_SIZE)
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_presence_dot(status).flex_none())
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(color))
                            .child(balances_hover_heading(status, labels.as_ref(), issue)),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(balances_hover_detail(
                        status,
                        labels.as_ref(),
                        issue,
                        network_mode,
                    )),
            )
            .when_some(labels.as_ref(), |this, labels| {
                this.child(render_balance_sync_progress_section(labels))
            })
            .when_some(indexed_catch_up, |this, catch_up| {
                this.child(render_balance_indexed_catch_up_note(catch_up))
            })
            .when_some(sync_tip, |this, sync_tip| {
                this.child(render_balance_sync_tip_section(sync_tip, now))
            })
            .when_some(balance_pending_detail(counts), |this, detail| {
                this.child(render_status_hover_note_base(
                    "Balance updates pending",
                    &detail,
                    theme::WARNING,
                    0.08,
                ))
            })
    }
}

fn render_ppoi_status_hover_target(
    root: &Entity<WalletRoot>,
    id: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let tooltip_root = root.clone();
    div().id(id).hoverable_tooltip(move |_window, cx| {
        let root = tooltip_root.clone();
        cx.new(|cx| PpoiStatusHoverCard::new(root, cx)).into()
    })
}

struct PpoiStatusHoverCard {
    root: Entity<WalletRoot>,
}

impl PpoiStatusHoverCard {
    fn new(root: Entity<WalletRoot>, cx: &mut Context<'_, Self>) -> Self {
        cx.observe(&root, |_this, _root, cx| cx.notify()).detach();
        Self { root }
    }
}

impl Render for PpoiStatusHoverCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (status, progress, refreshing, counts, retrying) = {
            let root = self.root.read(cx);
            let state = root.chain_states.get(&root.selected_chain);
            let counts = root
                .wallet_status_counts(state.and_then(ChainUtxoState::snapshot).map(AsRef::as_ref));
            let status = state.map_or(PresenceStatus::Unknown, |state| {
                root.ppoi_status_for_state(state, counts)
            });
            let refreshing = state.is_some_and(ChainUtxoState::poi_refreshing);
            (
                status,
                root.selected_chain_poi_artifact_progress().cloned(),
                refreshing,
                counts,
                root.poi_artifact_cache_retrying_chains
                    .contains(&root.selected_chain),
            )
        };
        let color = presence_status_color(status);
        let event_label = ppoi_event_header_label(progress.as_ref());

        div()
            .w(px(360.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::SURFACE))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap_3()
            .text_size(APP_TEXT_SIZE)
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_presence_dot(status).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(color))
                                    .truncate()
                                    .child(ppoi_hover_heading(
                                        status,
                                        progress.as_ref(),
                                        refreshing,
                                    )),
                            )
                            .when_some(event_label, |this, label| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_color(rgb(theme::TEXT_MUTED))
                                        .child(format!("({label})")),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(ppoi_hover_detail(status, progress.as_ref(), refreshing)),
            )
            .when_some(
                progress
                    .as_ref()
                    .filter(|progress| progress.total_lists > 1),
                |this, progress| this.child(render_ppoi_list_progress_section(progress)),
            )
            .when_some(
                progress.as_ref().filter(|progress| !progress.is_ready()),
                |this, progress| {
                    if progress.is_error() {
                        this.child(render_ppoi_artifact_error_section(
                            self.root.clone(),
                            progress,
                            status,
                            retrying,
                        ))
                    } else {
                        this.child(render_ppoi_artifact_progress_section(progress, status))
                    }
                },
            )
            .when(refreshing, |this| {
                this.child(render_ppoi_hover_note(
                    "Refreshing PPOI status",
                    "Checking private-output PPOI status and retrying recoverable outputs.",
                    theme::WARNING,
                ))
            })
            .when(counts.ppoi_attention_count() > 0, |this| {
                this.child(render_ppoi_hover_action_note(
                    self.root.clone(),
                    "Needs review",
                    ppoi_attention_detail(counts),
                    ppoi_attention_hover_color(counts),
                ))
            })
    }
}

fn render_ppoi_artifact_progress_section(
    progress: &PoiArtifactCacheProgress,
    status: PresenceStatus,
) -> gpui::Div {
    let percent = progress.percent();
    let completed_lists = if percent == 100 && progress.total_lists > 0 {
        progress.total_lists
    } else {
        progress.completed_lists.min(progress.total_lists)
    };
    let list_count = if progress.total_lists == 1 {
        "list"
    } else {
        "lists"
    };
    let list_text = if progress.total_lists == 0 {
        "Preparing POI list metadata".to_string()
    } else if progress.total_lists == 1 && completed_lists == 1 {
        "POI list ready".to_string()
    } else {
        format!(
            "{} of {} {} ready",
            completed_lists, progress.total_lists, list_count
        )
    };
    let color = presence_status_color(status);

    div()
        .rounded_md()
        .border_1()
        .border_color(rgb_with_alpha(color, 0.24))
        .bg(rgb_with_alpha(color, 0.05))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(color))
                        .child(ppoi_artifact_phase_label(progress.phase)),
                )
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(color))
                        .child(format!("{percent}%")),
                ),
        )
        .child(
            UiProgress::new()
                .h(px(7.0))
                .value(f32::from(percent))
                .bg(rgb(color)),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(list_text),
        )
        .when(
            progress.current_event_index.is_some() || progress.target_event_index.is_some(),
            |this| {
                this.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(ppoi_event_progress_label(progress)),
                )
            },
        )
        .when_some(progress.current_list_key.as_ref(), |this, list_key| {
            this.child(
                div()
                    .font_family(APP_MONO_FONT_FAMILY)
                    .text_size(px(12.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(format!("List {}", short_poi_list_key(list_key.as_slice()))),
            )
        })
        .when_some(progress.last_error.as_ref(), |this, error| {
            this.child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(format!("Last error: {error}")),
            )
        })
}

fn render_ppoi_artifact_error_section(
    root: Entity<WalletRoot>,
    progress: &PoiArtifactCacheProgress,
    status: PresenceStatus,
    retrying: bool,
) -> gpui::Div {
    let color = if status == PresenceStatus::Error {
        theme::DANGER
    } else {
        theme::WARNING
    };
    let error = progress
        .last_error
        .clone()
        .unwrap_or_else(|| "Artifact cache refresh failed.".to_string());

    div()
        .rounded_md()
        .border_1()
        .border_color(rgb_with_alpha(color, 0.34))
        .bg(rgb_with_alpha(color, 0.05))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(color))
                .child("Last refresh failed"),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(error),
        )
        .child(
            div().flex().justify_end().child(
                app_button("wallet-status-ppoi-retry-artifact-cache", "Retry refresh")
                    .small()
                    .loading(retrying)
                    .disabled(retrying)
                    .on_click(move |_event, _window, cx| {
                        cx.stop_propagation();
                        root.update(cx, |root, cx| {
                            root.retry_selected_poi_artifact_cache_refresh(cx);
                        });
                    }),
            ),
        )
}

fn render_ppoi_hover_note(title: &str, detail: &str, color: u32) -> gpui::Div {
    render_ppoi_hover_note_base(title, detail, color, 0.08)
}

fn render_ppoi_hover_action_note(
    root: Entity<WalletRoot>,
    title: &'static str,
    detail: String,
    color: u32,
) -> gpui::Stateful<gpui::Div> {
    render_ppoi_hover_note_base(title, &detail, color, 0.08)
        .id("wallet-status-ppoi-needs-review")
        .cursor_pointer()
        .hover(move |this| this.bg(rgb_with_alpha(color, 0.14)))
        .on_click(move |_event, window, cx| {
            cx.stop_propagation();
            root.update(cx, |root, cx| {
                root.open_private_pending_status_dialog(window, cx);
            });
        })
}

fn render_ppoi_hover_note_base(title: &str, detail: &str, color: u32, bg_alpha: f32) -> gpui::Div {
    render_status_hover_note_base(title, detail, color, bg_alpha)
}

fn render_status_hover_note_base(
    title: &str,
    detail: &str,
    color: u32,
    bg_alpha: f32,
) -> gpui::Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(color))
        .bg(rgb_with_alpha(color, bg_alpha))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(color))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(detail.to_string()),
        )
}

fn balances_hover_heading(
    status: PresenceStatus,
    labels: Option<&SyncStatusLabels>,
    issue: Option<BalanceSyncIssue>,
) -> String {
    if let Some(issue) = issue {
        return balance_sync_issue_heading(issue).to_string();
    }
    if let Some(labels) = labels {
        return labels.title.clone();
    }
    match status {
        PresenceStatus::Healthy => "Balances ready",
        PresenceStatus::Active => "Balances catching up",
        PresenceStatus::Error => "Balance sync error",
        PresenceStatus::Unknown => "Balances unavailable",
    }
    .to_string()
}

fn balances_hover_detail(
    status: PresenceStatus,
    labels: Option<&SyncStatusLabels>,
    issue: Option<BalanceSyncIssue>,
    network_mode: WalletNetworkMode,
) -> String {
    if let Some(issue) = issue {
        return balance_sync_issue_detail(issue, network_mode);
    }
    if labels.is_some() {
        return "Private balance sync is catching up with chain state.".to_string();
    }
    match status {
        PresenceStatus::Healthy => "Private balances are synced and following chain state.",
        PresenceStatus::Active => "Private balance sync is catching up with chain state.",
        PresenceStatus::Error => "Private balance sync reported an error.",
        PresenceStatus::Unknown => "Private balance sync state is not available yet.",
    }
    .to_string()
}

fn render_balance_sync_progress_section(labels: &SyncStatusLabels) -> gpui::Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb_with_alpha(theme::WARNING, 0.34))
        .bg(rgb_with_alpha(theme::WARNING, 0.05))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    UiProgress::new()
                        .flex_1()
                        .h(px(7.0))
                        .value(f32::from(labels.percent))
                        .bg(rgb(theme::WARNING)),
                )
                .child(
                    div()
                        .w(px(42.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::WARNING))
                        .child(format!("{}%", labels.percent)),
                ),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(labels.detail.clone()),
        )
}

fn render_balance_indexed_catch_up_note(catch_up: WalletIndexedCatchUpStatus) -> gpui::Div {
    render_status_hover_note_base(
        balance_indexed_catch_up_note_title(catch_up.source),
        &balance_indexed_catch_up_note_detail(catch_up.source),
        theme::WARNING,
        0.08,
    )
    .child(render_balance_sync_tip_row(
        "Catch-up range",
        format!("{} -> {}", catch_up.from_block, catch_up.target_block),
    ))
}

fn balance_indexed_catch_up_note_title(source: WalletIndexedCatchUpSource) -> &'static str {
    match source {
        WalletIndexedCatchUpSource::Squid => "Using Squid catch-up",
        WalletIndexedCatchUpSource::IndexedArtifacts => "Using artifact catch-up",
    }
}

fn balance_indexed_catch_up_note_detail(source: WalletIndexedCatchUpSource) -> String {
    match source {
        WalletIndexedCatchUpSource::Squid => {
            "RPC log sync is behind, so balances are catching up from the Squid indexed wallet source."
                .to_string()
        }
        WalletIndexedCatchUpSource::IndexedArtifacts => {
            "Squid catch-up is unavailable, so balances are catching up from verified indexed artifacts."
                .to_string()
        }
    }
}

fn render_balance_sync_tip_section(sync_tip: WalletSyncTip, now_secs: u64) -> gpui::Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb_with_alpha(theme::BORDER, 0.72))
        .bg(rgb_with_alpha(theme::SURFACE_ELEVATED, 0.34))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEXT))
                .child("Chain position"),
        )
        .child(render_balance_sync_tip_row(
            "Wallet state",
            format_block_label(Some(sync_tip.last_scanned_block)),
        ))
        .child(render_balance_sync_tip_row(
            "Safe head",
            format_block_label(sync_tip.safe_head_block),
        ))
        .child(render_balance_sync_tip_row(
            "RPC head",
            format_block_label(sync_tip.head_block),
        ))
        .when_some(
            sync_tip.head_last_advanced_at_unix_secs,
            |this, advanced_at| {
                this.child(render_balance_sync_tip_row(
                    "Head advanced",
                    format!(
                        "{} ago",
                        format_duration_compact(now_secs.saturating_sub(advanced_at))
                    ),
                ))
            },
        )
}

fn render_balance_sync_tip_row(label: &'static str, value: String) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_size(px(12.0))
        .child(
            div()
                .min_w_0()
                .text_color(rgb(theme::TEXT_MUTED))
                .truncate()
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .font_family(APP_MONO_FONT_FAMILY)
                .text_color(rgb(theme::TEXT))
                .child(value),
        )
}

fn balance_sync_issue_heading(issue: BalanceSyncIssue) -> &'static str {
    match issue {
        BalanceSyncIssue::HeadUnavailable => "Balance head unavailable",
        BalanceSyncIssue::HeadStalled { .. } => "Balance source stale",
        BalanceSyncIssue::Lagging { .. } => "Balances lagging",
    }
}

pub(super) fn balance_sync_issue_detail(
    issue: BalanceSyncIssue,
    network_mode: WalletNetworkMode,
) -> String {
    match issue {
        BalanceSyncIssue::HeadUnavailable => "Waiting for chain head updates.".to_string(),
        BalanceSyncIssue::HeadStalled {
            stale_secs,
            threshold_secs: _,
        } => format!(
            "RPC head has not advanced for {}. {}",
            format_duration_compact(stale_secs),
            balance_sync_issue_suggestion(network_mode)
        ),
        BalanceSyncIssue::Lagging {
            lag_blocks,
            threshold_blocks: _,
        } => format!(
            "Wallet state is {lag_blocks} safe-head blocks behind. {}",
            balance_sync_issue_suggestion(network_mode)
        ),
    }
}

fn balance_sync_issue_suggestion(network_mode: WalletNetworkMode) -> &'static str {
    match network_mode {
        WalletNetworkMode::Tor => "Consider generating a new Tor session or using premium RPCs.",
        WalletNetworkMode::Proxy | WalletNetworkMode::Direct => "Consider using premium RPCs.",
    }
}

fn balance_pending_detail(counts: WalletStatusCounts) -> Option<String> {
    let mut parts = Vec::new();
    if counts.pending_incoming_outputs > 0 {
        parts.push(count_label(
            counts.pending_incoming_outputs,
            "incoming output",
        ));
    }
    if counts.pending_outgoing_outputs > 0 {
        parts.push(count_label(
            counts.pending_outgoing_outputs,
            "outgoing output",
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!(
            "{} waiting for confirmation and safe-head finality.",
            parts.join(" · ")
        ))
    }
}

fn format_block_label(block: Option<u64>) -> String {
    block.map_or_else(|| "Waiting".to_string(), |block| format!("block {block}"))
}

fn format_duration_compact(secs: u64) -> String {
    if secs < SECONDS_PER_MINUTE {
        return format!("{secs}s");
    }
    if secs < SECONDS_PER_HOUR {
        return format!("{}m", secs / SECONDS_PER_MINUTE);
    }
    if secs < SECONDS_PER_DAY {
        return format!("{}h", secs / SECONDS_PER_HOUR);
    }
    format!("{}d", secs / SECONDS_PER_DAY)
}

fn ppoi_hover_heading(
    status: PresenceStatus,
    progress: Option<&PoiArtifactCacheProgress>,
    refreshing: bool,
) -> &'static str {
    if let Some(progress) = progress {
        if progress.is_error() {
            return if status == PresenceStatus::Error {
                "PPOI checks blocked"
            } else {
                "Artifact cache refresh failed"
            };
        }
        if progress.is_active() {
            return if progress.phase == PoiArtifactCachePhase::LiveTailing {
                "Following POI event tail"
            } else {
                "Rebuilding local POI artifact cache"
            };
        }
    }
    if refreshing {
        return "Refreshing PPOI status";
    }
    match status {
        PresenceStatus::Healthy => "PPOI ready",
        PresenceStatus::Active => "PPOI catching up",
        PresenceStatus::Error => "PPOI checks blocked",
        PresenceStatus::Unknown => "PPOI status unavailable",
    }
}

fn ppoi_hover_detail(
    status: PresenceStatus,
    progress: Option<&PoiArtifactCacheProgress>,
    refreshing: bool,
) -> &'static str {
    if let Some(progress) = progress {
        if progress.is_error() {
            return if progress.ready_for_wallet_checks {
                "Using last ready cache state."
            } else {
                "No ready local POI cache is available."
            };
        }
        if progress.is_active() {
            return "Private-output PPOI checks wait for this cache before refreshing.";
        }
    }
    if refreshing {
        return "Checking private-output PPOI status and retrying recoverable outputs.";
    }
    match status {
        PresenceStatus::Healthy => "Up to date and following the source.",
        PresenceStatus::Active => "Catching up with the PPOI source.",
        PresenceStatus::Error => "PPOI checks are blocked until the artifact cache rebuilds.",
        PresenceStatus::Unknown => "PPOI source or artifact-cache status is not available yet.",
    }
}

const fn ppoi_artifact_phase_label(phase: PoiArtifactCachePhase) -> &'static str {
    match phase {
        PoiArtifactCachePhase::Idle => "Idle",
        PoiArtifactCachePhase::LoadingPersisted => "Loading persisted cache",
        PoiArtifactCachePhase::Resetting => "Resetting cache",
        PoiArtifactCachePhase::FetchingManifest => "Fetching manifest",
        PoiArtifactCachePhase::DownloadingBase => "Downloading base",
        PoiArtifactCachePhase::ApplyingDeltas => "Applying deltas",
        PoiArtifactCachePhase::SyncingBlockedShields => "Syncing blocked Shields",
        PoiArtifactCachePhase::LiveTailing => "Live tailing",
        PoiArtifactCachePhase::ValidatingRoots => "Validating roots",
        PoiArtifactCachePhase::Ready => "Ready",
        PoiArtifactCachePhase::Error => "Error",
    }
}

fn ppoi_event_progress_label(progress: &PoiArtifactCacheProgress) -> String {
    match (progress.current_event_index, progress.target_event_index) {
        (Some(current), Some(target)) if current >= target => format!("Event {target}"),
        (Some(current), Some(target)) => format!("Event {} of {}", current.min(target), target),
        (Some(current), None) => format!("Event {current}"),
        (None, Some(target)) => format!("Target event {target}"),
        (None, None) => String::new(),
    }
}

fn ppoi_event_header_label(progress: Option<&PoiArtifactCacheProgress>) -> Option<String> {
    let progress = progress?;
    if progress.total_lists != 1 {
        return None;
    }
    if let [list] = progress.list_progress.as_slice() {
        return ppoi_inline_event_label(list.current_event_index, list.target_event_index);
    }
    ppoi_inline_event_label(progress.current_event_index, progress.target_event_index)
}

fn ppoi_inline_event_label(current: Option<u64>, target: Option<u64>) -> Option<String> {
    match (current, target) {
        (Some(current), Some(target)) if current < target => {
            Some(format!("event {current}/{target}"))
        }
        (Some(current), Some(target)) => Some(format!("event {}", current.min(target))),
        (Some(current), None) => Some(format!("event {current}")),
        (None, Some(target)) => Some(format!("event {target}")),
        (None, None) => None,
    }
}

fn render_ppoi_list_progress_section(progress: &PoiArtifactCacheProgress) -> gpui::Div {
    let ready_lists = progress
        .list_progress
        .iter()
        .filter(|progress| progress.ready_for_wallet_checks)
        .count();

    div()
        .rounded_md()
        .border_1()
        .border_color(rgb_with_alpha(theme::BORDER, 0.72))
        .bg(rgb_with_alpha(theme::SURFACE_ELEVATED, 0.34))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::TEXT))
                        .child("POI lists"),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(format!(
                            "{} of {} ready",
                            ready_lists.min(progress.total_lists),
                            progress.total_lists,
                        )),
                ),
        )
        .children(
            progress
                .list_progress
                .iter()
                .map(render_ppoi_list_progress_row),
        )
}

fn render_ppoi_list_progress_row(progress: &PoiArtifactCacheListProgress) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_size(px(12.0))
        .child(
            div()
                .min_w_0()
                .font_family(APP_MONO_FONT_FAMILY)
                .text_color(rgb(theme::TEXT_MUTED))
                .truncate()
                .child(short_poi_list_key(progress.list_key.as_slice())),
        )
        .child(
            div()
                .flex_none()
                .font_family(APP_MONO_FONT_FAMILY)
                .text_color(rgb(theme::TEXT_MUTED))
                .child(ppoi_list_event_label(progress).unwrap_or_else(|| "Not ready".to_string())),
        )
}

fn ppoi_list_event_label(progress: &PoiArtifactCacheListProgress) -> Option<String> {
    match (progress.current_event_index, progress.target_event_index) {
        (Some(current), Some(target)) if current < target => {
            Some(format!("Event {current}/{target}"))
        }
        (Some(current), Some(target)) => Some(format!("Event {}", current.min(target))),
        (Some(current), None) => Some(format!("Event {current}")),
        (None, Some(target)) => Some(format!("Event {target}")),
        (None, None) => None,
    }
}

fn short_poi_list_key(bytes: &[u8]) -> String {
    let encoded = hex::encode(bytes);
    if encoded.len() <= 16 {
        return encoded;
    }
    format!("{}...{}", &encoded[..8], &encoded[encoded.len() - 6..])
}

const fn ppoi_attention_badge_color(counts: WalletStatusCounts) -> u32 {
    if counts.blocked_shield_outputs > 0 {
        theme::DANGER
    } else {
        theme::WARNING_BG
    }
}

const fn ppoi_attention_hover_color(counts: WalletStatusCounts) -> u32 {
    if counts.blocked_shield_outputs > 0 {
        theme::DANGER
    } else {
        theme::WARNING
    }
}

fn ppoi_attention_detail(counts: WalletStatusCounts) -> String {
    if counts.blocked_shield_outputs > 0 && counts.recoverable_poi_outputs > 0 {
        format!(
            "Review {} and {}",
            count_label(counts.blocked_shield_outputs, "blocked Shield output"),
            count_label(counts.recoverable_poi_outputs, "recoverable PPOI output"),
        )
    } else if counts.blocked_shield_outputs > 0 {
        format!(
            "Review {}",
            count_label(counts.blocked_shield_outputs, "blocked Shield output")
        )
    } else {
        format!(
            "Review {}",
            count_label(counts.recoverable_poi_outputs, "recoverable PPOI output")
        )
    }
}

fn status_presence_dot(status: PresenceStatus) -> gpui::Div {
    if status == PresenceStatus::Healthy {
        return healthy_presence_dot();
    }
    div()
        .size(px(7.0))
        .rounded_full()
        .bg(rgb(presence_status_color(status)))
}

fn healthy_presence_dot() -> gpui::Div {
    const SLOT_SIZE: f32 = 15.0;

    div()
        .relative()
        .size(px(SLOT_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .absolute()
                .size(px(9.0))
                .rounded_full()
                .bg(rgb_with_alpha(theme::SUCCESS, 0.38))
                .with_animation(
                    "presence-pulse",
                    Animation::new(Duration::from_secs_f64(2.0))
                        .repeat()
                        .with_easing(bounce(ease_in_out)),
                    |this, delta| {
                        let size = 9.0 + delta * 7.0;
                        let offset = (SLOT_SIZE - size) / 2.0;
                        this.size(px(size))
                            .top(px(offset))
                            .left(px(offset))
                            .opacity(0.52 - delta * 0.34)
                    },
                ),
        )
        .child(div().size(px(6.0)).rounded_full().bg(rgb(theme::SUCCESS)))
}

const fn presence_status_color(status: PresenceStatus) -> u32 {
    match status {
        PresenceStatus::Healthy => theme::SUCCESS,
        PresenceStatus::Active => theme::WARNING,
        PresenceStatus::Error => theme::DANGER,
        PresenceStatus::Unknown => theme::TEXT_MUTED,
    }
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn walletconnect_tab_attention_badge(count: usize) -> impl IntoElement {
    app_status_tag(attention_count_label(count), theme::WARNING)
}

fn attention_count_label(count: usize) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}
