use tokio::sync::mpsc;

use crate::events::AutomatonEvent;
use crate::state::AutomatonState;
use crate::types::AutomatonId;

pub struct TickContext {
    pub automaton_id: AutomatonId,
    pub state: AutomatonState,
    pub event_tx: mpsc::Sender<AutomatonEvent>,
    pub config: serde_json::Value,
    pub workspace_root: Option<std::path::PathBuf>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl TickContext {
    pub fn new(
        automaton_id: AutomatonId,
        state: AutomatonState,
        event_tx: mpsc::Sender<AutomatonEvent>,
        config: serde_json::Value,
        workspace_root: Option<std::path::PathBuf>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            automaton_id,
            state,
            event_tx,
            config,
            workspace_root,
            shutdown,
        }
    }

    pub fn emit(&self, event: AutomatonEvent) {
        if let Err(e) = self.event_tx.try_send(event) {
            tracing::warn!("automaton event channel full or closed: {e}");
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    pub fn cancellation_token(&self) -> &tokio_util::sync::CancellationToken {
        &self.shutdown
    }
}
