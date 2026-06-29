use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use gpui::{
    AnyElement, Context, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{WindowExt, progress::Progress as UiProgress};
use tokio::runtime::Handle;
use tokio::sync::{OnceCell, oneshot, watch};
use ui::theme::{self, APP_TEXT_SIZE};
use wallet_ops::{
    DesktopWalletSyncStartPolicy, HttpContext, ListUtxosOutput, PoiArtifactCacheProgress,
    PoiCacheService, PoiReadSource, SyncProgressUnit, SyncProgressUpdate,
    ViewWalletChainSessionRequest, WalletSessionStore, WalletSyncTip,
    vault::{DesktopVaultStore, WalletSource},
};

use super::utxo::should_focus_utxo_table;
use super::{BroadcasterActivityTab, WalletRoot, WalletTab, count_label};

pub(super) enum ChainUtxoState {
    Idle,
    Loading {
        progress: Option<SyncProgressUpdate>,
    },
    Syncing {
        snapshot: Arc<ListUtxosOutput>,
        progress: Option<SyncProgressUpdate>,
        session: Arc<wallet_ops::WalletSession>,
        sync_tip: WalletSyncTip,
        poi_refreshing: bool,
    },
    Ready {
        snapshot: Arc<ListUtxosOutput>,
        session: Arc<wallet_ops::WalletSession>,
        sync_tip: WalletSyncTip,
        poi_refreshing: bool,
    },
    Error {
        message: Arc<str>,
        start_block: Option<u64>,
    },
}

const WALLET_SYNC_STARTUP_SUPERSEDED: &str = "wallet sync startup superseded";
const MERKLE_RESET_SYNC_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct WalletSyncLifecycle {
    generation: Arc<AtomicU64>,
    next_task_id: u64,
    session_store: Arc<OnceCell<Arc<WalletSessionStore>>>,
    startup_tasks: BTreeMap<u64, WalletSyncStartupTask>,
    current_task_by_chain: BTreeMap<u64, u64>,
}

pub(super) struct WalletSyncStartupRegistration {
    pub(super) chain_id: u64,
    pub(super) generation: u64,
    pub(super) task_id: u64,
    pub(super) generation_token: Arc<AtomicU64>,
    pub(super) session_store: Arc<OnceCell<Arc<WalletSessionStore>>>,
}

struct WalletSyncStartupTask {
    chain_id: u64,
    generation: u64,
    task_id: u64,
    join: tokio::task::JoinHandle<()>,
}

pub(super) struct WalletSyncLifecycleCleanup {
    startup_tasks: Vec<WalletSyncStartupTask>,
    session_store: Arc<OnceCell<Arc<WalletSessionStore>>>,
}

#[derive(Clone)]
pub(super) struct WalletSyncLifecycleCleanupTask {
    completed_rx: watch::Receiver<Option<WalletSyncLifecycleCleanupReport>>,
}

pub(super) struct WalletSyncLifecycleCleanupWaitGroup {
    tasks: Vec<WalletSyncLifecycleCleanupTask>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct WalletSyncLifecycleCleanupReport {
    pub(super) stopped_startup_tasks: usize,
    pub(super) failed_startup_tasks: usize,
    pub(super) shut_down_session_store: bool,
}

impl WalletSyncLifecycle {
    pub(super) fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            next_task_id: 0,
            session_store: Arc::new(OnceCell::new()),
            startup_tasks: BTreeMap::new(),
            current_task_by_chain: BTreeMap::new(),
        }
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(super) fn prepare_startup(&mut self, chain_id: u64) -> WalletSyncStartupRegistration {
        let generation = self.current_generation();
        self.next_task_id = self.next_task_id.wrapping_add(1).max(1);
        let task_id = self.next_task_id;

        for task in self
            .startup_tasks
            .values()
            .filter(|task| task.chain_id == chain_id && task.generation == generation)
        {
            task.join.abort();
        }
        self.current_task_by_chain.insert(chain_id, task_id);

        WalletSyncStartupRegistration {
            chain_id,
            generation,
            task_id,
            generation_token: Arc::clone(&self.generation),
            session_store: Arc::clone(&self.session_store),
        }
    }

    pub(super) fn track_startup(
        &mut self,
        registration: &WalletSyncStartupRegistration,
        join: tokio::task::JoinHandle<()>,
    ) {
        self.startup_tasks.insert(
            registration.task_id,
            WalletSyncStartupTask {
                chain_id: registration.chain_id,
                generation: registration.generation,
                task_id: registration.task_id,
                join,
            },
        );
    }

    pub(super) fn is_current_startup(&self, chain_id: u64, generation: u64, task_id: u64) -> bool {
        self.current_generation() == generation
            && self
                .current_task_by_chain
                .get(&chain_id)
                .is_some_and(|current| *current == task_id)
    }

    pub(super) fn finish_startup(&mut self, chain_id: u64, generation: u64, task_id: u64) {
        if self
            .startup_tasks
            .get(&task_id)
            .is_some_and(|task| task.chain_id == chain_id && task.generation == generation)
        {
            self.startup_tasks.remove(&task_id);
        }
        if self
            .current_task_by_chain
            .get(&chain_id)
            .is_some_and(|current| *current == task_id)
        {
            self.current_task_by_chain.remove(&chain_id);
        }
    }

    pub(super) fn finish_startup_after_session_installation(
        &mut self,
        chain_id: u64,
        generation: u64,
        task_id: u64,
        ready: bool,
    ) {
        if self
            .startup_tasks
            .get(&task_id)
            .is_some_and(|task| task.chain_id == chain_id && task.generation == generation)
        {
            self.startup_tasks.remove(&task_id);
        }
        if ready {
            if self
                .current_task_by_chain
                .get(&chain_id)
                .is_some_and(|current| *current == task_id)
            {
                self.current_task_by_chain.remove(&chain_id);
            }
        }
    }

    pub(super) fn invalidate(&mut self) -> WalletSyncLifecycleCleanup {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.current_task_by_chain.clear();
        let startup_tasks = std::mem::take(&mut self.startup_tasks)
            .into_values()
            .collect::<Vec<_>>();
        let session_store = std::mem::replace(&mut self.session_store, Arc::new(OnceCell::new()));
        for task in &startup_tasks {
            task.join.abort();
        }
        WalletSyncLifecycleCleanup {
            startup_tasks,
            session_store,
        }
    }
}

impl WalletSyncLifecycleCleanup {
    #[cfg(test)]
    pub(super) async fn shutdown(self) -> Result<WalletSyncLifecycleCleanupReport, String> {
        Ok(self.shutdown_inner().await)
    }

    pub(super) fn spawn(self, runtime: &Handle) -> WalletSyncLifecycleCleanupTask {
        let (completed_tx, completed_rx) = watch::channel(None);
        runtime.spawn(async move {
            let report = self.shutdown_inner().await;
            let _ = completed_tx.send(Some(report));
        });
        WalletSyncLifecycleCleanupTask { completed_rx }
    }

    async fn shutdown_inner(self) -> WalletSyncLifecycleCleanupReport {
        for task in &self.startup_tasks {
            task.join.abort();
        }

        let stopped_startup_tasks = self.startup_tasks.len();
        let mut failed_startup_tasks = 0;
        for task in self.startup_tasks {
            match task.join.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    failed_startup_tasks += 1;
                    tracing::warn!(
                        chain_id = task.chain_id,
                        task_id = task.task_id,
                        %error,
                        "wallet sync startup task failed during cleanup"
                    );
                }
            }
        }

        let shut_down_session_store = if let Some(store) = self.session_store.get().cloned() {
            store.shutdown().await;
            true
        } else {
            false
        };

        WalletSyncLifecycleCleanupReport {
            stopped_startup_tasks,
            failed_startup_tasks,
            shut_down_session_store,
        }
    }
}

