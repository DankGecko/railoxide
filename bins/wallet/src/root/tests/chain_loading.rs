use super::*;
use crate::root::chain_load::{WalletSyncLifecycle, WalletSyncLifecycleCleanupWaitGroup};
use wallet_ops::{
    PoiArtifactCachePhase, PoiArtifactCacheProgress, WalletIndexedCatchUpSource,
    WalletIndexedCatchUpStatus, WalletNetworkMode, WalletSessionStore, WalletSyncTip,
};

fn poi_artifact_progress(
    phase: PoiArtifactCachePhase,
    ready_for_wallet_checks: bool,
) -> PoiArtifactCacheProgress {
    PoiArtifactCacheProgress {
        chain_id: 1,
        phase,
        completed_lists: usize::from(ready_for_wallet_checks),
        total_lists: 1,
        current_list_key: None,
        current_event_index: None,
        target_event_index: None,
        list_progress: Vec::new(),
        ready_for_wallet_checks,
        last_error: None,
    }
}

#[test]
fn chain_load_uses_default_sync_options() {
    let overrides = super::chain_load_overrides();

    assert_eq!(overrides.init_block_number, None);
    assert_eq!(overrides.sync_to_block, None);
    assert_eq!(overrides.sync_start_policy, None);
    assert!(overrides.use_indexed_wallet_catch_up);
    assert!(!overrides.rewind_wallet_cache);
}

#[tokio::test]
async fn wallet_sync_lifecycle_cleanup_aborts_in_flight_startups() {
    struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    let mut lifecycle = WalletSyncLifecycle::new();
    let registration = lifecycle.prepare_startup(1);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let join = tokio::spawn(async move {
        let _notify = NotifyOnDrop(Some(dropped_tx));
        let _ = started_tx.send(());
        std::future::pending::<()>().await;
    });
    lifecycle.track_startup(&registration, join);

    tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .expect("startup notification timeout")
        .expect("startup notification");
    assert!(lifecycle.is_current_startup(1, registration.generation, registration.task_id));
    let cleanup = lifecycle.invalidate();

    assert!(!lifecycle.is_current_startup(1, registration.generation, registration.task_id));
    let report = cleanup.shutdown().await.expect("shutdown lifecycle");
    assert_eq!(report.stopped_startup_tasks, 1);
    assert!(!report.shut_down_session_store);
    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("abort notification timeout")
        .expect("abort notification");
}

#[test]
fn wallet_sync_lifecycle_keeps_syncing_installation_current_until_ready() {
    let mut lifecycle = WalletSyncLifecycle::new();
    let registration = lifecycle.prepare_startup(1);

    lifecycle.finish_startup_after_session_installation(
        1,
        registration.generation,
        registration.task_id,
        false,
    );
    assert!(lifecycle.is_current_startup(1, registration.generation, registration.task_id));

    lifecycle.finish_startup_after_session_installation(
        1,
        registration.generation,
        registration.task_id,
        true,
    );
    assert!(!lifecycle.is_current_startup(1, registration.generation, registration.task_id));
}

