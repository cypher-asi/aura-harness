//! Default `AgentToolExecutor` implementation wrapping the kernel's `ExecutorRouter`.
//!
//! Bridges between the `AgentToolExecutor` trait (agent-loop layer) and the
//! existing executor infrastructure in `aura-executor`.

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use async_trait::async_trait;
use aura_core::{Action, AgentId, ToolCall};
use aura_core::{ExecuteContext, decode_tool_effect};
use aura_kernel::ExecutorRouter;
use std::path::PathBuf;
use tracing::{debug, error, info};

/// Bridges the `AgentToolExecutor` trait to the kernel's `ExecutorRouter`.
///
/// Translates `ToolCallInfo` into `Action`s, dispatches through the router,
/// and converts `Effect`s back into `ToolCallResult`s.
pub struct KernelToolExecutor {
    executor: ExecutorRouter,
    agent_id: AgentId,
    workspace: PathBuf,
}

impl KernelToolExecutor {
    /// Create a new executor bridge.
    #[must_use]
    pub const fn new(executor: ExecutorRouter, agent_id: AgentId, workspace: PathBuf) -> Self {
        Self {
            executor,
            agent_id,
            workspace,
        }
    }
}

#[async_trait]
impl AgentToolExecutor for KernelToolExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut results = Vec::new();

        for tool in tool_calls {
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
                    results.push(ToolCallResult {
                        tool_use_id: tool.id.clone(),
                        content: format!("Internal serialization error: {e}"),
                        is_error: true,
                        stop_loop: false,
                    });
                    continue;
                }
            };
            let ctx = ExecuteContext::new(self.agent_id, action.action_id, self.workspace.clone());

            let effect = self.executor.execute(&ctx, &action).await;
            let decoded = decode_tool_effect(&effect);
            let (content, is_error) = (decoded.content, decoded.is_error);

            info!(
                tool_use_id = %tool.id,
                tool_name = %tool.name,
                is_error = is_error,
                effect_status = ?effect.status,
                result_len = content.len(),
                workspace = %self.workspace.display(),
                "Tool execution completed"
            );

            results.push(ToolCallResult {
                tool_use_id: tool.id.clone(),
                content,
                is_error,
                stop_loop: false,
            });
        }

        results
    }
}
