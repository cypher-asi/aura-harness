use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::handle::AutomatonHandle;
use crate::metadata::Schedule;
use crate::state::AutomatonState;
use crate::types::{AutomatonId, AutomatonInfo, AutomatonStatus};

/// A long-running background task registered with [`AutomatonRuntime`].
///
/// Implementations define lifecycle hooks and a recurring `tick` that drives execution.
#[async_trait::async_trait]
pub trait Automaton: Send + Sync + 'static {
    /// Returns the automaton's type identifier (e.g. `"dev-loop"`, `"chat"`).
    fn kind(&self) -> &str;

    /// Returns the default schedule for this automaton. Defaults to [`Schedule::OnDemand`].
    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    /// Called once after installation, before the first tick. Use for setup work.
    async fn on_install(&self, _ctx: &TickContext) -> Result<(), AutomatonError> {
        Ok(())
    }

    /// Called repeatedly while the automaton is running. Return [`TickOutcome`] to
    /// signal whether to continue, finish, or yield.
    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError>;

    /// Called after the run loop exits (whether by completion, cancellation, or error).
    async fn on_stop(&self, _ctx: &TickContext) -> Result<(), AutomatonError> {
        Ok(())
    }
}

/// Result of a single [`Automaton::tick`] invocation.
#[derive(Debug, Clone)]
pub enum TickOutcome {
    /// The automaton should be ticked again immediately.
    Continue,
    /// The automaton has finished its work and should be cleaned up.
    Done,
    /// The automaton is yielding control (e.g. waiting for external input).
    Yield { reason: String },
}

struct RunningAutomaton {
    info: AutomatonInfo,
    cancel: CancellationToken,
}

/// Manages the lifecycle of [`Automaton`] instances (install, run, stop, list).
pub struct AutomatonRuntime {
    instances: Arc<DashMap<String, RunningAutomaton>>,
}

