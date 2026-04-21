use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::handle::AutomatonHandle;
use crate::schedule::Schedule;
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
    #[allow(dead_code)]
    status_tx: watch::Sender<AutomatonStatus>,
    event_tx: mpsc::Sender<AutomatonEvent>,
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
            status_tx: status_tx.clone(),
            event_tx: event_tx.clone(),
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

    #[allow(clippy::needless_pass_by_value)]
    pub fn trigger(&self, id: &str, payload: serde_json::Value) -> Result<(), AutomatonError> {
        if let Some(entry) = self.instances.get(id) {
            let _ = entry.value().event_tx.try_send(AutomatonEvent::LogLine {
                message: format!("trigger: {payload}"),
            });
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
/// `SkillRegistry` and `DefaultToolRegistry`.
impl aura_core::Registry for AutomatonRuntime {
    type Id = String;
    type Item = AutomatonInfo;

    fn register(
        &mut self,
        _id: Self::Id,
        _item: Self::Item,
    ) -> Result<(), aura_core::RegistryError> {
        Err(aura_core::RegistryError::Unsupported(
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
mod tests {
    use super::*;

    struct ImmediateAutomaton;

    #[async_trait::async_trait]
    impl Automaton for ImmediateAutomaton {
        fn kind(&self) -> &str {
            "immediate"
        }

        async fn tick(&self, _ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
            Ok(TickOutcome::Done)
        }
    }

    #[tokio::test]
    async fn test_automaton_runtime_install_and_list() {
        let runtime = AutomatonRuntime::new();

        let (handle, _rx) = runtime
            .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
            .await
            .unwrap();

        let list = runtime.list();
        assert!(
            list.iter()
                .any(|info| info.id.as_str() == handle.id().as_str()),
            "installed automaton should appear in list()"
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "immediate");
    }

    #[tokio::test]
    async fn registry_trait_read_only_view() {
        use aura_core::{Registry, RegistryError};

        let mut runtime = AutomatonRuntime::new();
        assert!(Registry::is_empty(&runtime));

        let (handle, _rx) = runtime
            .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
            .await
            .unwrap();
        let id = handle.id().as_str().to_string();

        assert_eq!(Registry::len(&runtime), 1);
        let info = Registry::get(&runtime, &id).expect("info present");
        assert_eq!(info.kind, "immediate");

        let iter_ids: Vec<_> = Registry::iter(&runtime)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(iter_ids, vec![id.clone()]);

        let err = Registry::register(&mut runtime, id.clone(), info)
            .expect_err("direct register must be unsupported");
        assert!(matches!(err, RegistryError::Unsupported(_)));

        let removed = Registry::remove(&mut runtime, &id);
        assert!(removed.is_none(), "direct remove is unsupported");
    }

    #[tokio::test]
    async fn test_automaton_runtime_start_stop() {
        let runtime = AutomatonRuntime::new();

        let (handle, mut rx) = runtime
            .install(Box::new(ImmediateAutomaton), serde_json::json!({}), None)
            .await
            .unwrap();

        let id = handle.id().as_str().to_string();

        let mut saw_started = false;
        let mut saw_done = false;
        while let Ok(evt) = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
        {
            match evt {
                Some(AutomatonEvent::Started { .. }) => saw_started = true,
                Some(AutomatonEvent::Done) => {
                    saw_done = true;
                    break;
                }
                None => break,
                _ => {}
            }
        }

        assert!(saw_started, "should have received Started event");
        assert!(saw_done, "should have received Done event");

        let stop_result = runtime.stop(&id);
        assert!(
            stop_result.is_err(),
            "stop after completion should return NotFound since the instance is cleaned up"
        );
    }
}
