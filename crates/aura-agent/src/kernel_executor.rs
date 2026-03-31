//! Default `AgentToolExecutor` implementation wrapping the kernel's `ExecutorRouter`.
//!
//! Bridges between the `AgentToolExecutor` trait (agent-loop layer) and the
//! executor infrastructure now owned by `aura-core` / `aura-kernel`.

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use async_trait::async_trait;
use aura_core::{Action, AgentId, ToolCall};
use aura_kernel::{decode_tool_effect, ExecuteContext};
use aura_kernel::{ExecutorRouter, PermissionLevel, Policy};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Controls whether tools in a batch run sequentially or concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    /// Execute tool calls one at a time in order (default).
    #[default]
    Sequential,
    /// Execute all tool calls concurrently via `join_all`.
    Parallel,
}

const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// Bridges the `AgentToolExecutor` trait to the kernel's `ExecutorRouter`.
///
/// Translates `ToolCallInfo` into `Action`s, dispatches through the router,
/// and converts `Effect`s back into `ToolCallResult`s. Supports sequential
/// and parallel execution modes, per-tool timeouts, and optional policy
/// enforcement.
pub struct KernelToolExecutor {
    executor: ExecutorRouter,
    agent_id: AgentId,
    workspace: PathBuf,
    execution_mode: ToolExecutionMode,
    tool_timeout: Duration,
    policy: Option<Policy>,
}

impl KernelToolExecutor {
    /// Create a new executor bridge with sequential mode, 120 s timeout, and no policy.
    #[must_use]
    pub fn new(executor: ExecutorRouter, agent_id: AgentId, workspace: PathBuf) -> Self {
        Self {
            executor,
            agent_id,
            workspace,
            execution_mode: ToolExecutionMode::default(),
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            policy: None,
        }
    }

    /// Switch to parallel execution mode.
    #[must_use]
    pub const fn with_parallel(mut self) -> Self {
        self.execution_mode = ToolExecutionMode::Parallel;
        self
    }

    /// Override the per-tool timeout.
    #[must_use]
    pub const fn with_timeout(mut self, dur: Duration) -> Self {
        self.tool_timeout = dur;
        self
    }

    /// Attach a [`Policy`] for pre-dispatch permission checks.
    #[must_use]
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Execute a single tool call with timeout, returning the result.
    async fn execute_one(&self, tool: &ToolCallInfo) -> ToolCallResult {
        if let Some(ref policy) = self.policy {
            if policy.check_tool_permission(&tool.name) == PermissionLevel::Deny {
                warn!(
                    tool_use_id = %tool.id,
                    tool_name = %tool.name,
                    "Tool denied by policy"
                );
                return ToolCallResult::error(
                    &tool.id,
                    format!("Tool '{}' is not allowed by policy", tool.name),
                );
            }
        }

        debug!(
            tool_use_id = %tool.id,
            tool_name = %tool.name,
            workspace = %self.workspace.display(),
            "Executing tool via KernelToolExecutor"
        );

        let tool_call = ToolCall::new(tool.name.clone(), tool.input.clone());
        let action = match Action::delegate_tool(&tool_call) {
            Ok(a) => a,
            Err(e) => {
                error!(
                    tool_use_id = %tool.id,
                    tool_name = %tool.name,
                    error = %e,
                    "Failed to serialize tool call to Action"
                );
                return ToolCallResult::error(
                    &tool.id,
                    format!("Internal serialization error: {e}"),
                );
            }
        };

        let ctx = ExecuteContext::new(self.agent_id, action.action_id, self.workspace.clone());
        let timeout_dur = self.tool_timeout;

        if let Ok(effect) =
            tokio::time::timeout(timeout_dur, self.executor.execute(&ctx, &action)).await
        {
            let decoded = decode_tool_effect(&effect);
            info!(
                tool_use_id = %tool.id,
                tool_name = %tool.name,
                is_error = decoded.is_error,
                effect_status = ?effect.status,
                result_len = decoded.content.len(),
                workspace = %self.workspace.display(),
                "Tool execution completed"
            );
            ToolCallResult {
                tool_use_id: tool.id.clone(),
                content: decoded.content,
                is_error: decoded.is_error,
                stop_loop: false,
            }
        } else {
            warn!(
                tool_use_id = %tool.id,
                tool_name = %tool.name,
                timeout_ms = timeout_dur.as_millis() as u64,
                "Tool timed out"
            );
            ToolCallResult::error(
                &tool.id,
                format!("Tool timed out after {}ms", timeout_dur.as_millis()),
            )
        }
    }
}

#[async_trait]
impl AgentToolExecutor for KernelToolExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        match self.execution_mode {
            ToolExecutionMode::Sequential => {
                let mut results = Vec::with_capacity(tool_calls.len());
                for tool in tool_calls {
                    results.push(self.execute_one(tool).await);
                }
                results
            }
            ToolExecutionMode::Parallel => {
                let futs: Vec<_> = tool_calls.iter().map(|t| self.execute_one(t)).collect();
                futures_util::future::join_all(futs).await
            }
        }
    }
}
