//! Default `AgentToolExecutor` implementation wrapping the kernel's `ExecutorRouter`.
//!
//! Bridges between the `AgentToolExecutor` trait (agent-loop layer) and the
//! existing executor infrastructure in `aura-executor`.

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use async_trait::async_trait;
use aura_core::{Action, AgentId, EffectStatus, ToolCall, ToolResult};
use aura_executor::{ExecuteContext, ExecutorRouter};
use std::path::PathBuf;

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
            let tool_call = ToolCall::new(tool.name.clone(), tool.input.clone());
            let action = match Action::delegate_tool(&tool_call) {
                Ok(a) => a,
                Err(e) => {
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

            let (content, is_error) = if effect.status == EffectStatus::Committed {
                match serde_json::from_slice::<ToolResult>(&effect.payload) {
                    Ok(tool_result) => {
                        let text = if tool_result.stdout.is_empty() {
                            "Success (no output)".to_string()
                        } else {
                            String::from_utf8_lossy(&tool_result.stdout).to_string()
                        };
                        (text, !tool_result.ok)
                    }
                    Err(_) => ("Tool executed successfully".to_string(), false),
                }
            } else {
                let err_msg = if let Ok(tool_result) =
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
                (err_msg, true)
            };

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
