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

#[async_trait::async_trait]
pub trait Automaton: Send + Sync + 'static {
    fn kind(&self) -> &str;

    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    async fn on_install(&self, _ctx: &TickContext) -> Result<(), AutomatonError> {
        Ok(())
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError>;

    async fn on_stop(&self, _ctx: &TickContext) -> Result<(), AutomatonError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum TickOutcome {
    Continue,
    Done,
    Yield { reason: String },
}

struct RunningAutomaton {
    info: AutomatonInfo,
    cancel: CancellationToken,
    #[allow(dead_code)]
    status_tx: watch::Sender<AutomatonStatus>,
    event_tx: mpsc::UnboundedSender<AutomatonEvent>,
}

pub struct AutomatonRuntime {
    instances: Arc<DashMap<String, RunningAutomaton>>,
}

impl AutomatonRuntime {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(DashMap::new()),
        }
    }

    pub async fn install(
        &self,
        automaton: Box<dyn Automaton>,
        config: serde_json::Value,
        workspace_root: Option<std::path::PathBuf>,
    ) -> Result<(AutomatonHandle, mpsc::UnboundedReceiver<AutomatonEvent>), AutomatonError> {
        let id = AutomatonId::new();
        let schedule = automaton.default_schedule();
        let cancel = CancellationToken::new();
        let (status_tx, status_rx) = watch::channel(AutomatonStatus::Installing);
        let (pause_tx, pause_rx) = watch::channel(false);
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let info = AutomatonInfo {
            id: id.clone(),
            kind: automaton.kind().to_string(),
            status: AutomatonStatus::Installing,
            schedule: schedule.clone(),
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
        event_tx: mpsc::UnboundedSender<AutomatonEvent>,
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
            let _ = event_tx.send(AutomatonEvent::Error {
                automaton_id: id.to_string(),
                message: e.to_string(),
            });
            let _ = event_tx.send(AutomatonEvent::Done);
            instances.remove(id.as_str());
            return;
        }

        let _ = status_tx.send(AutomatonStatus::Running);
        let _ = event_tx.send(AutomatonEvent::Started {
            automaton_id: id.to_string(),
        });

        let final_status = loop {
            if cancel.is_cancelled() {
                break AutomatonStatus::Stopped;
            }

            if *pause_rx.borrow() {
                let _ = status_tx.send(AutomatonStatus::Paused);
                let _ = event_tx.send(AutomatonEvent::Paused {
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
                        let _ = event_tx.send(AutomatonEvent::Resumed {
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
                Ok(TickOutcome::Continue) => continue,
                Ok(TickOutcome::Done) => break AutomatonStatus::Completed,
                Ok(TickOutcome::Yield { reason }) => {
                    info!(automaton_id = %id, %reason, "automaton yielded");
                    break AutomatonStatus::Completed;
                }
                Err(e) => {
                    error!(automaton_id = %id, error = %e, "tick failed");
                    let _ = event_tx.send(AutomatonEvent::Error {
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
        let _ = event_tx.send(AutomatonEvent::Stopped {
            automaton_id: id.to_string(),
            reason: format!("{final_status:?}"),
        });
        let _ = event_tx.send(AutomatonEvent::Done);
        instances.remove(id.as_str());
    }

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

    pub fn stop(&self, id: &str) -> Result<(), AutomatonError> {
        if let Some(entry) = self.instances.get(id) {
            entry.value().cancel.cancel();
            Ok(())
        } else {
            Err(AutomatonError::NotFound(id.to_string()))
        }
    }

    pub fn trigger(&self, id: &str, payload: serde_json::Value) -> Result<(), AutomatonError> {
        if let Some(entry) = self.instances.get(id) {
            let _ = entry.value().event_tx.send(AutomatonEvent::LogLine {
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