#[tokio::test]
async fn wallet_sync_lifecycle_cleanup_detects_late_initialized_store() {
    let root_dir = temp_wallet_db_root("wallet-sync-lifecycle-late-store");
    let mut lifecycle = WalletSyncLifecycle::new();
    let registration = lifecycle.prepare_startup(1);
    let old_session_store = Arc::clone(&registration.session_store);
    let cleanup = lifecycle.invalidate();
    let store = Arc::new(WalletSessionStore::open(root_dir.clone()).expect("open session store"));

    assert!(old_session_store.set(store).is_ok());
    let report = cleanup.shutdown().await.expect("shutdown lifecycle");

    assert_eq!(report.stopped_startup_tasks, 0);
    assert!(report.shut_down_session_store);
    let _ = fs::remove_dir_all(root_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wallet_sync_lifecycle_cleanup_timeout_keeps_cleanup_retryable() {
    let mut lifecycle = WalletSyncLifecycle::new();
    let registration = lifecycle.prepare_startup(1);
    let join = tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(100));
    });
    lifecycle.track_startup(&registration, join);
    let cleanup = lifecycle.invalidate();
    let cleanup_task = cleanup.spawn(&tokio::runtime::Handle::current());

    let error = WalletSyncLifecycleCleanupWaitGroup::new(vec![cleanup_task.clone()])
        .shutdown_with_timeout(Duration::from_millis(1))
        .await
        .expect_err("cleanup should time out");

    assert_eq!(error, "timed out stopping wallet sync; try again");
    let report = WalletSyncLifecycleCleanupWaitGroup::new(vec![cleanup_task])
        .shutdown_with_timeout(Duration::from_secs(1))
        .await
        .expect("cleanup should still complete");
    assert_eq!(report.stopped_startup_tasks, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wallet_sync_lifecycle_wait_group_reuses_timed_out_cleanup() {
    let mut lifecycle = WalletSyncLifecycle::new();
    let registration = lifecycle.prepare_startup(1);
    let join = tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(100));
    });
    lifecycle.track_startup(&registration, join);
    let cleanup_task = lifecycle
        .invalidate()
        .spawn(&tokio::runtime::Handle::current());

    let first_wait = WalletSyncLifecycleCleanupWaitGroup::new(vec![cleanup_task.clone()])
        .shutdown_with_timeout(Duration::from_millis(1))
        .await;
    assert_eq!(
        first_wait.expect_err("cleanup should time out"),
        "timed out stopping wallet sync; try again"
    );

    let report = WalletSyncLifecycleCleanupWaitGroup::new(vec![cleanup_task])
        .shutdown_with_timeout(Duration::from_secs(1))
        .await
        .expect("retry should wait on retained cleanup");
    assert_eq!(report.stopped_startup_tasks, 1);
}

#[test]
fn repair_cache_block_parses_zero_as_deployment() {
    assert_eq!(parse_repair_cache_block("0"), Ok(None));
    assert_eq!(parse_repair_cache_block(""), Ok(None));
    assert_eq!(parse_repair_cache_block(" 24936249 "), Ok(Some(24936249)));
    assert!(parse_repair_cache_block("nope").is_err());
}

#[test]
fn repair_cache_help_text_only_mentions_hint_when_available() {
    assert!(repair_cache_help_text(true).contains("wallet start block below"));
    assert!(!repair_cache_help_text(false).contains("wallet start block below"));
    assert!(repair_cache_help_text(false).contains("deployment block"));
}

#[test]
fn chain_error_state_preserves_start_block_hint() {
    let state = ChainUtxoState::Error {
        message: Arc::from("sync failed"),
        start_block: Some(24936250),
    };

    assert_eq!(state.start_block(), Some(24936250));
    assert!(!state.renders_table());
}

#[test]
fn loading_summary_uses_sync_stage_and_percent() {
    let commitments =
        SyncProgressUpdate::new(SyncProgressStage::SynchronizingCommitments, 100, 150, 300);
    let preparing =
        SyncProgressUpdate::artifact_chunk(SyncProgressStage::PreparingUtxoIndex, 25, 100, 3, 12);
    let indexing = SyncProgressUpdate::new(SyncProgressStage::IndexingUtxos, 100, 150, 300);

    assert_eq!(
        loading_summary(Some(commitments)),
        "Synchronizing commitments · 25%"
    );
    assert_eq!(
        loading_summary(Some(preparing)),
        "Preparing UTXO index · 25%"
    );
    assert_eq!(loading_summary(Some(indexing)), "Indexing UTXOs · 25%");
    assert_eq!(loading_summary(None), "Preparing wallet sync...");
}

