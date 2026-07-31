use broadcaster_monitor::{event_channel, shared};
use broadcaster_monitor_waku::{RelayNetworkConfig, RelayNetworkMode, WakuMonitorConfig};
use tokio::sync::watch;

use super::super::private_broadcaster::{
    cancel_private_broadcaster_progress, cancel_public_broadcaster_tasks,
};
use super::*;

#[test]
fn locked_startup_and_proxy_mode_keep_waku_inactive() {
    assert!(!should_start_waku_runtime(
        false,
        false,
        false,
        RelayNetworkMode::Direct
    ));
    assert!(!should_start_waku_runtime(
        true,
        false,
        false,
        RelayNetworkMode::Proxy
    ));
    assert!(active_waku_client(None).is_none());
    assert!(!refresh_active_waku(None));
}

#[test]
fn every_view_unlock_kind_starts_at_most_one_runtime() {
    for unlock_kind in ["software", "hardware-compatible", "empty-vault"] {
        assert!(
            should_start_waku_runtime(true, false, false, RelayNetworkMode::Direct),
            "{unlock_kind} unlock should start Waku"
        );
        assert!(
            !should_start_waku_runtime(true, true, false, RelayNetworkMode::Direct),
            "{unlock_kind} unlock completion must be idempotent"
        );
    }
}

#[test]
fn unlock_time_waku_failure_preserves_unlock_and_has_no_fallback() {
    let view_unlocked = true;
    let result = build_waku_client_if_needed(true, || Err::<(), _>("construction failed"));

    assert_eq!(result, Err("construction failed"));
    assert!(view_unlocked);
    assert!(active_waku_client(None).is_none());
}

#[test]
fn later_delivery_demand_retries_after_client_construction_failure() {
    let mut attempts = 0;
    let first = build_waku_client_if_needed(
        should_start_waku_for_delivery(
            DeliveryMode::PublicBroadcaster,
            true,
            false,
            false,
            RelayNetworkMode::Direct,
        ),
        || {
            attempts += 1;
            Err::<(), _>("transient construction failure")
        },
    );
    assert_eq!(first, Err("transient construction failure"));

    let second = build_waku_client_if_needed(
        should_start_waku_for_delivery(
            DeliveryMode::PublicBroadcaster,
            true,
            false,
            false,
            RelayNetworkMode::Direct,
        ),
        || {
            attempts += 1;
            Ok::<_, &str>("recovered client")
        },
    )
    .expect("later demand retries construction");

    assert_eq!(attempts, 2);
    assert_eq!(second, Some("recovered client"));
}

#[test]
fn later_delivery_demand_retries_after_active_worker_failure() {
    let generation = 11;
    let mut stopping_generation = None;
    let monitor_state = shared();
    let (event_tx, _event_rx) = event_channel(16);
    let action = complete_waku_worker_generation(
        Some(generation),
        &mut stopping_generation,
        generation,
        WakuWorkerCompletionKind::WorkerError,
        true,
        &monitor_state,
        &event_tx,
    );
    assert_eq!(action, WakuWorkerCompletionAction::HandleActiveFailure);

    let retry = build_waku_client_if_needed(
        should_start_waku_for_delivery(
            DeliveryMode::PublicBroadcaster,
            true,
            false,
            stopping_generation.is_some(),
            RelayNetworkMode::Direct,
        ),
        || Ok::<_, &str>("replacement client"),
    )
    .expect("later demand retries after worker failure");

    assert_eq!(retry, Some("replacement client"));
}

#[test]
fn delivery_demand_does_not_retry_when_lifecycle_or_policy_disallows_waku() {
    for (view_unlocked, runtime_stopping, network_mode) in [
        (false, false, RelayNetworkMode::Direct),
        (true, true, RelayNetworkMode::Direct),
        (true, false, RelayNetworkMode::Proxy),
    ] {
        let mut attempted = false;
        let result = build_waku_client_if_needed(
            should_start_waku_for_delivery(
                DeliveryMode::PublicBroadcaster,
                view_unlocked,
                false,
                runtime_stopping,
                network_mode,
            ),
            || {
                attempted = true;
                Ok::<_, &str>(())
            },
        )
        .expect("ineligible demand is ignored");
        assert!(result.is_none());
        assert!(!attempted);
    }

    assert!(!should_start_waku_for_delivery(
        DeliveryMode::ManualCalldata,
        true,
        false,
        false,
        RelayNetworkMode::Direct,
    ));
}

