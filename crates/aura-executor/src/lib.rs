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

use std::collections::HashMap;

use async_trait::async_trait;
use aura_core::{Action, Effect, EffectStatus, ToolResult};

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

/// Result of decoding a tool execution effect into displayable text.
#[derive(Debug, Clone)]
pub struct DecodedToolResult {
    /// Text content (stdout on success, stderr on failure).
    pub content: String,
    /// Whether the effect represents an error.
    pub is_error: bool,
    /// Additional metadata from the tool result, if available.
    pub metadata: HashMap<String, String>,
}

/// Decode a tool execution [`Effect`] into text content, error status, and metadata.
///
/// Shared between `KernelToolExecutor` (agent loop) and `TurnProcessor` (runtime).
#[must_use]
pub fn decode_tool_effect(effect: &Effect) -> DecodedToolResult {
    if effect.status == EffectStatus::Committed {
        match serde_json::from_slice::<ToolResult>(&effect.payload) {
            Ok(tool_result) => {
                let content = if tool_result.stdout.is_empty() {
                    "Success (no output)".to_string()
                } else {
                    String::from_utf8_lossy(&tool_result.stdout).to_string()
                };
                DecodedToolResult {
                    content,
                    is_error: !tool_result.ok,
                    metadata: tool_result.metadata,
                }
            }
            Err(_) => DecodedToolResult {
                content: "Tool executed successfully".to_string(),
                is_error: false,
                metadata: HashMap::new(),
            },
        }
    } else {
        let content = if let Ok(tool_result) =
            serde_json::from_slice::<ToolResult>(&effect.payload)
        {
            String::from_utf8_lossy(&tool_result.stderr).to_string()
        } else {
            let raw = String::from_utf8_lossy(&effect.payload);
            if raw.is_empty() {
                "Tool execution failed".to_string()
            } else {
                raw.to_string()
            }
        };
        DecodedToolResult {
            content,
            is_error: true,
            metadata: HashMap::new(),
        }
    }
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