#[test]
fn sync_status_labels_describe_no_progress_context() {
    assert_eq!(
        sync_status_labels(SyncStatusContext::Loading, None),
        SyncStatusLabels {
            title: "Preparing wallet sync".to_string(),
            percent: 0,
            detail: "Connecting to chain and loading local wallet state...".to_string(),
        }
    );
    assert_eq!(
        sync_status_labels(SyncStatusContext::Syncing, None),
        SyncStatusLabels {
            title: "Checking wallet sync".to_string(),
            percent: 0,
            detail: "Checking for new wallet events...".to_string(),
        }
    );
}

#[test]
fn sync_status_labels_use_progress_when_available() {
    let progress = SyncProgressUpdate::new(SyncProgressStage::IndexingUtxos, 100, 150, 300);

    assert_eq!(
        sync_status_labels(SyncStatusContext::Loading, Some(progress)),
        SyncStatusLabels {
            title: "Indexing UTXOs".to_string(),
            percent: 25,
            detail: "Block 150 of 300".to_string(),
        }
    );
}

#[test]
fn loading_chain_state_keeps_utxo_table_available() {
    let state = ChainUtxoState::Loading { progress: None };

    assert!(state.renders_table());
    assert!(state.is_syncing());
    assert!(!matches!(state, ChainUtxoState::Ready { .. }));
    assert!(state.snapshot().is_none());
}

#[test]
fn progress_detail_clamps_current_block() {
    let progress = SyncProgressUpdate::new(SyncProgressStage::IndexingUtxos, 100, 400, 300);

    assert_eq!(progress_detail(progress), "Block 300 of 300");
}

#[test]
fn progress_detail_uses_artifact_chunks_for_utxo_prep() {
    let progress =
        SyncProgressUpdate::artifact_chunk(SyncProgressStage::PreparingUtxoIndex, 58, 100, 7, 12);

    assert_eq!(progress_detail(progress), "Artifact chunk 7 of 12");
}

#[test]
fn progress_detail_describes_pending_artifact_chunks() {
    let progress =
        SyncProgressUpdate::artifact_chunk(SyncProgressStage::PreparingUtxoIndex, 25, 100, 0, 11);

    assert_eq!(
        progress_detail(progress),
        "Downloading 11 artifact chunks..."
    );
}

#[test]
fn progress_detail_describes_artifact_metadata() {
    let progress =
        SyncProgressUpdate::artifact_preparation(SyncProgressStage::PreparingUtxoIndex, 5, 100);

    assert_eq!(progress_detail(progress), "Preparing artifact metadata...");
}

#[test]
fn progress_detail_describes_artifact_apply_completion() {
    let progress =
        SyncProgressUpdate::artifact_applied(SyncProgressStage::SynchronizingCommitments);

    assert_eq!(progress_detail(progress), "Commitment artifacts applied");
}

#[test]
fn progress_detail_describes_commitment_tail() {
    let progress = SyncProgressUpdate::commitment_tail(200, 225, 300);

    assert_eq!(
        progress_detail(progress),
        "Checking commitment tail: block 225 of 300"
    );
}