#[test]
fn stop_removes_access_signals_workers_and_clears_monitor_state() {
    let config = WakuMonitorConfig {
        network: RelayNetworkConfig::proxy(reqwest::Client::new()),
        ..WakuMonitorConfig::default()
    };
    let client = config.build_client().expect("proxy Waku delivery client");
    let weak_client = Arc::downgrade(&client);
    let (worker_shutdown, worker_shutdown_rx) = watch::channel(false);
    let mut runtime = Some(WalletWakuRuntime {
        client,
        worker_shutdown,
        generation: 1,
    });
    let monitor_state = shared();
    monitor_state
        .write()
        .upsert_fee(fee_row(1, Address::from([0x11; 20]), "stale-fees"));
    let (event_tx, event_rx) = event_channel(16);
    let initial_rev = event_rx.borrow().to_owned();

    assert!(active_waku_client(runtime.as_ref()).is_some());
    assert!(stop_waku_runtime(&mut runtime, &monitor_state, &event_tx));
    assert!(runtime.is_none());
    assert!(*worker_shutdown_rx.borrow());
    assert!(weak_client.upgrade().is_none());
    assert!(monitor_state.read().fee_rows().is_empty());
    assert!(monitor_state.read().peer_rows().is_empty());
    assert_eq!(
        monitor_state.read().peer_summary(),
        broadcaster_monitor::PeerSummary::default()
    );
    assert!(*event_rx.borrow() > initial_rev);
}

#[test]
fn later_unlock_can_install_a_fresh_runtime_generation() {
    assert!(should_start_waku_runtime(
        true,
        false,
        false,
        RelayNetworkMode::Tor
    ));
    assert!(!should_start_waku_runtime(
        false,
        false,
        false,
        RelayNetworkMode::Tor
    ));
    assert!(!should_start_waku_runtime(
        true,
        false,
        true,
        RelayNetworkMode::Tor
    ));
}

#[test]
fn worker_error_after_stop_allows_a_fresh_runtime_generation() {
    let generation = 7;
    let mut stopping_generation = Some(generation);
    let monitor_state = shared();
    monitor_state
        .write()
        .upsert_fee(fee_row(1, Address::from([0x33; 20]), "late-worker-fees"));
    let (event_tx, event_rx) = event_channel(16);
    let initial_rev = event_rx.borrow().to_owned();

    let action = complete_waku_worker_generation(
        None,
        &mut stopping_generation,
        generation,
        WakuWorkerCompletionKind::WorkerError,
        true,
        &monitor_state,
        &event_tx,
    );

    assert_eq!(
        action,
        WakuWorkerCompletionAction::FinalizedStop { restart: true }
    );
    assert!(stopping_generation.is_none());
    assert!(monitor_state.read().fee_rows().is_empty());
    assert!(*event_rx.borrow() > initial_rev);
    assert!(should_start_waku_runtime(
        true,
        false,
        stopping_generation.is_some(),
        RelayNetworkMode::Direct
    ));
}

#[tokio::test]
async fn lock_cancellation_aborts_in_flight_broadcaster_work() {
    let key = UnshieldAssetKey {
        chain_id: 1,
        token: Address::from([0x22; 20]),
    };
    let mut progress = Some(private_progress_state(
        PrivateSubmissionProgressFlow::PublicBroadcaster,
        key,
    ));
    let task = tokio::spawn(std::future::pending::<()>());
    progress.as_mut().expect("progress").task_abort_handle = Some(task.abort_handle());

    let _ = cancel_private_broadcaster_progress(&mut progress);

    assert!(progress.is_none());
    assert!(task.await.expect_err("task was aborted").is_cancelled());
}

#[tokio::test]
async fn lock_cancellation_aborts_every_tracked_broadcaster_task() {
    let first = tokio::spawn(std::future::pending::<()>());
    let second = tokio::spawn(std::future::pending::<()>());
    let mut handles = vec![first.abort_handle(), second.abort_handle()];

    cancel_public_broadcaster_tasks(&mut handles);

    assert!(handles.is_empty());
    assert!(
        first
            .await
            .expect_err("first task was aborted")
            .is_cancelled()
    );
    assert!(
        second
            .await
            .expect_err("second task was aborted")
            .is_cancelled()
    );
}