impl WalletSyncLifecycleCleanupTask {
    pub(super) fn is_finished(&self) -> bool {
        self.completed_rx.borrow().is_some()
    }

    async fn wait(mut self) -> Result<WalletSyncLifecycleCleanupReport, String> {
        loop {
            let report = { *self.completed_rx.borrow() };
            if let Some(report) = report {
                return Ok(report);
            }
            self.completed_rx
                .changed()
                .await
                .map_err(|_| "wallet sync cleanup task ended before completion".to_string())?;
        }
    }
}

impl WalletSyncLifecycleCleanupWaitGroup {
    pub(super) fn new(tasks: Vec<WalletSyncLifecycleCleanupTask>) -> Self {
        Self { tasks }
    }

    pub(super) async fn shutdown_for_merkle_reset(
        self,
    ) -> Result<WalletSyncLifecycleCleanupReport, String> {
        self.shutdown_with_timeout(MERKLE_RESET_SYNC_SHUTDOWN_TIMEOUT)
            .await
    }

    pub(super) async fn shutdown_with_timeout(
        self,
        timeout: Duration,
    ) -> Result<WalletSyncLifecycleCleanupReport, String> {
        tokio::time::timeout(timeout, self.wait())
            .await
            .map_err(|_| "timed out stopping wallet sync; try again".to_string())?
    }

    async fn wait(self) -> Result<WalletSyncLifecycleCleanupReport, String> {
        let mut combined = WalletSyncLifecycleCleanupReport::default();
        for task in self.tasks {
            let report = task.wait().await?;
            combined.stopped_startup_tasks += report.stopped_startup_tasks;
            combined.failed_startup_tasks += report.failed_startup_tasks;
            combined.shut_down_session_store |= report.shut_down_session_store;
        }
        Ok(combined)
    }
}

fn wallet_sync_startup_superseded(generation: &AtomicU64, expected: u64) -> bool {
    generation.load(Ordering::Acquire) != expected
}

fn wallet_sync_startup_superseded_error() -> eyre::Report {
    eyre::eyre!(WALLET_SYNC_STARTUP_SUPERSEDED)
}

impl ChainUtxoState {
    pub(super) const fn snapshot(&self) -> Option<&Arc<ListUtxosOutput>> {
        match self {
            Self::Syncing { snapshot, .. } | Self::Ready { snapshot, .. } => Some(snapshot),
            Self::Idle | Self::Loading { .. } | Self::Error { .. } => None,
        }
    }

    pub(super) const fn progress(&self) -> Option<SyncProgressUpdate> {
        match self {
            Self::Loading { progress } | Self::Syncing { progress, .. } => *progress,
            Self::Idle | Self::Ready { .. } | Self::Error { .. } => None,
        }
    }

    pub(super) fn start_block(&self) -> Option<u64> {
        match self {
            Self::Syncing { session, .. } | Self::Ready { session, .. } => {
                Some(session.start_block)
            }
            Self::Error { start_block, .. } => *start_block,
            Self::Idle | Self::Loading { .. } => None,
        }
    }

    pub(super) const fn renders_table(&self) -> bool {
        matches!(
            self,
            Self::Loading { .. } | Self::Syncing { .. } | Self::Ready { .. }
        )
    }

    pub(super) const fn is_syncing(&self) -> bool {
        matches!(self, Self::Loading { .. } | Self::Syncing { .. })
    }

    pub(super) const fn poi_refreshing(&self) -> bool {
        match self {
            Self::Syncing { poi_refreshing, .. } | Self::Ready { poi_refreshing, .. } => {
                *poi_refreshing
            }
            Self::Idle | Self::Loading { .. } | Self::Error { .. } => false,
        }
    }

    pub(super) fn poi_refresh_session(&self) -> Option<Arc<wallet_ops::WalletSession>> {
        match self {
            Self::Syncing { session, .. } | Self::Ready { session, .. } => Some(session.clone()),
            Self::Idle | Self::Loading { .. } | Self::Error { .. } => None,
        }
    }

    pub(super) const fn sync_tip(&self) -> Option<WalletSyncTip> {
        match self {
            Self::Syncing { sync_tip, .. } | Self::Ready { sync_tip, .. } => Some(*sync_tip),
            Self::Idle | Self::Loading { .. } | Self::Error { .. } => None,
        }
    }

    pub(super) const fn private_action_forms_available(&self) -> bool {
        matches!(self, Self::Syncing { .. } | Self::Ready { .. })
    }