#[test]
fn wallet_status_presence_classifies_sync_and_ppoi_health() {
    let ppoi_attention = WalletStatusCounts {
        recoverable_poi_outputs: 2,
        ..WalletStatusCounts::default()
    };
    let blocked_shield_attention = WalletStatusCounts {
        blocked_shield_outputs: 1,
        ..WalletStatusCounts::default()
    };

    assert_eq!(
        ppoi_presence_status(true, true, false, None, WalletStatusCounts::default()),
        PresenceStatus::Active
    );
    assert_eq!(
        ppoi_presence_status(false, true, false, None, WalletStatusCounts::default()),
        PresenceStatus::Healthy
    );
    assert_eq!(
        ppoi_presence_status(false, false, false, None, WalletStatusCounts::default()),
        PresenceStatus::Unknown
    );
    assert_eq!(
        ppoi_presence_status(false, true, true, None, WalletStatusCounts::default()),
        PresenceStatus::Unknown
    );

    let active_cache = poi_artifact_progress(PoiArtifactCachePhase::ApplyingDeltas, false);
    let ready_cache = poi_artifact_progress(PoiArtifactCachePhase::Ready, true);
    let usable_error = poi_artifact_progress(PoiArtifactCachePhase::Error, true);
    let blocking_error = poi_artifact_progress(PoiArtifactCachePhase::Error, false);
    assert_eq!(
        ppoi_presence_status(
            false,
            true,
            true,
            Some(&active_cache),
            WalletStatusCounts::default()
        ),
        PresenceStatus::Active
    );
    assert_eq!(
        ppoi_presence_status(
            false,
            true,
            true,
            Some(&ready_cache),
            WalletStatusCounts::default()
        ),
        PresenceStatus::Healthy
    );
    assert_eq!(
        ppoi_presence_status(
            false,
            true,
            true,
            Some(&usable_error),
            WalletStatusCounts::default()
        ),
        PresenceStatus::Active
    );
    assert_eq!(
        ppoi_presence_status(
            false,
            true,
            true,
            Some(&blocking_error),
            WalletStatusCounts {
                pending_poi_assets: 1,
                ..WalletStatusCounts::default()
            }
        ),
        PresenceStatus::Error
    );
    assert_eq!(
        ppoi_presence_status(
            false,
            true,
            true,
            Some(&blocking_error),
            WalletStatusCounts {
                blocked_shield_outputs: 1,
                ..WalletStatusCounts::default()
            }
        ),
        PresenceStatus::Active
    );
    assert_eq!(ppoi_attention.ppoi_attention_count(), 2);
    assert_eq!(blocked_shield_attention.ppoi_attention_count(), 1);
}

#[test]
fn balance_sync_presence_degrades_for_stalled_or_lagging_heads() {
    let now = 1_000;
    let fresh = WalletSyncTip {
        last_scanned_block: 990,
        head_block: Some(1_012),
        safe_head_block: Some(1_000),
        head_last_advanced_at_unix_secs: Some(now - 30),
        indexed_catch_up: None,
    };

    assert_eq!(balance_stale_timeout_secs(1), 120);
    assert_eq!(balance_lag_threshold_blocks(1), 10);
    assert_eq!(balance_stale_timeout_secs(137), 45);
    assert_eq!(balance_lag_threshold_blocks(137), 22);
    assert_eq!(balance_sync_issue(Some(fresh), 1, now), None);
    assert_eq!(
        balances_presence_status(false, true, Some(fresh), 1, now),
        PresenceStatus::Healthy
    );

    let stalled = WalletSyncTip {
        head_last_advanced_at_unix_secs: Some(now - 121),
        ..fresh
    };
    assert_eq!(
        balance_sync_issue(Some(stalled), 1, now),
        Some(BalanceSyncIssue::HeadStalled {
            stale_secs: 121,
            threshold_secs: 120,
        })
    );
    assert_eq!(
        balances_presence_status(false, true, Some(stalled), 1, now),
        PresenceStatus::Active
    );

    let lagging = WalletSyncTip {
        last_scanned_block: 989,
        ..fresh
    };
    assert_eq!(
        balance_sync_issue(Some(lagging), 1, now),
        Some(BalanceSyncIssue::Lagging {
            lag_blocks: 11,
            threshold_blocks: 10,
        })
    );
    assert_eq!(
        balances_presence_status(false, true, Some(lagging), 1, now),
        PresenceStatus::Active
    );

    let indexed_catch_up = WalletSyncTip {
        indexed_catch_up: Some(WalletIndexedCatchUpStatus {
            source: WalletIndexedCatchUpSource::Squid,
            from_block: 990,
            target_block: 1_000,
        }),
        ..fresh
    };
    assert_eq!(balance_sync_issue(Some(indexed_catch_up), 1, now), None);
    assert_eq!(
        balances_presence_status(false, true, Some(indexed_catch_up), 1, now),
        PresenceStatus::Active
    );

    assert_eq!(
        balance_sync_issue(None, 1, now),
        Some(BalanceSyncIssue::HeadUnavailable)
    );
    assert_eq!(
        balances_presence_status(false, true, None, 1, now),
        PresenceStatus::Unknown
    );
    assert_eq!(
        balances_presence_status(true, false, None, 1, now),
        PresenceStatus::Active
    );
}

