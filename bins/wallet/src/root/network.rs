use std::collections::VecDeque;
use std::fmt::Display;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eyre::WrapErr;
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, div,
    prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{Disableable, Icon, Sizable, alert::Alert, button::ButtonVariants};
use tokio::runtime::Handle;
use tokio::sync::watch;
use ui::controls::{app_button, app_strong_text};
use ui::format::{format_compact_latency, format_relative_age};
use ui::theme::{self, APP_TEXT_SIZE};
use wallet_ops::{
    HttpContext, TorBridgeActivitySnapshot, WalletNetworkHealth, WalletNetworkHealthCause,
    WalletNetworkHealthState, WalletNetworkMode, request_tor_state_reset,
};

use crate::assets::RailgunNetworkStatusIcon;

use super::ui_helpers::{format_decimal_byte_rate, format_decimal_bytes};
use super::{
    NETWORK_HEALTH_REFRESH_INTERVAL, TOR_EXIT_IP_QUERY_TIMEOUT, TOR_EXIT_IP_QUERY_URL,
    TOR_HEALTH_RETRY_TIMEOUT, WalletRoot, format_report_chain, rgb_with_alpha,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum TorExitIpQueryState {
    #[default]
    Idle,
    Querying,
    Success(IpAddr),
    Error(Arc<str>),
}

pub(super) const fn network_health_color(health: &WalletNetworkHealth) -> u32 {
    match (health.mode, health.state) {
        (WalletNetworkMode::Tor, WalletNetworkHealthState::Ready) => theme::SUCCESS,
        (WalletNetworkMode::Tor, WalletNetworkHealthState::Reconnecting) => theme::WARNING,
        (WalletNetworkMode::Tor, WalletNetworkHealthState::Degraded) => theme::DANGER,
        (WalletNetworkMode::Proxy, _) => theme::PRIMARY,
        (WalletNetworkMode::Direct, _) => theme::TEXT_MUTED,
    }
}

const TOR_ACTIVITY_RATE_WINDOW: Duration = Duration::from_secs(5);
const TOR_ACTIVITY_INTERVAL_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug)]
struct DownloadInterval {
    start: Instant,
    end: Instant,
    downloaded_bytes: u64,
}

struct TorBridgeActivitySampler {
    baseline: Option<(u64, u64, Instant)>,
    intervals: VecDeque<DownloadInterval>,
}

impl TorBridgeActivitySampler {
    fn new() -> Self {
        Self {
            baseline: None,
            intervals: VecDeque::new(),
        }
    }

    fn sample(
        &mut self,
        snapshot: Option<&TorBridgeActivitySnapshot>,
        now: Instant,
    ) -> Option<u64> {
        let Some(snapshot) = snapshot else {
            self.reset();
            return None;
        };

        let Some((generation, downloaded_bytes, previous_at)) = self.baseline else {
            self.reset_to(snapshot, now);
            return None;
        };

        if generation != snapshot.generation || snapshot.downloaded_bytes < downloaded_bytes {
            self.reset_to(snapshot, now);
            return None;
        }

        if now <= previous_at {
            self.reset_to(snapshot, now);
            return None;
        }

        self.baseline = Some((snapshot.generation, snapshot.downloaded_bytes, now));
        self.intervals.push_back(DownloadInterval {
            start: previous_at,
            end: now,
            downloaded_bytes: snapshot.downloaded_bytes - downloaded_bytes,
        });
        self.evict_old_intervals(now);
        self.weighted_rate(now)
    }

    fn reset(&mut self) {
        self.baseline = None;
        self.intervals.clear();
    }

    fn reset_to(&mut self, snapshot: &TorBridgeActivitySnapshot, now: Instant) {
        self.baseline = Some((snapshot.generation, snapshot.downloaded_bytes, now));
        self.intervals.clear();
    }

