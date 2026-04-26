use tokio::sync::mpsc;

use crate::events::AutomatonEvent;
use crate::state::AutomatonState;
use crate::types::AutomatonId;
use crate::AutomatonError;

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

    pub fn emit(&self, event: AutomatonEvent) -> Result<(), AutomatonError> {
        self.event_tx
            .try_send(event)
            .map_err(|e| AutomatonError::EventDelivery(e.to_string()))
    }

    pub fn is_cancelled(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    pub fn cancellation_token(&self) -> &tokio_util::sync::CancellationToken {
        &self.shutdown
    }
}

#[cfg(test)]
mod tests {
    use super::TickContext;
    use crate::events::AutomatonEvent;
    use crate::state::AutomatonState;
    use crate::types::AutomatonId;
    use crate::AutomatonError;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn test_context(channel_size: usize) -> (TickContext, mpsc::Receiver<AutomatonEvent>) {
        let (tx, rx) = mpsc::channel(channel_size);
        let ctx = TickContext::new(
            AutomatonId::from_string("test-automaton"),
            AutomatonState::new(),
            tx,
            json!({}),
            None,
            CancellationToken::new(),
        );
        (ctx, rx)
    }

    #[test]
    fn emit_delivers_event_when_capacity_exists() {
        let (ctx, mut rx) = test_context(1);

        ctx.emit(AutomatonEvent::LogLine {
            message: "hello".to_string(),
        })
        .expect("emit event");

        assert!(matches!(
            rx.try_recv(),
            Ok(AutomatonEvent::LogLine { message }) if message == "hello"
        ));
    }

    #[test]
    fn emit_returns_structured_error_when_channel_is_full() {
        let (ctx, mut rx) = test_context(1);
        ctx.emit(AutomatonEvent::LogLine {
            message: "first".to_string(),
        })
        .expect("fill channel");

        let result = ctx.emit(AutomatonEvent::LogLine {
            message: "second".to_string(),
        });

        assert!(matches!(result, Err(AutomatonError::EventDelivery(_))));
        assert!(matches!(
            rx.try_recv(),
            Ok(AutomatonEvent::LogLine { message }) if message == "first"
        ));
    }
}