    pub(super) const fn private_action_generation_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SyncStatusContext {
    Loading,
    Syncing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SyncStatusLabels {
    pub(super) title: String,
    pub(super) percent: u8,
    pub(super) detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PresenceStatus {
    Healthy,
    Active,
    Error,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BalanceSyncIssue {
    HeadUnavailable,
    HeadStalled {
        stale_secs: u64,
        threshold_secs: u64,
    },
    Lagging {
        lag_blocks: u64,
        threshold_blocks: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct WalletStatusCounts {
    pub(super) pending_incoming_outputs: usize,
    pub(super) pending_outgoing_outputs: usize,
    pub(super) pending_poi_assets: usize,
    pub(super) recoverable_poi_outputs: usize,
    pub(super) blocked_shield_outputs: usize,
}

impl WalletStatusCounts {
    pub(super) const fn has_private_attention(self) -> bool {
        self.pending_incoming_outputs > 0
            || self.pending_outgoing_outputs > 0
            || self.pending_poi_assets > 0
            || self.recoverable_poi_outputs > 0
            || self.blocked_shield_outputs > 0
    }

    pub(super) const fn ppoi_attention_count(self) -> usize {
        self.recoverable_poi_outputs + self.blocked_shield_outputs
    }

    pub(super) const fn has_ppoi_blocking_checks(self) -> bool {
        self.pending_poi_assets > 0 || self.recoverable_poi_outputs > 0
    }
}

pub(super) fn balances_presence_status(
    syncing: bool,
    ready: bool,
    sync_tip: Option<WalletSyncTip>,
    chain_id: u64,
    now_secs: u64,
) -> PresenceStatus {
    if syncing {
        return PresenceStatus::Active;
    }
    if !ready {
        return PresenceStatus::Unknown;
    }
    if sync_tip.is_some_and(|tip| tip.indexed_catch_up.is_some()) {
        return PresenceStatus::Active;
    }
    match balance_sync_issue(sync_tip, chain_id, now_secs) {
        None => PresenceStatus::Healthy,
        Some(BalanceSyncIssue::HeadUnavailable) => PresenceStatus::Unknown,
        Some(BalanceSyncIssue::HeadStalled { .. } | BalanceSyncIssue::Lagging { .. }) => {
            PresenceStatus::Active
        }
    }
}

pub(super) fn balance_sync_issue(
    sync_tip: Option<WalletSyncTip>,
    chain_id: u64,
    now_secs: u64,
) -> Option<BalanceSyncIssue> {
    let Some(sync_tip) = sync_tip else {
        return Some(BalanceSyncIssue::HeadUnavailable);
    };
    if sync_tip.head_block.is_none() {
        return Some(BalanceSyncIssue::HeadUnavailable);
    }
    let Some(safe_head_block) = sync_tip.safe_head_block else {
        return Some(BalanceSyncIssue::HeadUnavailable);
    };
    let Some(head_last_advanced_at) = sync_tip.head_last_advanced_at_unix_secs else {
        return Some(BalanceSyncIssue::HeadUnavailable);
    };

    let threshold_secs = balance_stale_timeout_secs(chain_id);
    let stale_secs = now_secs.saturating_sub(head_last_advanced_at);
    if stale_secs > threshold_secs {
        return Some(BalanceSyncIssue::HeadStalled {
            stale_secs,
            threshold_secs,
        });
    }

    let lag_blocks = safe_head_block.saturating_sub(sync_tip.last_scanned_block);
    let threshold_blocks = balance_lag_threshold_blocks(chain_id);
    if lag_blocks > threshold_blocks {
        return Some(BalanceSyncIssue::Lagging {
            lag_blocks,
            threshold_blocks,
        });
    }

    None
}

pub(super) const fn balance_block_time_secs(chain_id: u64) -> u64 {
    match chain_id {
        1 => 12,
        56 => 3,
        137 => 2,
        42161 => 1,
        _ => 12,
    }
}

pub(super) const fn balance_stale_timeout_secs(chain_id: u64) -> u64 {
    let timeout = balance_block_time_secs(chain_id) * 10;
    if timeout < 45 { 45 } else { timeout }
}

pub(super) const fn balance_lag_threshold_blocks(chain_id: u64) -> u64 {
    let block_time = balance_block_time_secs(chain_id);
    let threshold = balance_stale_timeout_secs(chain_id) / block_time;
    if threshold < 2 { 2 } else { threshold }
}

pub(super) fn ppoi_presence_status(
    refreshing: bool,
    source_available: bool,
    artifact_cache_expected: bool,
    artifact_progress: Option<&PoiArtifactCacheProgress>,
    counts: WalletStatusCounts,
) -> PresenceStatus {
    if !source_available {
        return PresenceStatus::Unknown;
    }

    if let Some(progress) = artifact_progress {
        if progress.is_error() {
            return if !progress.ready_for_wallet_checks && counts.has_ppoi_blocking_checks() {
                PresenceStatus::Error
            } else {
                PresenceStatus::Active
            };
        }
        if progress.is_active() {
            return PresenceStatus::Active;
        }
        if !progress.is_ready() {
            return PresenceStatus::Unknown;
        }
    } else if artifact_cache_expected {
        return if refreshing {
            PresenceStatus::Active
        } else {
            PresenceStatus::Unknown
        };
    }

    if refreshing {
        PresenceStatus::Active
    } else {
        PresenceStatus::Healthy
    }
}

pub(super) fn ready_wallet_status_labels(counts: WalletStatusCounts) -> SyncStatusLabels {
    let title = if counts.blocked_shield_outputs > 0 {
        "Private assets need attention"
    } else if counts.recoverable_poi_outputs > 0 {
        "PPOI recovery available"
    } else if counts.has_private_attention() {
        "Private balance update pending"
    } else {
        "Wallet ready"
    };
    SyncStatusLabels {
        title: title.to_string(),
        percent: 100,
        detail: ready_wallet_status_detail(counts),
    }
}

fn ready_wallet_status_detail(counts: WalletStatusCounts) -> String {
    if counts.blocked_shield_outputs > 0 {
        let verb = if counts.blocked_shield_outputs == 1 {
            " needs attention"
        } else {
            " need attention"
        };
        return count_label(counts.blocked_shield_outputs, "blocked Shield output") + verb;
    }
    if counts.recoverable_poi_outputs > 0 {
        return count_label(counts.recoverable_poi_outputs, "output") + " can retry PPOI recovery";
    }
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
    if counts.pending_poi_assets > 0 {
        parts.push(count_label(counts.pending_poi_assets, "PPOI-pending asset"));
    }
    if parts.is_empty() {
        "Private wallet synced and ready".to_string()
    } else {
        parts.join(" · ")
    }
}

impl SyncStatusContext {
    const fn fallback_title(self) -> &'static str {
        match self {
            Self::Loading => "Preparing wallet sync",
            Self::Syncing => "Checking wallet sync",
        }
    }

    const fn fallback_detail(self) -> &'static str {
        match self {
            Self::Loading => "Connecting to chain and loading local wallet state...",
            Self::Syncing => "Checking for new wallet events...",
        }
    }
}

#[derive(Clone)]
pub(super) struct ChainLoadOverrides {
    pub(super) init_block_number: Option<u64>,
    pub(super) sync_to_block: Option<u64>,
    pub(super) sync_start_policy: Option<DesktopWalletSyncStartPolicy>,
    pub(super) use_indexed_wallet_catch_up: bool,
    pub(super) rewind_wallet_cache: bool,
}

pub(super) const fn chain_load_overrides() -> ChainLoadOverrides {
    ChainLoadOverrides {
        init_block_number: None,
        sync_to_block: None,
        sync_start_policy: None,
        use_indexed_wallet_catch_up: true,
        rewind_wallet_cache: false,
    }
}

pub(super) fn wallet_generation_matches(
    selected_wallet_id: Option<&str>,
    active_wallet_generation: u64,
    wallet_id: &str,
    generation: u64,
) -> bool {
    active_wallet_generation == generation && selected_wallet_id == Some(wallet_id)
}

pub(super) fn start_shared_poi_cache_service(
    poi_read_source: &PoiReadSource,
    poi_rpc_url: &reqwest::Url,
    vault_store: Option<&Arc<DesktopVaultStore>>,
    http: &HttpContext,
    runtime: &Handle,
    chain_ids: &[u64],
) -> Option<Arc<PoiCacheService>> {
    let PoiReadSource::IndexedArtifacts(artifact_config) = poi_read_source else {
        return None;
    };
    let Some(vault_store) = vault_store else {
        tracing::warn!("artifact POI cache service disabled because wallet DB is unavailable");
        return None;
    };

    let service = Arc::new(
        PoiCacheService::new(
            vault_store.db(),
            artifact_config.clone(),
            Some(http.client.clone()),
        )
        .with_poi_rpc_url(poi_rpc_url.clone()),
    );
    let startup_service = Arc::clone(&service);
    let chain_ids = chain_ids.to_vec();
    runtime.spawn(async move {
        startup_service.start_chains(chain_ids).await;
    });
    Some(service)
}

pub(super) fn loading_summary(progress: Option<SyncProgressUpdate>) -> String {
    progress.map_or_else(
        || "Preparing wallet sync...".to_string(),
        |progress| format!("{} · {}%", progress.stage.label(), progress.percent()),
    )
}

pub(super) fn sync_status_labels(
    context: SyncStatusContext,
    progress: Option<SyncProgressUpdate>,
) -> SyncStatusLabels {
    SyncStatusLabels {
        title: progress.map_or_else(
            || context.fallback_title().to_string(),
            |progress| progress.stage.label().to_string(),
        ),
        percent: progress.map_or(0, SyncProgressUpdate::percent),
        detail: progress.map_or_else(|| context.fallback_detail().to_string(), progress_detail),
    }
}

pub(super) fn sync_status_bar(
    context: SyncStatusContext,
    progress: Option<SyncProgressUpdate>,
    right_children: Vec<AnyElement>,
) -> gpui::Div {
    let labels = sync_status_labels(context, progress);
    wallet_status_bar(labels, true, true, right_children)
}

pub(super) fn ready_status_bar(
    counts: WalletStatusCounts,
    right_children: Vec<AnyElement>,
) -> gpui::Div {
    wallet_status_bar(
        ready_wallet_status_labels(counts),
        false,
        ready_wallet_status_shows_text(counts),
        right_children,
    )
}

pub(super) const fn ready_wallet_status_shows_text(_counts: WalletStatusCounts) -> bool {
    false
}

fn wallet_status_bar(
    labels: SyncStatusLabels,
    show_progress: bool,
    show_text: bool,
    right_children: Vec<AnyElement>,
) -> gpui::Div {
    div()
        .h(px(36.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_3()
        .px(px(12.0))
        .bg(rgb(theme::SURFACE))
        .border_t_1()
        .border_color(rgb(theme::BORDER))
        .when(show_text, |bar| {
            bar.child(
                div()
                    .min_w(px(170.0))
                    .text_color(rgb(theme::TEXT))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(labels.title)),
            )
        })
        .when(show_progress, |bar| {
            bar.child(
                UiProgress::new()
                    .w(px(190.0))
                    .h(px(6.0))
                    .value(f32::from(labels.percent)),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_color(rgb(theme::INFO))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("{}%", labels.percent))),
            )
        })
        .when(show_text, |bar| {
            bar.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(rgb(theme::TEXT_MUTED))
                    .text_size(APP_TEXT_SIZE)
                    .child(SharedString::from(labels.detail)),
            )
        })
        .when(!show_text, |bar| bar.child(div().flex_1()))
        .when(!right_children.is_empty(), |bar| {
            bar.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .children(right_children),
            )
        })
}

pub(super) fn progress_detail(progress: SyncProgressUpdate) -> String {
    match progress.unit {
        SyncProgressUnit::Block => {}
        SyncProgressUnit::ArtifactPreparation => {
            return "Preparing artifact metadata...".to_string();
        }
        SyncProgressUnit::ArtifactChunk { completed, total } => {
            if total == 0 {
                return "Artifact chunks prepared".to_string();
            }
            if completed == 0 {
                return format!("Downloading {total} artifact chunks...");
            }
            let completed = completed.min(total);
            return format!("Artifact chunk {completed} of {total}");
        }
        SyncProgressUnit::ArtifactApplied => {
            return match progress.stage {
                wallet_ops::SyncProgressStage::SynchronizingCommitments => {
                    "Commitment artifacts applied".to_string()
                }
                wallet_ops::SyncProgressStage::PreparingUtxoIndex => {
                    "UTXO index artifacts prepared".to_string()
                }
                wallet_ops::SyncProgressStage::IndexingUtxos => "Artifacts applied".to_string(),
            };
        }
        SyncProgressUnit::CommitmentTail => {
            let current = progress
                .current_block
                .max(progress.start_block)
                .min(progress.target_block);
            return format!(
                "Checking commitment tail: block {current} of {}",
                progress.target_block
            );
        }
    }
    let current = progress
        .current_block
        .max(progress.start_block)
        .min(progress.target_block);
    format!("Block {current} of {}", progress.target_block)
}

impl WalletRoot {
    pub(super) fn selected_wallet_source(&self) -> WalletSource {
        let Some(selected_wallet_id) = self.selected_wallet_id.as_ref() else {
            return WalletSource::Imported;
        };
        self.wallet_options
            .iter()
            .find(|option| option.wallet_id.as_ref() == selected_wallet_id.as_ref())
            .map_or(WalletSource::Imported, |option| option.source)
    }