#[test]
fn balance_sync_issue_detail_suggests_network_remedies() {
    let lagging = BalanceSyncIssue::Lagging {
        lag_blocks: 186,
        threshold_blocks: 45,
    };
    let stalled = BalanceSyncIssue::HeadStalled {
        stale_secs: 60,
        threshold_secs: 45,
    };

    assert_eq!(
        balance_sync_issue_detail(lagging, WalletNetworkMode::Tor),
        "Wallet state is 186 safe-head blocks behind. Consider generating a new Tor session or using premium RPCs."
    );
    assert_eq!(
        balance_sync_issue_detail(lagging, WalletNetworkMode::Direct),
        "Wallet state is 186 safe-head blocks behind. Consider using premium RPCs."
    );
    assert_eq!(
        balance_sync_issue_detail(stalled, WalletNetworkMode::Proxy),
        "RPC head has not advanced for 1m. Consider using premium RPCs."
    );
    assert_eq!(
        balance_sync_issue_detail(BalanceSyncIssue::HeadUnavailable, WalletNetworkMode::Tor),
        "Waiting for chain head updates."
    );
}

#[test]
fn ready_wallet_status_labels_prioritize_actionable_private_attention() {
    assert_eq!(
        ready_wallet_status_labels(WalletStatusCounts::default()),
        SyncStatusLabels {
            title: "Wallet ready".to_string(),
            percent: 100,
            detail: "Private wallet synced and ready".to_string(),
        }
    );
    assert_eq!(
        ready_wallet_status_labels(WalletStatusCounts {
            blocked_shield_outputs: 1,
            recoverable_poi_outputs: 3,
            ..WalletStatusCounts::default()
        }),
        SyncStatusLabels {
            title: "Private assets need attention".to_string(),
            percent: 100,
            detail: "1 blocked Shield output needs attention".to_string(),
        }
    );
    assert_eq!(
        ready_wallet_status_labels(WalletStatusCounts {
            recoverable_poi_outputs: 3,
            ..WalletStatusCounts::default()
        }),
        SyncStatusLabels {
            title: "PPOI recovery available".to_string(),
            percent: 100,
            detail: "3 outputs can retry PPOI recovery".to_string(),
        }
    );
    assert_eq!(
        ready_wallet_status_labels(WalletStatusCounts {
            pending_incoming_outputs: 1,
            pending_outgoing_outputs: 2,
            pending_poi_assets: 1,
            ..WalletStatusCounts::default()
        }),
        SyncStatusLabels {
            title: "Private balance update pending".to_string(),
            percent: 100,
            detail: "1 incoming output · 2 outgoing outputs · 1 PPOI-pending asset".to_string(),
        }
    );
}

#[test]
fn ready_wallet_status_text_is_hidden_for_all_ready_states() {
    assert!(!ready_wallet_status_shows_text(
        WalletStatusCounts::default()
    ));
    assert!(!ready_wallet_status_shows_text(WalletStatusCounts {
        pending_incoming_outputs: 1,
        ..WalletStatusCounts::default()
    }));
    assert!(!ready_wallet_status_shows_text(WalletStatusCounts {
        recoverable_poi_outputs: 1,
        ..WalletStatusCounts::default()
    }));
    assert!(!ready_wallet_status_shows_text(WalletStatusCounts {
        blocked_shield_outputs: 1,
        ..WalletStatusCounts::default()
    }));
}
