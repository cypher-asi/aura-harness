//! Executor router for dispatching actions.

use crate::{ExecuteContext, Executor};
use aura_core::{Action, Effect, EffectKind, EffectStatus};
use std::sync::Arc;
use tracing::{debug, error, instrument};

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
        // Find an executor that can handle this action
        for executor in &self.executors {
            if executor.can_handle(action) {
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
        }

        // No executor found
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

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{ActionId, ActionKind, AgentId};
    use bytes::Bytes;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_no_executor() {
        let router = ExecutorRouter::new();
        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());

        let effect = router.execute(&ctx, &action).await;
        assert_eq!(effect.status, EffectStatus::Failed);
    }

    #[tokio::test]
    async fn test_noop_executor() {
        use crate::NoOpExecutor;

        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(NoOpExecutor));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());

        let effect = router.execute(&ctx, &action).await;
        assert_eq!(effect.status, EffectStatus::Committed);
    }

    // ========================================================================
    // Additional Router Tests
    // ========================================================================

    /// A test executor that only handles specific action kinds.
    struct SelectiveExecutor {
        handled_kind: ActionKind,
        should_fail: bool,
    }

    impl SelectiveExecutor {
        fn new(kind: ActionKind) -> Self {
            Self {
                handled_kind: kind,
                should_fail: false,
            }
        }

        fn failing(kind: ActionKind) -> Self {
            Self {
                handled_kind: kind,
                should_fail: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl Executor for SelectiveExecutor {
        async fn execute(&self, _ctx: &ExecuteContext, action: &Action) -> anyhow::Result<Effect> {
            if self.should_fail {
                anyhow::bail!("Executor configured to fail");
            }
            Ok(Effect::committed_agreement(
                action.action_id,
                Bytes::from(format!("Handled by {:?} executor", self.handled_kind)),
            ))
        }

        fn can_handle(&self, action: &Action) -> bool {
            action.kind == self.handled_kind
        }

        fn name(&self) -> &'static str {
            "selective"
        }
    }

    #[tokio::test]
    async fn test_router_dispatches_to_correct_executor() {
        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(SelectiveExecutor::new(ActionKind::Delegate)));
        router.add_executor(Arc::new(SelectiveExecutor::new(ActionKind::Reason)));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );

        // Delegate action should be handled
        let delegate_action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());
        let effect = router.execute(&ctx, &delegate_action).await;
        assert_eq!(effect.status, EffectStatus::Committed);
        let payload = String::from_utf8_lossy(&effect.payload);
        assert!(payload.contains("Delegate"));

        // Reason action should be handled by different executor
        let reason_action = Action::new(ActionId::generate(), ActionKind::Reason, Bytes::new());
        let effect = router.execute(&ctx, &reason_action).await;
        assert_eq!(effect.status, EffectStatus::Committed);
        let payload = String::from_utf8_lossy(&effect.payload);
        assert!(payload.contains("Reason"));
    }

    #[tokio::test]
    async fn test_router_no_matching_executor() {
        let mut router = ExecutorRouter::new();
        // Only add executor for Delegate actions
        router.add_executor(Arc::new(SelectiveExecutor::new(ActionKind::Delegate)));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );

        // Memorize action has no handler
        let memorize_action = Action::new(ActionId::generate(), ActionKind::Memorize, Bytes::new());
        let effect = router.execute(&ctx, &memorize_action).await;
        assert_eq!(effect.status, EffectStatus::Failed);
    }

    #[tokio::test]
    async fn test_router_executor_failure_propagates() {
        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(SelectiveExecutor::failing(ActionKind::Delegate)));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );

        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());
        let effect = router.execute(&ctx, &action).await;

        assert_eq!(effect.status, EffectStatus::Failed);
        let payload = String::from_utf8_lossy(&effect.payload);
        assert!(payload.contains("Executor error"));
    }

    #[tokio::test]
    async fn test_router_first_matching_executor_wins() {
        let mut router = ExecutorRouter::new();

        // Add two executors that both handle Delegate
        // First one should win
        struct FirstExecutor;
        struct SecondExecutor;

        #[async_trait::async_trait]
        impl Executor for FirstExecutor {
            async fn execute(&self, _ctx: &ExecuteContext, action: &Action) -> anyhow::Result<Effect> {
                Ok(Effect::committed_agreement(action.action_id, "first"))
            }
            fn can_handle(&self, action: &Action) -> bool {
                action.kind == ActionKind::Delegate
            }
            fn name(&self) -> &'static str {
                "first"
            }
        }

        #[async_trait::async_trait]
        impl Executor for SecondExecutor {
            async fn execute(&self, _ctx: &ExecuteContext, action: &Action) -> anyhow::Result<Effect> {
                Ok(Effect::committed_agreement(action.action_id, "second"))
            }
            fn can_handle(&self, action: &Action) -> bool {
                action.kind == ActionKind::Delegate
            }
            fn name(&self) -> &'static str {
                "second"
            }
        }

        router.add_executor(Arc::new(FirstExecutor));
        router.add_executor(Arc::new(SecondExecutor));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());

        let effect = router.execute(&ctx, &action).await;
        let payload = String::from_utf8_lossy(&effect.payload);
        assert_eq!(payload, "first");
    }

    #[tokio::test]
    async fn test_router_with_executors_constructor() {
        use crate::NoOpExecutor;

        let router = ExecutorRouter::with_executors(vec![
            Arc::new(NoOpExecutor) as Arc<dyn Executor>,
        ]);

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());

        let effect = router.execute(&ctx, &action).await;
        assert_eq!(effect.status, EffectStatus::Committed);
    }

    #[tokio::test]
    async fn test_router_default() {
        let router = ExecutorRouter::default();

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let action = Action::new(ActionId::generate(), ActionKind::Delegate, Bytes::new());

        // Default router has no executors
        let effect = router.execute(&ctx, &action).await;
        assert_eq!(effect.status, EffectStatus::Failed);
    }

    #[tokio::test]
    async fn test_router_preserves_action_id_in_effect() {
        use crate::NoOpExecutor;

        let mut router = ExecutorRouter::new();
        router.add_executor(Arc::new(NoOpExecutor));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        
        let action_id = ActionId::generate();
        let action = Action::new(action_id, ActionKind::Delegate, Bytes::new());

        let effect = router.execute(&ctx, &action).await;
        assert_eq!(effect.action_id, action_id);
    }
}