    fn selected_wallet_sync_start_policy(&self) -> DesktopWalletSyncStartPolicy {
        let Some(selected_wallet_id) = self.selected_wallet_id.as_ref() else {
            return DesktopWalletSyncStartPolicy::ImportedHistoricalBackfill;
        };
        self.wallet_metadata
            .iter()
            .find(|metadata| metadata.wallet_uuid == selected_wallet_id.as_ref())
            .map_or(
                DesktopWalletSyncStartPolicy::ImportedHistoricalBackfill,
                DesktopWalletSyncStartPolicy::from,
            )
    }

    pub(super) fn selected_chain_wallet_start_block(&self) -> Option<u64> {
        self.chain_states
            .get(&self.selected_chain)
            .and_then(ChainUtxoState::start_block)
    }

    pub(super) fn selected_chain_poi_artifact_progress(&self) -> Option<&PoiArtifactCacheProgress> {
        self.poi_artifact_cache_progress.get(&self.selected_chain)
    }

    pub(super) fn is_active_wallet_generation(&self, wallet_id: &str, generation: u64) -> bool {
        wallet_generation_matches(
            self.selected_wallet_id.as_deref(),
            self.active_wallet_generation,
            wallet_id,
            generation,
        )
    }

    pub(super) fn reset_wallet_scoped_state(&mut self, cx: &mut Context<'_, Self>) {
        self.send_forms.clear();
        self.unshield_forms.clear();
        self.set_broadcaster_preferences(wallet_ops::vault::BroadcasterPreferences::default(), cx);
        self.broadcaster_preference_error = None;
        self.active_broadcaster_tab = BroadcasterActivityTab::default();
        self.clear_public_wallet_runtime_state();
        self.private_action_form = None;
        self.clear_private_broadcaster_progress_state();
        self.broadcaster_picker = None;
        self.blocked_shield_rescue_rows.clear();
        self.blocked_shield_refunds_in_flight.clear();
        self.blocked_shield_rescue_lookup_generation =
            self.blocked_shield_rescue_lookup_generation.wrapping_add(1);
        self.active_wallet_tab = WalletTab::default();
        for state in self.chain_states.values_mut() {
            *state = ChainUtxoState::Idle;
        }
        self.poi_artifact_cache_retrying_chains.clear();
        self.sync_utxo_table(cx);
    }