    fn evict_old_intervals(&mut self, now: Instant) {
        let cutoff = now
            .checked_sub(TOR_ACTIVITY_RATE_WINDOW)
            .or_else(|| self.intervals.front().map(|interval| interval.start))
            .unwrap_or(now);
        while self
            .intervals
            .front()
            .is_some_and(|interval| interval.end <= cutoff)
        {
            self.intervals.pop_front();
        }
        while self.intervals.len() > TOR_ACTIVITY_INTERVAL_LIMIT {
            self.intervals.pop_front();
        }
    }

    fn weighted_rate(&self, now: Instant) -> Option<u64> {
        let cutoff = now
            .checked_sub(TOR_ACTIVITY_RATE_WINDOW)
            .or_else(|| self.intervals.front().map(|interval| interval.start))
            .unwrap_or(now);
        let mut weighted_bytes = 0_u128;
        let mut elapsed_nanos = 0_u128;

        for interval in &self.intervals {
            let overlap_start = interval.start.max(cutoff);
            let overlap_end = interval.end.min(now);
            if overlap_end <= overlap_start {
                continue;
            }
            let interval_duration = interval.end.saturating_duration_since(interval.start);
            let overlap_duration = overlap_end.saturating_duration_since(overlap_start);
            let interval_nanos = interval_duration.as_nanos();
            let overlap_nanos = overlap_duration.as_nanos();
            if interval_nanos == 0 || overlap_nanos == 0 {
                continue;
            }

            weighted_bytes = weighted_bytes.saturating_add(
                u128::from(interval.downloaded_bytes).saturating_mul(overlap_nanos)
                    / interval_nanos,
            );
            elapsed_nanos = elapsed_nanos.saturating_add(overlap_nanos);
        }

        if elapsed_nanos == 0 {
            return None;
        }

        Some(
            weighted_bytes
                .saturating_mul(1_000_000_000)
                .checked_div(elapsed_nanos)
                .unwrap_or(0)
                .min(u128::from(u64::MAX)) as u64,
        )
    }
}

fn format_optional_number<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| "--".to_owned(), |value| value.to_string())
}

fn next_tor_exit_ip_query_generation(current: u64) -> u64 {
    current.saturating_add(1)
}

fn tor_exit_ip_query_completion_is_current(
    current_generation: u64,
    completion_generation: u64,
    state: &TorExitIpQueryState,
) -> bool {
    current_generation == completion_generation && matches!(state, TorExitIpQueryState::Querying)
}

