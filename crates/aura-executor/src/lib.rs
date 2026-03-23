//! # aura-executor
//!
//! Executor trait and router for dispatching actions to executors.
//!
//! The executor framework provides the boundary between deterministic
//! kernel logic and external side effects.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod context;
mod router;

pub use context::ExecuteContext;
pub use router::ExecutorRouter;

use async_trait::async_trait;
use aura_core::{Action, Effect};

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("executor not found: {0}")]
    NotFound(String),
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Executor trait for handling actions.
///
/// Executors are responsible for converting authorized Actions into Effects.
/// They may perform side effects (tools, network calls, etc.) and must
/// return appropriate Effect statuses.
#[async_trait]
pub trait Executor: Send + Sync {
    /// Execute an action and produce an effect.
    ///
    /// # Errors
    /// Returns error if execution fails. The caller should convert this
    /// to a Failed effect and record it.
    async fn execute(&self, ctx: &ExecuteContext, action: &Action) -> Result<Effect, ExecutorError>;

    /// Check if this executor can handle the given action.
    fn can_handle(&self, action: &Action) -> bool;

    /// Get the executor name for logging/debugging.
    fn name(&self) -> &'static str;
}

/// A no-op executor that accepts all actions and returns empty committed effects.
#[cfg(test)]
pub(crate) struct NoOpExecutor;

#[cfg(test)]
#[async_trait]
impl Executor for NoOpExecutor {
    async fn execute(&self, _ctx: &ExecuteContext, action: &Action) -> Result<Effect, ExecutorError> {
        Ok(Effect::committed_agreement(
            action.action_id,
            bytes::Bytes::new(),
        ))
    }

    fn can_handle(&self, _action: &Action) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}