    pub(super) fn shutdown_wallet_session_store(&mut self) {
        let cleanup = self.wallet_sync_lifecycle.invalidate();
        self.start_wallet_sync_cleanup(cleanup);
    }

    fn start_wallet_sync_cleanup(
        &mut self,
        cleanup: WalletSyncLifecycleCleanup,
    ) -> WalletSyncLifecycleCleanupTask {
        self.prune_finished_wallet_sync_cleanups();
        let task = cleanup.spawn(&self.runtime);
        self.wallet_sync_cleanup_tasks.push(task.clone());
        task
    }

    fn prune_finished_wallet_sync_cleanups(&mut self) {
        self.wallet_sync_cleanup_tasks
            .retain(|cleanup| !cleanup.is_finished());
    }

    fn wallet_sync_cleanup_wait_group(&mut self) -> WalletSyncLifecycleCleanupWaitGroup {
        self.prune_finished_wallet_sync_cleanups();
        WalletSyncLifecycleCleanupWaitGroup::new(self.wallet_sync_cleanup_tasks.clone())
    }

    pub(super) fn begin_merkle_forest_cache_reset(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> WalletSyncLifecycleCleanupWaitGroup {
        self.merkle_forest_cache_resetting = true;
        self.active_wallet_generation = self.active_wallet_generation.wrapping_add(1);
        let cleanup = self.wallet_sync_lifecycle.invalidate();
        self.start_wallet_sync_cleanup(cleanup);
        self.send_forms.clear();
        self.unshield_forms.clear();
        self.private_action_form = None;
        self.clear_private_broadcaster_progress_state();
        self.broadcaster_picker = None;
        for state in self.chain_states.values_mut() {
            *state = ChainUtxoState::Idle;
        }
        self.poi_artifact_cache_retrying_chains.clear();
        self.sync_utxo_table(cx);
        cx.notify();
        self.wallet_sync_cleanup_wait_group()
    }

    pub(super) fn finish_merkle_forest_cache_reset(
        &mut self,
        reset_succeeded: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.merkle_forest_cache_resetting = false;
        self.prune_finished_wallet_sync_cleanups();
        if reset_succeeded && self.view_session.is_some() {
            self.ensure_chain_load(self.selected_chain, cx);
        } else {
            cx.notify();
        }
    }

    fn is_current_chain_load_startup(
        &self,
        wallet_id: &str,
        active_wallet_generation: u64,
        chain_id: u64,
        lifecycle_generation: u64,
        task_id: u64,
    ) -> bool {
        self.is_active_wallet_generation(wallet_id, active_wallet_generation)
            && self.wallet_sync_lifecycle.is_current_startup(
                chain_id,
                lifecycle_generation,
                task_id,
            )
    }

    fn finish_chain_load_startup(
        &mut self,
        chain_id: u64,
        lifecycle_generation: u64,
        task_id: u64,
    ) {
        self.wallet_sync_lifecycle
            .finish_startup(chain_id, lifecycle_generation, task_id);
    }

    pub(super) fn ensure_chain_load(&mut self, chain_id: u64, cx: &mut Context<'_, Self>) {
        let overrides = chain_load_overrides();
        self.start_chain_load(chain_id, &overrides, false, cx);
    }

    pub(super) fn ensure_chain_load_with_start_policy(
        &mut self,
        chain_id: u64,
        sync_start_policy: Option<DesktopWalletSyncStartPolicy>,
        cx: &mut Context<'_, Self>,
    ) {
        let mut overrides = chain_load_overrides();
        overrides.sync_start_policy = sync_start_policy;
        self.start_chain_load(chain_id, &overrides, false, cx);
    }

    pub(super) fn start_chain_load(
        &mut self,
        chain_id: u64,
        overrides: &ChainLoadOverrides,
        force: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(view_session) = self.view_session.clone() else {
            tracing::debug!(
                chain_id,
                "skipping wallet sync without active wallet session"
            );
            return;
        };
        if self.merkle_forest_cache_resetting {
            tracing::debug!(
                chain_id,
                "skipping wallet sync during Merkle forest cache reset"
            );
            return;
        }
        if matches!(
            self.chain_states.get(&chain_id),
            Some(
                ChainUtxoState::Loading { .. }
                    | ChainUtxoState::Syncing { .. }
                    | ChainUtxoState::Ready { .. }
            )
        ) && !force
        {
            return;
        }

        let previous_start_block = self
            .chain_states
            .get(&chain_id)
            .and_then(ChainUtxoState::start_block);

        let previous_session = if force {
            match self.chain_states.remove(&chain_id) {
                Some(
                    ChainUtxoState::Syncing { session, .. } | ChainUtxoState::Ready { session, .. },
                ) => Some(session),
                Some(state) => {
                    self.chain_states.insert(chain_id, state);
                    None
                }
                None => None,
            }
        } else {
            None
        };

        self.chain_states
            .insert(chain_id, ChainUtxoState::Loading { progress: None });
        self.sync_utxo_table(cx);

        let active_wallet_id: Arc<str> = Arc::from(view_session.wallet_id().to_owned());
        let active_wallet_generation = self.active_wallet_generation;
        let (progress_tx, mut progress_rx) = watch::channel(None);
        let request = ViewWalletChainSessionRequest {
            view_session,
            chain_id,
            effective_chain: self.effective_chain_configs.get(&chain_id).cloned(),
            sync_start_policy: overrides
                .sync_start_policy
                .unwrap_or_else(|| self.selected_wallet_sync_start_policy()),
            init_block_number: overrides.init_block_number,
            sync_to_block: overrides.sync_to_block,
            use_indexed_wallet_catch_up: overrides.use_indexed_wallet_catch_up,
            poi_read_source: self.poi_read_source.clone(),
            poi_rpc_url: self.poi_rpc_url.clone(),
            rewind_wallet_cache: overrides.rewind_wallet_cache,
            progress_tx: Some(progress_tx),
            local_poi_caches: None,
        };
        let db_path = self.options.db_path.clone();
        let http = self.http.clone();
        let poi_cache_service = self.poi_cache_service.clone();
        let registration = self.wallet_sync_lifecycle.prepare_startup(chain_id);
        let lifecycle_generation = registration.generation;
        let chain_load_task_id = registration.task_id;
        let lifecycle_generation_token = Arc::clone(&registration.generation_token);
        let session_store = Arc::clone(&registration.session_store);
        let vault_db = self.vault_store.as_ref().map(|store| store.db());
        let (result_tx, result_rx) = oneshot::channel();
        let join = self.runtime.spawn(async move {
            let result = async move {
                if wallet_sync_startup_superseded(&lifecycle_generation_token, lifecycle_generation)
                {
                    return Err(wallet_sync_startup_superseded_error());
                }
                if let Some(previous_session) = previous_session {
                    previous_session.stop().await?;
                }
                let mut request = request;
                if let Some(poi_cache_service) = poi_cache_service.as_ref() {
                    if wallet_sync_startup_superseded(
                        &lifecycle_generation_token,
                        lifecycle_generation,
                    ) {
                        return Err(wallet_sync_startup_superseded_error());
                    }
                    let local_poi_caches = poi_cache_service.start_chain(chain_id).await;
                    if wallet_sync_startup_superseded(
                        &lifecycle_generation_token,
                        lifecycle_generation,
                    ) {
                        return Err(wallet_sync_startup_superseded_error());
                    }
                    request.local_poi_caches = Some(local_poi_caches);
                }
                if wallet_sync_startup_superseded(&lifecycle_generation_token, lifecycle_generation)
                {
                    return Err(wallet_sync_startup_superseded_error());
                }
                let store = session_store
                    .get_or_try_init(|| {
                        let db_path = db_path.clone();
                        let vault_db = vault_db.clone();
                        async move {
                            Ok::<Arc<WalletSessionStore>, eyre::Report>(Arc::new(match vault_db {
                                Some(db) => WalletSessionStore::from_db(db),
                                None => WalletSessionStore::open(db_path)?,
                            }))
                        }
                    })
                    .await?
                    .clone();
                if wallet_sync_startup_superseded(&lifecycle_generation_token, lifecycle_generation)
                {
                    store.shutdown().await;
                    return Err(wallet_sync_startup_superseded_error());
                }
                let session = store
                    .start_view_wallet_session_immediate(request, None, &http)
                    .await?;
                if wallet_sync_startup_superseded(&lifecycle_generation_token, lifecycle_generation)
                {
                    if let Err(error) = session.stop().await {
                        tracing::warn!(
                            chain_id,
                            %error,
                            "failed to stop superseded wallet sync session"
                        );
                    }
                    store.shutdown().await;
                    return Err(wallet_sync_startup_superseded_error());
                }
                Ok(session)
            }
            .await;
            let _ = result_tx.send(result);
        });
        self.wallet_sync_lifecycle
            .track_startup(&registration, join);

        let progress_wallet_id = Arc::clone(&active_wallet_id);
        cx.spawn(async move |this, cx| {
            loop {
                if progress_rx.changed().await.is_err() {
                    break;
                }
                let progress = *progress_rx.borrow();
                let should_continue = this.update(cx, |root, cx| {
                    if !root.is_current_chain_load_startup(
                        progress_wallet_id.as_ref(),
                        active_wallet_generation,
                        chain_id,
                        lifecycle_generation,
                        chain_load_task_id,
                    ) {
                        return false;
                    }
                    match root.chain_states.get_mut(&chain_id) {
                        Some(
                            ChainUtxoState::Loading { progress: state }
                            | ChainUtxoState::Syncing {
                                progress: state, ..
                            },
                        ) => *state = progress,
                        Some(
                            ChainUtxoState::Idle
                            | ChainUtxoState::Ready { .. }
                            | ChainUtxoState::Error { .. },
                        )
                        | None => return false,
                    }
                    cx.notify();
                    true
                });
                if !matches!(should_continue, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        let result_wallet_id = active_wallet_id;
        cx.spawn(async move |this, cx| {
            let session = match result_rx.await {
                Ok(Ok(session)) => Arc::new(session),
                Ok(Err(error)) => {
                    let _ = this.update(cx, |root, cx| {
                        let is_current = root.is_current_chain_load_startup(
                            result_wallet_id.as_ref(),
                            active_wallet_generation,
                            chain_id,
                            lifecycle_generation,
                            chain_load_task_id,
                        );
                        root.finish_chain_load_startup(
                            chain_id,
                            lifecycle_generation,
                            chain_load_task_id,
                        );
                        if !is_current {
                            return;
                        }
                        root.chain_states.insert(
                            chain_id,
                            ChainUtxoState::Error {
                                message: Arc::from(error.to_string()),
                                start_block: previous_start_block,
                            },
                        );
                        if root.selected_chain == chain_id {
                            root.sync_utxo_table(cx);
                        }
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |root, cx| {
                        let is_current = root.is_current_chain_load_startup(
                            result_wallet_id.as_ref(),
                            active_wallet_generation,
                            chain_id,
                            lifecycle_generation,
                            chain_load_task_id,
                        );
                        root.finish_chain_load_startup(
                            chain_id,
                            lifecycle_generation,
                            chain_load_task_id,
                        );
                        if !is_current {
                            return;
                        }
                        root.chain_states.insert(
                            chain_id,
                            ChainUtxoState::Error {
                                message: Arc::from(format!("wallet UTXO task failed: {error}")),
                                start_block: previous_start_block,
                            },
                        );
                        if root.selected_chain == chain_id {
                            root.sync_utxo_table(cx);
                        }
                        cx.notify();
                    });
                    return;
                }
            };

            let is_current = this
                .update(cx, |root, _cx| {
                    let is_current = root.is_current_chain_load_startup(
                        result_wallet_id.as_ref(),
                        active_wallet_generation,
                        chain_id,
                        lifecycle_generation,
                        chain_load_task_id,
                    );
                    if !is_current {
                        root.finish_chain_load_startup(
                            chain_id,
                            lifecycle_generation,
                            chain_load_task_id,
                        );
                    }
                    is_current
                })
                .unwrap_or(false);
            if !is_current {
                if let Err(error) = session.stop().await {
                    tracing::warn!(chain_id, %error, "failed to stop stale wallet sync session");
                }
                return;
            }

            let mut snapshots_rx = session.snapshots_rx.clone();
            let mut ready_rx = session.ready_rx.clone();
            let mut sync_tip_rx = session.sync_tip_rx.clone();
            let mut poi_refreshing_rx = session.poi_refreshing_rx.clone();
            let initial_snapshot = snapshots_rx.borrow().clone();
            let mut ready = *ready_rx.borrow();
            let initial_sync_tip = *sync_tip_rx.borrow();
            let initial_poi_refreshing = *poi_refreshing_rx.borrow();

            let installed = this.update(cx, |root, cx| {
                if !root.is_current_chain_load_startup(
                    result_wallet_id.as_ref(),
                    active_wallet_generation,
                    chain_id,
                    lifecycle_generation,
                    chain_load_task_id,
                ) {
                    root.finish_chain_load_startup(
                        chain_id,
                        lifecycle_generation,
                        chain_load_task_id,
                    );
                    return false;
                }
                root.wallet_sync_lifecycle.finish_startup_after_session_installation(
                    chain_id,
                    lifecycle_generation,
                    chain_load_task_id,
                    ready,
                );
                let progress = root
                    .chain_states
                    .get(&chain_id)
                    .and_then(ChainUtxoState::progress);
                let state = if ready {
                    ChainUtxoState::Ready {
                        snapshot: initial_snapshot.clone(),
                        session: session.clone(),
                        sync_tip: initial_sync_tip,
                        poi_refreshing: initial_poi_refreshing,
                    }
                } else {
                    ChainUtxoState::Syncing {
                        snapshot: initial_snapshot.clone(),
                        progress,
                        session: session.clone(),
                        sync_tip: initial_sync_tip,
                        poi_refreshing: initial_poi_refreshing,
                    }
                };
                root.chain_states.insert(chain_id, state);
                if root.selected_chain == chain_id {
                    root.sync_utxo_table(cx);
                    root.focus_utxo_table_on_render = should_focus_utxo_table(
                        root.active_activity,
                        root.active_wallet_tab,
                        root.chain_states.get(&chain_id),
                    );
                }
                cx.notify();
                true
            });
            if !matches!(installed, Ok(true)) {
                if let Err(error) = session.stop().await {
                    tracing::warn!(chain_id, %error, "failed to stop stale wallet sync session");
                }
                return;
            }

            loop {
                tokio::select! {
                    changed = snapshots_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let snapshot = snapshots_rx.borrow().clone();
                        let should_continue = this.update(cx, |root, cx| {
                            if !root.is_active_wallet_generation(
                                result_wallet_id.as_ref(),
                                active_wallet_generation,
                            ) {
                                return false;
                            }
                            {
                                let Some(state) = root.chain_states.get_mut(&chain_id) else {
                                    return false;
                                };
                                match state {
                                    ChainUtxoState::Syncing { snapshot: current, .. }
                                    | ChainUtxoState::Ready { snapshot: current, .. } => {
                                        *current = snapshot.clone();
                                    }
                                    ChainUtxoState::Idle
                                    | ChainUtxoState::Loading { .. }
                                    | ChainUtxoState::Error { .. } => return false,
                                }
                            }
                            root.refresh_open_form_assets_for_snapshot(&snapshot, cx);
                            if root.selected_chain == chain_id {
                                root.sync_utxo_table(cx);
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(should_continue, Ok(true)) {
                            break;
                        }
                    }
                    changed = ready_rx.changed(), if !ready => {
                        if changed.is_err() {
                            ready = true;
                            continue;
                        }
                        ready = *ready_rx.borrow();
                        if !ready {
                            continue;
                        }
                        let should_continue = this.update(cx, |root, cx| {
                            if !root.is_active_wallet_generation(
                                result_wallet_id.as_ref(),
                                active_wallet_generation,
                            ) {
                                return false;
                            }
                            let Some(state) = root.chain_states.remove(&chain_id) else {
                                return false;
                            };
                            match state {
                                ChainUtxoState::Syncing { snapshot, session, sync_tip, poi_refreshing, .. } => {
                                    root.finish_chain_load_startup(
                                        chain_id,
                                        lifecycle_generation,
                                        chain_load_task_id,
                                    );
                                    root.chain_states.insert(
                                        chain_id,
                                        ChainUtxoState::Ready { snapshot, session, sync_tip, poi_refreshing },
                                    );
                                    root.reschedule_ready_public_broadcaster_cost_estimates(chain_id, cx);
                                    if root.selected_chain == chain_id {
                                        root.sync_utxo_table(cx);
                                    }
                                    cx.notify();
                                    true
                                }
                                ChainUtxoState::Ready { .. } => {
                                    root.chain_states.insert(chain_id, state);
                                    true
                                }
                                ChainUtxoState::Idle
                                | ChainUtxoState::Loading { .. }
                                | ChainUtxoState::Error { .. } => {
                                    root.chain_states.insert(chain_id, state);
                                    false
                                }
                            }
                        });
                        if !matches!(should_continue, Ok(true)) {
                            break;
                        }
                    }
                    changed = sync_tip_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let sync_tip = *sync_tip_rx.borrow();
                        let should_continue = this.update(cx, |root, cx| {
                            if !root.is_active_wallet_generation(
                                result_wallet_id.as_ref(),
                                active_wallet_generation,
                            ) {
                                return false;
                            }
                            let Some(state) = root.chain_states.get_mut(&chain_id) else {
                                return false;
                            };
                            match state {
                                ChainUtxoState::Syncing { sync_tip: state, .. }
                                | ChainUtxoState::Ready { sync_tip: state, .. } => {
                                    *state = sync_tip;
                                }
                                ChainUtxoState::Idle
                                | ChainUtxoState::Loading { .. }
                                | ChainUtxoState::Error { .. } => return false,
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(should_continue, Ok(true)) {
                            break;
                        }
                    }
                    changed = poi_refreshing_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let poi_refreshing = *poi_refreshing_rx.borrow();
                        let should_continue = this.update(cx, |root, cx| {
                            if !root.is_active_wallet_generation(
                                result_wallet_id.as_ref(),
                                active_wallet_generation,
                            ) {
                                return false;
                            }
                            let Some(state) = root.chain_states.get_mut(&chain_id) else {
                                return false;
                            };
                            match state {
                                ChainUtxoState::Syncing { poi_refreshing: state, .. }
                                | ChainUtxoState::Ready { poi_refreshing: state, .. } => {
                                    *state = poi_refreshing;
                                }
                                ChainUtxoState::Idle
                                | ChainUtxoState::Loading { .. }
                                | ChainUtxoState::Error { .. } => return false,
                            }
                            if root.selected_chain == chain_id {
                                root.sync_utxo_table(cx);
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(should_continue, Ok(true)) {
                            break;
                        }
                    }
                }
            }

            let _ = this.update(cx, |root, _cx| {
                root.finish_chain_load_startup(chain_id, lifecycle_generation, chain_load_task_id);
            });
        })
        .detach();
    }

    pub(super) fn select_chain(
        &mut self,
        chain_id: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.selected_chain == chain_id {
            return;
        }
        window.close_all_dialogs(cx);
        self.selected_chain = chain_id;
        self.ui_state.last_chain_id = Some(chain_id);
        self.save_ui_state();
        self.sync_broadcaster_monitor_chain_filter(chain_id, window, cx);
        self.send_forms.clear();
        self.unshield_forms.clear();
        self.private_action_form = None;
        self.clear_private_broadcaster_progress_state();
        self.broadcaster_picker = None;
        self.local_pending_spent_clear_confirming = false;
        self.clear_public_chain_balance_state();
        self.sync_utxo_table(cx);
        if self.active_wallet_tab == WalletTab::Public {
            self.schedule_public_balance_refresh(cx);
        }
        if should_focus_utxo_table(
            self.active_activity,
            self.active_wallet_tab,
            self.chain_states.get(&chain_id),
        ) {
            self.focus_utxo_table_on_render = true;
        }
        if self.view_session.is_some() {
            self.ensure_chain_load(chain_id, cx);
        }
        cx.notify();
    }

    pub(super) fn sync_broadcaster_monitor_chain_filter(
        &self,
        chain_id: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.monitor.update(cx, |monitor, cx| {
            monitor.set_chain_filter(chain_id, window, cx);
        });
    }
}