impl WalletRoot {
    pub(super) fn spawn_network_health_monitor(&self, cx: &Context<'_, Self>) {
        if self.http.network_mode() != WalletNetworkMode::Tor {
            return;
        }

        let http = self.http.clone();
        let runtime = self.runtime.clone();
        let mut shutdown = self.root_shutdown.subscribe();
        cx.spawn(async move |this, cx| {
            loop {
                tokio::select! {
                    () = cx.background_executor().timer(NETWORK_HEALTH_REFRESH_INTERVAL) => {}
                    should_shutdown = wallet_root_shutdown_requested(&mut shutdown) => {
                        if should_shutdown {
                            break;
                        }
                        continue;
                    }
                }
                let health = http.network_health();
                let Ok(should_retry) = this.update(cx, |root, cx| {
                    let should_retry = health.cause == WalletNetworkHealthCause::TorBootstrap;
                    root.set_network_health(health, cx);
                    should_retry
                }) else {
                    break;
                };

                if should_retry {
                    tokio::select! {
                        () = retry_tor_bootstrap(&http, &runtime) => {}
                        should_shutdown = wallet_root_shutdown_requested(&mut shutdown) => {
                            if should_shutdown {
                                break;
                            }
                            continue;
                        }
                    }
                    let health = http.network_health();
                    if this
                        .update(cx, |root, cx| root.set_network_health(health, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    pub(super) fn spawn_tor_bridge_activity_sampler(&self, cx: &Context<'_, Self>) {
        if self.http.network_mode() != WalletNetworkMode::Tor {
            return;
        }
        let http = self.http.clone();
        let mut shutdown = self.root_shutdown.subscribe();
        cx.spawn(async move |this, cx| {
            let mut sampler = TorBridgeActivitySampler::new();
            loop {
                tokio::select! {
                    () = cx.background_executor().timer(Duration::from_secs(1)) => {}
                    should_shutdown = wallet_root_shutdown_requested(&mut shutdown) => {
                        if should_shutdown {
                            break;
                        }
                        continue;
                    }
                }
                let snapshot = http.tor_bridge_activity_snapshot();
                let rate = sampler.sample(snapshot.as_ref(), Instant::now());
                if this
                    .update(cx, |root, cx| {
                        if snapshot.as_ref().is_some_and(|snapshot| {
                            root.http.tor_session_generation() != snapshot.generation
                        }) {
                            return;
                        }
                        if root.tor_bridge_activity != snapshot || root.tor_download_rate != rate {
                            root.tor_bridge_activity = snapshot;
                            root.tor_download_rate = rate;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn set_network_health(&mut self, health: WalletNetworkHealth, cx: &mut Context<'_, Self>) {
        if self.network_health != health {
            self.network_health = health;
            cx.notify();
        }
    }

    pub(super) fn set_network_status_popover_open(
        &mut self,
        open: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if !open {
            self.network_status_error = None;
            self.invalidate_tor_exit_ip_query();
            self.tor_state_reset_confirming = false;
        }
        if self.network_status_popover_open != open {
            self.network_status_popover_open = open;
            cx.notify();
        } else if !open {
            cx.notify();
        }
    }

    fn start_new_tor_session(&mut self, cx: &mut Context<'_, Self>) {
        match self.http.start_new_tor_session() {
            Ok(generation) => {
                self.network_health = self.http.network_health();
                self.tor_bridge_activity = self.http.tor_bridge_activity_snapshot();
                self.tor_download_rate = None;
                self.invalidate_tor_exit_ip_query();
                let waku_refreshed = super::refresh_active_waku(self.waku_runtime.as_ref());
                let walletconnect_refreshed =
                    self.restart_walletconnect_relay_workers_for_network_session(cx);
                tracing::info!(
                    tor_session_generation = generation,
                    waku_refreshed,
                    walletconnect_refreshed,
                    "started new Tor session"
                );
                self.network_status_error = None;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to start new Tor session");
                self.network_status_error = Some(Arc::from(format_report_chain(&error)));
            }
        }
        cx.notify();
    }

    fn invalidate_tor_exit_ip_query(&mut self) {
        self.tor_exit_ip_query_generation =
            next_tor_exit_ip_query_generation(self.tor_exit_ip_query_generation);
        self.tor_exit_ip_query = TorExitIpQueryState::Idle;
    }

    fn query_tor_exit_ip(&mut self, cx: &mut Context<'_, Self>) {
        if self.http.network_mode() != WalletNetworkMode::Tor
            || matches!(self.tor_exit_ip_query, TorExitIpQueryState::Querying)
        {
            return;
        }

        self.network_status_error = None;
        self.tor_exit_ip_query_generation =
            next_tor_exit_ip_query_generation(self.tor_exit_ip_query_generation);
        let query_generation = self.tor_exit_ip_query_generation;
        self.tor_exit_ip_query = TorExitIpQueryState::Querying;
        cx.notify();

        let Some(proxy_url) = self.http.proxy_url.clone() else {
            self.tor_exit_ip_query = TorExitIpQueryState::Error(Arc::from(
                "Exit IP query requires the built-in Tor SOCKS bridge",
            ));
            cx.notify();
            return;
        };
        let query = self
            .runtime
            .spawn(async move { query_exit_ip_through_tor(proxy_url).await });
        cx.spawn(async move |this, cx| {
            let state = match query.await {
                Ok(Ok(ip)) => TorExitIpQueryState::Success(ip),
                Ok(Err(error)) => {
                    TorExitIpQueryState::Error(Arc::from(format_report_chain(&error)))
                }
                Err(error) => TorExitIpQueryState::Error(Arc::from(format!(
                    "Exit IP query task failed: {error}"
                ))),
            };
            let _ = this.update(cx, |root, cx| {
                if tor_exit_ip_query_completion_is_current(
                    root.tor_exit_ip_query_generation,
                    query_generation,
                    &root.tor_exit_ip_query,
                ) {
                    root.tor_exit_ip_query = state;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn begin_tor_state_reset_confirmation(&mut self, cx: &mut Context<'_, Self>) {
        self.network_status_error = None;
        self.invalidate_tor_exit_ip_query();
        self.tor_state_reset_confirming = true;
        cx.notify();
    }

    fn cancel_tor_state_reset_confirmation(&mut self, cx: &mut Context<'_, Self>) {
        self.tor_state_reset_confirming = false;
        cx.notify();
    }

    fn quit_and_reset_tor_state(&mut self, cx: &mut Context<'_, Self>) {
        match request_tor_state_reset(&self.options.db_path) {
            Ok(marker_path) => {
                tracing::warn!(
                    marker_path = %marker_path.display(),
                    "requested Tor state reset on next wallet startup; quitting wallet"
                );
                cx.quit();
            }
            Err(error) => {
                tracing::warn!(%error, "failed to request Tor state reset");
                self.network_status_error = Some(Arc::from(format_report_chain(&error)));
                self.tor_state_reset_confirming = false;
                cx.notify();
            }
        }
    }
}

pub(super) fn render_network_status_popover_content(
    root: Entity<WalletRoot>,
    health: &WalletNetworkHealth,
    color: u32,
    error: Option<Arc<str>>,
    exit_ip_query: TorExitIpQueryState,
    reset_confirming: bool,
    activity: Option<wallet_ops::TorBridgeActivitySnapshot>,
    download_rate: Option<u64>,
) -> gpui::Div {
    let session_root = root.clone();
    let query_root = root.clone();
    let reset_root = root.clone();
    let cancel_reset_root = root.clone();
    let confirm_reset_root = root;
    let exit_ip_querying = matches!(exit_ip_query, TorExitIpQueryState::Querying);
    let downloaded_bytes = activity.as_ref().map(|snapshot| snapshot.downloaded_bytes);
    let recent_connection_sample_count = activity
        .as_ref()
        .map(|snapshot| snapshot.recent_connection_sample_count);
    let recent_successful_sample_count = activity
        .as_ref()
        .map(|snapshot| snapshot.recent_successful_sample_count);
    let successful_connections = activity
        .as_ref()
        .map(|snapshot| snapshot.successful_connections);
    let failed_connections = activity
        .as_ref()
        .map(|snapshot| snapshot.failed_connections);
    let connecting_streams = activity
        .as_ref()
        .map(|snapshot| snapshot.connecting_streams);
    let active_streams = activity.as_ref().map(|snapshot| snapshot.active_streams);
    let generation = activity.as_ref().map(|snapshot| snapshot.generation);
    let median_setup_duration = activity
        .as_ref()
        .and_then(|snapshot| snapshot.median_setup_duration);
    let median_setup_label =
        median_setup_duration.map_or_else(|| "--".to_owned(), format_compact_latency);
    let last_activity_age = activity
        .as_ref()
        .and_then(|snapshot| snapshot.last_activity_age);
    div()
        .w(px(300.0))
        .flex()
        .flex_col()
        .gap_3()
        .text_size(APP_TEXT_SIZE)
        .text_color(rgb(theme::TEXT))
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(RailgunNetworkStatusIcon::Tor)
                        .small()
                        .text_color(rgb(color)),
                )
                .child(
                    app_strong_text(health.label())
                        .text_size(px(14.0))
                        .text_color(rgb(color)),
                ),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(health.detail.to_string()),
        )
        .when_some(error, |this, error| {
            this.child(Alert::error("wallet-network-status-error", error.to_string()).small())
        })
        .when(health.mode == WalletNetworkMode::Tor, |this| {
            this.child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .bg(rgb(theme::SURFACE))
                    .p(px(10.0))
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(if matches!(
                        health.cause,
                        WalletNetworkHealthCause::TorRuntimeSlow
                            | WalletNetworkHealthCause::TorRuntimeUnreliable
                    ) {
                        "Recent Tor connections are slow or unreliable. New connections and retries use a new session; active requests may finish on the old session. Use New Tor session to recover manually."
                    } else {
                        "Future wallet HTTP/RPC requests use the active Tor session. Waku peers and WalletConnect relay sockets reconnect using the new Tor session."
                    }),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .bg(rgb(theme::SURFACE))
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(app_strong_text("Tor HTTP/RPC activity").text_size(px(13.0)))
                    .child(format!(
                        "Download rate: {}",
                        format_decimal_byte_rate(download_rate)
                    ))
                    .child(format!(
                        "Downloaded this session: {}",
                        downloaded_bytes.map_or_else(
                            || "--".to_owned(),
                            format_decimal_bytes,
                        )
                    ))
                     .child(format!(
                         "Median setup: {} ({} recent successful samples of {} attempts)",
                         median_setup_label,
                         format_optional_number(recent_successful_sample_count),
                        format_optional_number(recent_connection_sample_count),
                    ))
                    .child(format!(
                        "Connections: {} succeeded, {} failed",
                        format_optional_number(successful_connections),
                        format_optional_number(failed_connections),
                    ))
                    .child(format!(
                        "Streams: {} connecting, {} active",
                        format_optional_number(connecting_streams),
                        format_optional_number(active_streams),
                    ))
                    .child(format!(
                        "Generation: {} | Last activity: {}",
                        format_optional_number(generation),
                        last_activity_age.map_or_else(
                            || "--".to_owned(),
                            format_relative_age,
                        ),
                    ))
                    .child("Passive, in-memory statistics from the wallet's internal Tor HTTP/RPC bridge. Destinations and request contents are not recorded. Excludes Waku and Tor network overhead.")
            )
            .child(
                app_button("wallet-network-new-tor-session", "New Tor session")
                    .outline()
                    .small()
                    .on_click(move |_event, _window, cx| {
                        cx.stop_propagation();
                        session_root.update(cx, |root, cx| {
                            root.start_new_tor_session(cx);
                        });
                    }),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .bg(rgb(theme::SURFACE))
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(
                                "Contacts https://ifconfig.me/ip through Tor.",
                            ),
                    )
                    .child(
                        app_button(
                            "wallet-network-query-exit-ip",
                            if exit_ip_querying {
                                "Querying..."
                            } else {
                                "Query exit IP"
                            },
                        )
                        .outline()
                        .small()
                        .loading(exit_ip_querying)
                        .disabled(exit_ip_querying)
                        .on_click(move |_event, _window, cx| {
                            cx.stop_propagation();
                            query_root.update(cx, |root, cx| {
                                root.query_tor_exit_ip(cx);
                            });
                        }),
                    )
                    .when(!matches!(exit_ip_query, TorExitIpQueryState::Idle), |this| {
                        this.child(match exit_ip_query {
                            TorExitIpQueryState::Idle => div().into_any_element(),
                            TorExitIpQueryState::Querying => div()
                                .text_size(px(12.0))
                                .line_height(px(17.0))
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child("Querying exit IP through Tor...")
                                .into_any_element(),
                            TorExitIpQueryState::Success(ip) => div()
                                .text_size(px(12.0))
                                .line_height(px(17.0))
                                .text_color(rgb(theme::SUCCESS))
                                .child(format!("Exit IP: {ip}"))
                                .into_any_element(),
                            TorExitIpQueryState::Error(error) => div()
                                .text_size(px(12.0))
                                .line_height(px(17.0))
                                .text_color(rgb(theme::DANGER))
                                .child(error.to_string())
                                .into_any_element(),
                        })
                    }),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if reset_confirming {
                        theme::DANGER
                    } else {
                        theme::BORDER
                    }))
                    .bg(if reset_confirming {
                        rgb_with_alpha(theme::DANGER, 0.08)
                    } else {
                        rgb(theme::SURFACE)
                    })
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(17.0))
                            .text_color(rgb(if reset_confirming {
                                theme::DANGER
                            } else {
                                theme::TEXT_MUTED
                            }))
                            .child(if reset_confirming {
                                "Clears Tor cache and guard state only. Wallet data is not deleted. The wallet will quit, and Tor state will be reset on next startup."
                            } else {
                                "If Tor hidden-service connectivity gets stuck, reset only Tor cache and guard state on next startup. Wallet data is not deleted."
                            }),
                    )
                    .when(!reset_confirming, |this| {
                        this.child(
                            app_button("wallet-network-reset-tor-state", "Reset Tor state")
                                .outline()
                                .small()
                                .danger()
                                .on_click(move |_event, _window, cx| {
                                    cx.stop_propagation();
                                    reset_root.update(cx, |root, cx| {
                                        root.begin_tor_state_reset_confirmation(cx);
                                    });
                                }),
                        )
                    })
                    .when(reset_confirming, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    app_button("wallet-network-cancel-tor-reset", "Cancel")
                                        .outline()
                                        .small()
                                        .on_click(move |_event, _window, cx| {
                                            cx.stop_propagation();
                                            cancel_reset_root.update(cx, |root, cx| {
                                                root.cancel_tor_state_reset_confirmation(cx);
                                            });
                                        }),
                                )
                                .child(
                                    app_button("wallet-network-confirm-tor-reset", "Quit and reset")
                                        .small()
                                        .danger()
                                        .on_click(move |_event, _window, cx| {
                                            cx.stop_propagation();
                                            confirm_reset_root.update(cx, |root, cx| {
                                                root.quit_and_reset_tor_state(cx);
                                            });
                                        }),
                                ),
                        )
                    }),
            )
        })
}

async fn wallet_root_shutdown_requested(shutdown: &mut watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    shutdown.changed().await.is_err() || *shutdown.borrow()
}

pub(super) async fn query_exit_ip_through_tor(proxy_url: reqwest::Url) -> eyre::Result<IpAddr> {
    let proxy = reqwest::Proxy::all(proxy_url.as_str())
        .wrap_err_with(|| format!("invalid Tor proxy URL {proxy_url}"))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .pool_max_idle_per_host(0)
        .build()
        .wrap_err("build one-shot Tor exit IP query client")?;
    let response = client
        .get(TOR_EXIT_IP_QUERY_URL)
        .timeout(TOR_EXIT_IP_QUERY_TIMEOUT)
        .send()
        .await
        .wrap_err("query Tor exit IP")?
        .error_for_status()
        .wrap_err("ifconfig.me returned an error status")?;
    let body = response
        .text()
        .await
        .wrap_err("read Tor exit IP response")?;
    let value = body.trim();
    value
        .parse::<IpAddr>()
        .wrap_err_with(|| format!("ifconfig.me returned a non-IP response: {value:?}"))
}

pub(super) async fn retry_tor_bootstrap(http: &HttpContext, runtime: &Handle) {
    let Some(arti_client) = http.arti_client() else {
        return;
    };

    let retry = runtime.spawn(async move {
        tokio::time::timeout(TOR_HEALTH_RETRY_TIMEOUT, arti_client.bootstrap()).await
    });
    match retry.await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::debug!(%error, "Tor bootstrap retry failed during health check");
        }
        Ok(Err(_elapsed)) => {
            tracing::debug!(
                timeout_secs = TOR_HEALTH_RETRY_TIMEOUT.as_secs(),
                "Tor bootstrap retry still pending during health check"
            );
        }
        Err(error) => {
            tracing::warn!(%error, "Tor bootstrap retry task failed during health check");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity_snapshot(generation: u64, downloaded_bytes: u64) -> TorBridgeActivitySnapshot {
        TorBridgeActivitySnapshot {
            generation,
            downloaded_bytes,
            connecting_streams: 0,
            active_streams: 0,
            successful_connections: 0,
            failed_connections: 0,
            recent_connection_sample_count: 0,
            recent_successful_sample_count: 0,
            median_setup_duration: None,
            last_activity_age: None,
        }
    }

    #[test]
    fn download_rate_sampler_reports_first_sample_and_idle_zero() {
        let mut sampler = TorBridgeActivitySampler::new();
        let start = Instant::now();
        let first = activity_snapshot(1, 100);

        assert_eq!(sampler.sample(Some(&first), start), None);
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(1, 100)),
                start + Duration::from_secs(1)
            ),
            Some(0)
        );
    }

    #[test]
    fn download_rate_sampler_smooths_weighted_intervals_and_clips_oldest() {
        let start = Instant::now();
        let mut sampler = TorBridgeActivitySampler::new();
        assert_eq!(sampler.sample(Some(&activity_snapshot(1, 0)), start), None);
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(1, 100)),
                start + Duration::from_secs(1)
            ),
            Some(100)
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(1, 300)),
                start + Duration::from_secs(2)
            ),
            Some(150)
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(1, 600)),
                start + Duration::from_secs(6)
            ),
            Some(100)
        );