impl AutomatonRuntime {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(DashMap::new()),
        }
    }

    /// Installs and starts an automaton, returning a handle and an event receiver.
    #[allow(clippy::unused_async)]
    pub async fn install(
        &self,
        automaton: Box<dyn Automaton>,
        config: serde_json::Value,
        workspace_root: Option<std::path::PathBuf>,
    ) -> Result<(AutomatonHandle, mpsc::Receiver<AutomatonEvent>), AutomatonError> {
        let id = AutomatonId::new();
        let schedule = automaton.default_schedule();
        let cancel = CancellationToken::new();
        let (status_tx, status_rx) = watch::channel(AutomatonStatus::Installing);
        let (pause_tx, pause_rx) = watch::channel(false);
        let (event_tx, event_rx) = mpsc::channel(1024);

        let info = AutomatonInfo {
            id: id.clone(),
            kind: automaton.kind().to_string(),
            status: AutomatonStatus::Installing,
            schedule,
            config: config.clone(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let running = RunningAutomaton {
            info,
            cancel: cancel.clone(),
        };

        self.instances.insert(id.as_str().to_string(), running);

        let instances = self.instances.clone();
        let id_clone = id.clone();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            Self::run_automaton(
                id_clone,
                automaton,
                config,
                workspace_root,
                cancel_clone,
                status_tx,
                pause_rx,
                event_tx,
                instances,
            )
            .await;
        });

        let handle = AutomatonHandle::new(id, cancel, status_rx, pause_tx);
        Ok((handle, event_rx))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_automaton(
        id: AutomatonId,
        automaton: Box<dyn Automaton>,
        config: serde_json::Value,
        workspace_root: Option<std::path::PathBuf>,
        cancel: CancellationToken,
        status_tx: watch::Sender<AutomatonStatus>,
        mut pause_rx: watch::Receiver<bool>,
        event_tx: mpsc::Sender<AutomatonEvent>,
        instances: Arc<DashMap<String, RunningAutomaton>>,
    ) {
        let state = AutomatonState::new();
        let mut ctx = TickContext::new(
            id.clone(),
            state,
            event_tx.clone(),
            config,
            workspace_root,
            cancel.clone(),
        );

        if let Err(e) = automaton.on_install(&ctx).await {
            error!(automaton_id = %id, error = %e, "on_install failed");
            let _ = status_tx.send(AutomatonStatus::Failed);
            let _ = event_tx.try_send(AutomatonEvent::Error {
                automaton_id: id.to_string(),
                message: e.to_string(),
            });
            let _ = event_tx.try_send(AutomatonEvent::Done);
            instances.remove(id.as_str());
            return;
        }

        let _ = status_tx.send(AutomatonStatus::Running);
        let _ = event_tx.try_send(AutomatonEvent::Started {
            automaton_id: id.to_string(),
        });

        let final_status = loop {
            if cancel.is_cancelled() {
                // Distinguishes a clean user-driven stop from the
                // generic `Stopped` final-status fan-out below. The
                // operator-facing log previously only carried a
                // `Stopped` reason via `AutomatonEvent::Stopped`,
                // which read identically to a natural completion or
                // crash-driven cleanup; emitting an explicit INFO
                // here makes user-initiated cancellation greppable
                // in production logs.
                info!(
                    automaton_id = %id,
                    "Automaton run loop observed cancellation; stopping"
                );
                break AutomatonStatus::Stopped;
            }

            if *pause_rx.borrow() {
                let _ = status_tx.send(AutomatonStatus::Paused);
                let _ = event_tx.try_send(AutomatonEvent::Paused {
                    automaton_id: id.to_string(),
                });
                loop {
                    if cancel.is_cancelled() {
                        break;
                    }
                    if pause_rx.changed().await.is_err() {
                        break;
                    }
                    if !*pause_rx.borrow() {
                        let _ = status_tx.send(AutomatonStatus::Running);
                        let _ = event_tx.try_send(AutomatonEvent::Resumed {
                            automaton_id: id.to_string(),
                        });
                        break;
                    }
                }
                if cancel.is_cancelled() {
                    info!(
                        automaton_id = %id,
                        "Automaton stopped while paused"
                    );
                    break AutomatonStatus::Stopped;
                }
            }

            match automaton.tick(&mut ctx).await {
                Ok(TickOutcome::Continue) => {}
                Ok(TickOutcome::Done) => break AutomatonStatus::Completed,
                Ok(TickOutcome::Yield { reason }) => {
                    info!(automaton_id = %id, %reason, "automaton yielded");
                    break AutomatonStatus::Completed;
                }
                Err(e) => {
                    error!(automaton_id = %id, error = %e, "tick failed");
                    let _ = event_tx.try_send(AutomatonEvent::Error {
                        automaton_id: id.to_string(),
                        message: e.to_string(),
                    });
                    break AutomatonStatus::Failed;
                }
            }
        };

        if let Err(e) = automaton.on_stop(&ctx).await {
            warn!(automaton_id = %id, error = %e, "on_stop error (non-fatal)");
        }

        let _ = status_tx.send(final_status);
        let _ = event_tx.try_send(AutomatonEvent::Stopped {
            automaton_id: id.to_string(),
            reason: format!("{final_status:?}"),
        });
        let _ = event_tx.try_send(AutomatonEvent::Done);
        instances.remove(id.as_str());
    }

    /// Returns metadata for all currently registered automaton instances.
    pub fn list(&self) -> Vec<AutomatonInfo> {
        self.instances
            .iter()
            .map(|entry| entry.value().info.clone())
            .collect()
    }

    pub fn get_info(&self, id: &str) -> Option<AutomatonInfo> {
        self.instances
            .get(id)
            .map(|entry| entry.value().info.clone())
    }

    /// Cancels the automaton with the given ID. Returns an error if not found.
    pub fn stop(&self, id: &str) -> Result<(), AutomatonError> {
        if let Some(entry) = self.instances.get(id) {
            entry.value().cancel.cancel();
            Ok(())
        } else {
            Err(AutomatonError::NotFound(id.to_string()))
        }
    }
}

impl Default for AutomatonRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// `Registry` trait impl (Wave 4 unification). The automaton runtime has
/// a genuinely lifecycle-managed surface (async `install`, cooperative
/// `stop`, worker-task–initiated cleanup), so `register` / `remove` are
/// intentionally *not* wired to `install` / `stop` here — those have
/// different semantics and invoking them through a generic trait would
/// hide the async/cancellation contract. The trait impl instead exposes
/// the read-only name -> metadata view (`get` / `iter` / `len`) shared by
/// `SkillRegistry` and `ToolCatalog`.
impl aura_core_types::Registry for AutomatonRuntime {
    type Id = String;
    type Item = AutomatonInfo;

    fn register(
        &mut self,
        _id: Self::Id,
        _item: Self::Item,
    ) -> Result<(), aura_core_types::RegistryError> {
        Err(aura_core_types::RegistryError::Unsupported(
            "AutomatonRuntime uses async install() to spawn running automatons",
        ))
    }

    fn get(&self, id: &Self::Id) -> Option<Self::Item> {
        self.get_info(id)
    }

    fn iter(&self) -> Vec<(Self::Id, Self::Item)> {
        self.list()
            .into_iter()
            .map(|info| (info.id.as_str().to_string(), info))
            .collect()
    }

    fn remove(&mut self, _id: &Self::Id) -> Option<Self::Item> {
        // AutomatonRuntime cleanup is worker-task-driven; direct removal
        // through the Registry surface is unsupported. Callers should
        // invoke `stop(id)` and wait for the `Stopped` event.
        None
    }

    fn len(&self) -> usize {
        self.instances.len()
    }

    fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

#[cfg(test)]
mod tests;
