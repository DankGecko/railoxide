use std::sync::Arc;

use gpui::{Context, WeakEntity};
use tokio::runtime::Handle;
use wallet_ops::vault::DesktopVaultStore;

use super::WalletRoot;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::root) enum WalletMaintenanceReset {
    #[default]
    Idle,
    Public,
    Merkle,
}

#[derive(Debug, Default)]
pub(in crate::root) struct WalletMaintenanceStateMachine {
    reset: WalletMaintenanceReset,
    generation: u64,
    status: Option<Arc<str>>,
}

impl WalletMaintenanceStateMachine {
    pub(in crate::root) const fn reset(&self) -> WalletMaintenanceReset {
        self.reset
    }

    pub(in crate::root) fn status(&self) -> Option<Arc<str>> {
        self.status.clone()
    }

    pub(in crate::root) fn try_acquire(
        &mut self,
        reset: WalletMaintenanceReset,
        status: impl Into<Arc<str>>,
    ) -> Option<u64> {
        if self.reset != WalletMaintenanceReset::Idle || reset == WalletMaintenanceReset::Idle {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.reset = reset;
        self.status = Some(status.into());
        Some(self.generation)
    }

    pub(in crate::root) fn complete(
        &mut self,
        generation: u64,
        status: impl Into<Arc<str>>,
    ) -> bool {
        if self.reset == WalletMaintenanceReset::Idle || self.generation != generation {
            return false;
        }
        self.reset = WalletMaintenanceReset::Idle;
        self.status = Some(status.into());
        true
    }

    fn clear_status(&mut self) -> bool {
        if self.reset != WalletMaintenanceReset::Idle {
            return false;
        }
        self.status.take().is_some()
    }

    pub(in crate::root) fn set_idle_status(&mut self, status: impl Into<Arc<str>>) -> bool {
        if self.reset != WalletMaintenanceReset::Idle {
            return false;
        }
        self.status = Some(status.into());
        true
    }
}

pub(in crate::root) struct WalletMaintenanceController {
    runtime: Handle,
    state: WalletMaintenanceStateMachine,
    active_root: Option<WeakEntity<WalletRoot>>,
}

impl WalletMaintenanceController {
    pub(in crate::root) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            state: WalletMaintenanceStateMachine::default(),
            active_root: None,
        }
    }

    pub(in crate::root) const fn reset(&self) -> WalletMaintenanceReset {
        self.state.reset()
    }

    pub(in crate::root) fn is_idle(&self) -> bool {
        self.reset() == WalletMaintenanceReset::Idle
    }

    pub(in crate::root) fn status(&self) -> Option<Arc<str>> {
        self.state.status()
    }

    pub(in crate::root) fn set_active_root(&mut self, active_root: WeakEntity<WalletRoot>) {
        self.active_root = Some(active_root);
    }

    pub(in crate::root) fn clear_active_root(&mut self) {
        self.active_root = None;
    }

    pub(in crate::root) fn clear_status(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.clear_status() {
            cx.notify();
        }
    }

    pub(in crate::root) fn start_public_reset(
        &mut self,
        vault_store: &DesktopVaultStore,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.active_root.as_ref().is_some_and(|root| {
            !root
                .update(cx, |root, _cx| root.destructive_cache_reset_is_allowed())
                .unwrap_or(false)
        }) {
            self.state
                .set_idle_status("Wait for wallet sync cleanup before resetting caches");
            cx.notify();
            return false;
        }
        let Some(generation) = self.state.try_acquire(
            WalletMaintenanceReset::Public,
            "Resetting public sync caches...",
        ) else {
            return false;
        };
        let reset_context = self.active_root.as_ref().and_then(|root| {
            root.update(cx, WalletRoot::begin_public_sync_cache_reset)
                .ok()
        });
        let db = vault_store.db();
        let resync_requested = reset_context.is_some();
        let join = self.runtime.spawn(async move {
            let store = match reset_context {
                Some(reset_context) => reset_context.shutdown_for_public_reset().await?,
                None => None,
            };
            if let Some(store) = store {
                let report = store.reset_public_sync_caches().await;
                if let Err(error) = report.persisted.as_ref() {
                    return Err(format!("persisted public cache reset failed: {error}"));
                }
                let failed = report.failed_chain_count();
                if failed > 0 {
                    let first_failure = report
                        .chains
                        .iter()
                        .find_map(|reset| {
                            reset.result.as_ref().err().map(|error| {
                                format!(
                                    "chain {} contract {}: {error}",
                                    reset.chain.chain_id, reset.chain.contract
                                )
                            })
                        })
                        .expect("failed reset report contains an error");
                    return Err(format!(
                        "{failed} of {} chain resets failed; first failure: {first_failure}",
                        report.chains.len()
                    ));
                }
                return Ok::<u64, String>(report.total_removed_entries);
            }
            wallet_ops::reset_persisted_public_sync_caches(db.as_ref())
                .await
                .map(wallet_ops::PersistedPublicSyncCacheResetReport::total_removed_entries)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let message = match join.await {
                Ok(Ok(removed)) if resync_requested => format!(
                    "Public sync caches reset; cleared {removed} cache records and requested resync"
                ),
                Ok(Ok(removed)) => {
                    format!("Persisted public sync caches reset; cleared {removed} cache records")
                }
                Ok(Err(error)) => format!("Failed to reset public sync caches: {error}"),
                Err(error) => format!("Public sync cache reset task failed: {error}"),
            };
            let _ = this.update(cx, |controller, cx| {
                if controller.state.complete(generation, message) {
                    if let Some(root) = controller.active_root.as_ref() {
                        let _ = root.update(cx, WalletRoot::finish_public_sync_cache_reset);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
        true
    }

    pub(in crate::root) fn start_merkle_reset(
        &mut self,
        vault_store: &DesktopVaultStore,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.active_root.as_ref().is_some_and(|root| {
            !root
                .update(cx, |root, _cx| root.destructive_cache_reset_is_allowed())
                .unwrap_or(false)
        }) {
            self.state
                .set_idle_status("Wait for wallet sync cleanup before resetting caches");
            cx.notify();
            return false;
        }
        let Some(generation) = self.state.try_acquire(
            WalletMaintenanceReset::Merkle,
            "Resetting local Merkle forest cache...",
        ) else {
            return false;
        };
        let active_root = self.active_root.clone();
        let cleanup = active_root.as_ref().and_then(|root| {
            root.update(cx, WalletRoot::begin_merkle_forest_cache_reset)
                .ok()
        });
        let resync_requested = cleanup.is_some();
        let db = vault_store.db();
        let join = self.runtime.spawn(async move {
            if let Some(cleanup) = cleanup {
                cleanup.shutdown_for_merkle_reset().await?;
            }
            tokio::task::spawn_blocking(move || {
                wallet_ops::reset_local_merkle_forest_cache(db.as_ref())
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        });
        cx.spawn(async move |this, cx| {
            let (message, reset_succeeded) = match join.await {
                Ok(Ok(removed)) if resync_requested => (
                    format!(
                        "Local Merkle forest cache reset; cleared {removed} snapshot files and restarted private sync"
                    ),
                    true,
                ),
                Ok(Ok(removed)) => (
                    format!(
                        "Local Merkle forest cache reset; cleared {removed} snapshot files"
                    ),
                    true,
                ),
                Ok(Err(error)) => (
                    format!("Failed to reset local Merkle forest cache: {error}"),
                    false,
                ),
                Err(error) => (
                    format!("Local Merkle forest cache reset task failed: {error}"),
                    false,
                ),
            };
            let _ = this.update(cx, |controller, cx| {
                if !controller.state.complete(generation, message) {
                    return;
                }
                if let Some(root) = controller.active_root.as_ref() {
                    let _ = root.update(cx, |root, cx| {
                        root.finish_merkle_forest_cache_reset(reset_succeeded, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }
}