        let mut partial_sampler = TorBridgeActivitySampler::new();
        assert_eq!(
            partial_sampler.sample(Some(&activity_snapshot(1, 0)), start),
            None
        );
        assert_eq!(
            partial_sampler.sample(
                Some(&activity_snapshot(1, 100)),
                start + Duration::from_secs(4),
            ),
            Some(25)
        );
        assert_eq!(
            partial_sampler.sample(
                Some(&activity_snapshot(1, 200)),
                start + Duration::from_secs(6),
            ),
            Some(35)
        );

        let mut long_sampler = TorBridgeActivitySampler::new();
        assert_eq!(
            long_sampler.sample(Some(&activity_snapshot(1, 0)), start),
            None
        );
        assert_eq!(
            long_sampler.sample(
                Some(&activity_snapshot(1, 1_000)),
                start + Duration::from_secs(10),
            ),
            Some(100)
        );
        assert!(long_sampler.intervals.len() <= TOR_ACTIVITY_INTERVAL_LIMIT);

        let mut bounded_sampler = TorBridgeActivitySampler::new();
        assert_eq!(
            bounded_sampler.sample(Some(&activity_snapshot(1, 0)), start),
            None
        );
        for index in 1..=32 {
            let _ = bounded_sampler.sample(
                Some(&activity_snapshot(1, index * 100)),
                start + Duration::from_secs(index),
            );
        }
        assert!(bounded_sampler.intervals.len() <= TOR_ACTIVITY_INTERVAL_LIMIT);
    }

    #[test]
    fn download_rate_sampler_resets_on_missing_generation_and_rollback() {
        let start = Instant::now();
        let mut sampler = TorBridgeActivitySampler::new();
        assert_eq!(
            sampler.sample(Some(&activity_snapshot(1, 100)), start),
            None
        );
        assert_eq!(sampler.sample(None, start + Duration::from_secs(1)), None);
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(1, 200)),
                start + Duration::from_secs(2)
            ),
            None
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(2, 0)),
                start + Duration::from_secs(3)
            ),
            None
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(2, 20)),
                start + Duration::from_secs(4)
            ),
            Some(20)
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(2, 10)),
                start + Duration::from_secs(5)
            ),
            None
        );
        assert_eq!(
            sampler.sample(
                Some(&activity_snapshot(2, 30)),
                start + Duration::from_secs(6)
            ),
            Some(20)
        );
    }

    #[test]
    fn exit_ip_query_completion_requires_current_query_token() {
        let generation = next_tor_exit_ip_query_generation(7);
        let querying = TorExitIpQueryState::Querying;
        assert!(tor_exit_ip_query_completion_is_current(
            generation, generation, &querying
        ));
        assert!(!tor_exit_ip_query_completion_is_current(
            generation,
            generation - 1,
            &querying
        ));
        assert!(!tor_exit_ip_query_completion_is_current(
            generation,
            generation,
            &TorExitIpQueryState::Idle
        ));
        assert_eq!(next_tor_exit_ip_query_generation(u64::MAX), u64::MAX);
    }
}
