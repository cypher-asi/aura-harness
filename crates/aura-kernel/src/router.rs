//! Executor router for dispatching actions.

use aura_core::{Action, Effect, EffectKind, EffectStatus, ExecuteContext, Executor};
use std::sync::Arc;
use tracing::{debug, error, instrument, warn};

/// Router that dispatches actions to the appropriate executor.
pub struct ExecutorRouter {
    executors: Vec<Arc<dyn Executor>>,
}

impl ExecutorRouter {
    /// Create a new empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            executors: Vec::new(),
        }
    }

    /// Add an executor to the router.
    pub fn add_executor(&mut self, executor: Arc<dyn Executor>) {
        self.executors.push(executor);
    }

    /// Create a router with the given executors.
    #[must_use]
    pub fn with_executors(executors: Vec<Arc<dyn Executor>>) -> Self {
        Self { executors }
    }

    /// Execute an action by finding and invoking the appropriate executor.
    #[instrument(skip(self, ctx, action), fields(action_id = %action.action_id, kind = ?action.kind))]
    pub async fn execute(&self, ctx: &ExecuteContext, action: &Action) -> Effect {
        let mut matched_count = 0usize;
        let mut selected: Option<&Arc<dyn Executor>> = None;
        for executor in &self.executors {
            if executor.can_handle(action) {
                matched_count += 1;
                if selected.is_none() {
                    selected = Some(executor);
                }
            }
        }

        if matched_count > 1 {
            warn!(
                matched_count,
                "Multiple executors can handle action; dispatching first registered match"
            );
        }

        if let Some(executor) = selected {
            debug!(executor = executor.name(), "Dispatching action to executor");
            match executor.execute(ctx, action).await {
                Ok(effect) => {
                    debug!(?effect.status, "Action executed successfully");
                    return effect;
                }
                Err(e) => {
                    error!(error = %e, "Executor failed");
                    return Effect::new(
                        action.action_id,
                        EffectKind::Agreement,
                        EffectStatus::Failed,
                        format!("Executor error: {e}"),
                    );
                }
            }
        }

        debug!("No executor found for action");
        Effect::new(
            action.action_id,
            EffectKind::Agreement,
            EffectStatus::Failed,
            "No executor available for action",
        )
    }
}

impl Default for ExecutorRouter {
    fn default() -> Self {
        Self::new()
    }
}
