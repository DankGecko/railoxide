use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;

const WALLET_SYNC_TIP_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct WalletSessionStore {
    db: Arc<DbStore>,
    sync_manager: Arc<SyncManager>,
}

impl WalletSessionStore {
    pub fn open(db_path: PathBuf) -> Result<Self> {
        let db = Arc::new(DbStore::open(DbConfig { root_dir: db_path }).wrap_err("open local db")?);
        Ok(Self::from_db(db))
    }

    #[must_use]
    pub fn from_db(db: Arc<DbStore>) -> Self {
        let sync_manager = Arc::new(SyncManager::new(Arc::clone(&db)));

        Self { db, sync_manager }
    }

    pub async fn start_view_wallet_session(
        &self,
        request: ViewWalletChainSessionRequest,
        rpc_url_override: Option<Url>,
        http: &HttpContext,
    ) -> Result<WalletSession> {
        self.start_view_wallet_session_with_wait(request, rpc_url_override, http, true)
            .await
    }

    pub async fn start_view_wallet_session_immediate(
        &self,
        request: ViewWalletChainSessionRequest,
        rpc_url_override: Option<Url>,
        http: &HttpContext,
    ) -> Result<WalletSession> {
        self.start_view_wallet_session_with_wait(request, rpc_url_override, http, false)
            .await
    }

    async fn start_view_wallet_session_with_wait(
        &self,
        request: ViewWalletChainSessionRequest,
        rpc_url_override: Option<Url>,
        http: &HttpContext,
        wait_until_ready: bool,
    ) -> Result<WalletSession> {
        let chain_id = request.chain_id;
        let poi_rpc_url = request.poi_rpc_url.clone();
        let synced = setup_synced_view_wallet_with_store(
            request.view_session,
            chain_id,
            request.sync_start_policy,
            request.init_block_number,
            request.sync_to_block,
            request.use_indexed_wallet_catch_up,
            request.effective_chain.clone(),
            request.poi_read_source.clone(),
            request.poi_rpc_url.clone(),
            request.local_poi_caches.clone(),
            request.rewind_wallet_cache,
            rpc_url_override,
            http,
            request.progress_tx.clone(),
            wait_until_ready,
            Arc::clone(&self.db),
            Arc::clone(&self.sync_manager),
        )
        .await?;

        wallet_session_from_view_synced(chain_id, poi_rpc_url, synced).await
    }

    pub async fn shutdown(&self) {
        self.sync_manager.shutdown().await;
    }
}

async fn wallet_session_from_view_synced(
    chain_id: u64,
    poi_rpc_url: Url,
    synced: SyncedViewWallet,
) -> Result<WalletSession> {
    wallet_session_from_parts(
        chain_id,
        poi_rpc_url,
        synced.db,
        synced.sync_manager,
        synced.chain_key,
        synced.start_block,
        synced.handle,
    )
    .await
}

async fn wallet_session_from_parts(
    chain_id: u64,
    poi_rpc_url: Url,
    db: Arc<DbStore>,
    sync_manager: Arc<SyncManager>,
    chain_key: ChainKey,
    start_block: u64,
    handle: WalletHandle,
) -> Result<WalletSession> {
    let mut rev_rx = handle.rev_rx.clone();
    let initial_snapshot = Arc::new(snapshot_from_handle(chain_id, &handle).await);
    let (snapshots_tx, snapshots_rx) = watch::channel(initial_snapshot);
    let cache_key = handle.cache_key.clone();
    let ready_rx = handle.ready_rx.clone();
    let sync_tip_rx =
        spawn_wallet_sync_tip_task(handle.clone(), sync_manager.chain_handle(&chain_key).await);
    let poi_refreshing_rx = handle.poi_refreshing_rx.clone();
    let snapshot_handle = handle.clone();
    tokio::spawn(async move {
        loop {
            if rev_rx.changed().await.is_err() {
                break;
            }
            let snapshot = Arc::new(snapshot_from_handle(chain_id, &snapshot_handle).await);
            if snapshots_tx.send(snapshot).is_err() {
                break;
            }
        }
    });

    Ok(WalletSession {
        chain_id,
        poi_rpc_url,
        cache_key,
        start_block,
        ready_rx,
        snapshots_rx,
        sync_tip_rx,
        poi_refreshing_rx,
        db,
        sync_manager,
        chain_key,
        handle,
    })
}

fn spawn_wallet_sync_tip_task(
    handle: WalletHandle,
    chain_handle: Option<sync_service::ChainHandle>,
) -> watch::Receiver<WalletSyncTip> {
    let now = now_epoch_secs();
    let head_block = chain_handle
        .as_ref()
        .map_or(0, |chain| *chain.head_rx.borrow());
    let safe_head_block = chain_handle
        .as_ref()
        .map_or(0, |chain| *chain.safe_head_rx.borrow());
    let head_last_advanced_at_unix_secs = nonzero_block(head_block).map(|_| now);
    let indexed_catch_up = *handle.indexed_catch_up_rx.borrow();
    let initial_tip = wallet_sync_tip_from_blocks(
        handle.last_scanned(),
        head_block,
        safe_head_block,
        head_last_advanced_at_unix_secs,
        indexed_catch_up,
    );
    let (sync_tip_tx, sync_tip_rx) = watch::channel(initial_tip);

    if let Some(chain_handle) = chain_handle {
        spawn_wallet_sync_tip_with_chain(
            handle,
            chain_handle,
            sync_tip_tx,
            initial_tip,
            head_block,
            head_last_advanced_at_unix_secs,
        );
    } else {
        spawn_wallet_sync_tip_without_chain(handle, sync_tip_tx, initial_tip);
    }

    sync_tip_rx
}

fn spawn_wallet_sync_tip_with_chain(
    handle: WalletHandle,
    chain_handle: sync_service::ChainHandle,
    sync_tip_tx: watch::Sender<WalletSyncTip>,
    mut last_tip: WalletSyncTip,
    mut max_observed_head_block: u64,
    mut head_last_advanced_at_unix_secs: Option<u64>,
) {
    let mut head_rx = chain_handle.head_rx;
    let mut safe_head_rx = chain_handle.safe_head_rx;
    let mut indexed_catch_up_rx = handle.indexed_catch_up_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(WALLET_SYNC_TIP_REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let force_send = tokio::select! {
                _ = interval.tick() => true,
                changed = head_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
                changed = safe_head_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
                changed = indexed_catch_up_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
            };

            let head_block = *head_rx.borrow();
            update_head_advance_state(
                &mut max_observed_head_block,
                &mut head_last_advanced_at_unix_secs,
                head_block,
                now_epoch_secs(),
            );
            let tip = wallet_sync_tip_from_blocks(
                handle.last_scanned(),
                head_block,
                *safe_head_rx.borrow(),
                head_last_advanced_at_unix_secs,
                *indexed_catch_up_rx.borrow(),
            );
            if !publish_wallet_sync_tip(&sync_tip_tx, &mut last_tip, tip, force_send) {
                break;
            }
        }
    });
}

fn spawn_wallet_sync_tip_without_chain(
    handle: WalletHandle,
    sync_tip_tx: watch::Sender<WalletSyncTip>,
    mut last_tip: WalletSyncTip,
) {
    let mut indexed_catch_up_rx = handle.indexed_catch_up_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(WALLET_SYNC_TIP_REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let force_send = tokio::select! {
                _ = interval.tick() => true,
                changed = indexed_catch_up_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
            };
            let tip = WalletSyncTip {
                last_scanned_block: handle.last_scanned(),
                indexed_catch_up: *indexed_catch_up_rx.borrow(),
                ..WalletSyncTip::default()
            };
            if !publish_wallet_sync_tip(&sync_tip_tx, &mut last_tip, tip, force_send) {
                break;
            }
        }
    });
}

fn publish_wallet_sync_tip(
    sync_tip_tx: &watch::Sender<WalletSyncTip>,
    last_tip: &mut WalletSyncTip,
    tip: WalletSyncTip,
    force_send: bool,
) -> bool {
    if !force_send && tip == *last_tip {
        return true;
    }
    if sync_tip_tx.send(tip).is_err() {
        return false;
    }
    *last_tip = tip;
    true
}

fn wallet_sync_tip_from_blocks(
    last_scanned_block: u64,
    head_block: u64,
    safe_head_block: u64,
    head_last_advanced_at_unix_secs: Option<u64>,
    indexed_catch_up: Option<WalletIndexedCatchUpStatus>,
) -> WalletSyncTip {
    WalletSyncTip {
        last_scanned_block,
        head_block: nonzero_block(head_block),
        safe_head_block: nonzero_block(safe_head_block),
        head_last_advanced_at_unix_secs,
        indexed_catch_up,
    }
}

fn update_head_advance_state(
    max_observed_head_block: &mut u64,
    head_last_advanced_at_unix_secs: &mut Option<u64>,
    head_block: u64,
    now_secs: u64,
) {
    if head_block > *max_observed_head_block {
        *max_observed_head_block = head_block;
        *head_last_advanced_at_unix_secs = Some(now_secs);
    }
}

const fn nonzero_block(block: u64) -> Option<u64> {
    if block == 0 { None } else { Some(block) }
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_advance_state_uses_monotonic_observed_head() {
        let mut max_observed_head_block = 100;
        let mut advanced_at = Some(10);

        update_head_advance_state(&mut max_observed_head_block, &mut advanced_at, 99, 20);
        assert_eq!(max_observed_head_block, 100);
        assert_eq!(advanced_at, Some(10));

        update_head_advance_state(&mut max_observed_head_block, &mut advanced_at, 100, 30);
        assert_eq!(max_observed_head_block, 100);
        assert_eq!(advanced_at, Some(10));

        update_head_advance_state(&mut max_observed_head_block, &mut advanced_at, 101, 40);
        assert_eq!(max_observed_head_block, 101);
        assert_eq!(advanced_at, Some(40));
    }

    #[test]
    fn wallet_sync_tip_publish_forces_time_driven_refresh() {
        let tip = WalletSyncTip {
            last_scanned_block: 100,
            head_block: Some(112),
            safe_head_block: Some(100),
            head_last_advanced_at_unix_secs: Some(10),
            indexed_catch_up: None,
        };
        let (tx, mut rx) = watch::channel(tip);
        let mut last_tip = tip;

        assert!(publish_wallet_sync_tip(&tx, &mut last_tip, tip, false));
        assert!(!rx.has_changed().expect("watch receiver open"));

        assert!(publish_wallet_sync_tip(&tx, &mut last_tip, tip, true));
        assert!(rx.has_changed().expect("watch receiver notified"));
        assert_eq!(*rx.borrow_and_update(), tip);

        let advanced_tip = WalletSyncTip {
            last_scanned_block: 101,
            ..tip
        };
        assert!(publish_wallet_sync_tip(
            &tx,
            &mut last_tip,
            advanced_tip,
            false,
        ));
        assert_eq!(last_tip, advanced_tip);
        assert!(rx.has_changed().expect("watch receiver notified"));
    }
}
