//! Tool call execution logic with policy checks and caching.

use super::{ExecutedToolCall, ToolCache, TurnProcessor};
use aura_kernel::PermissionLevel;
use aura_core::{
    tool_result_cache_key, Action, AgentId, ToolCall, CACHEABLE_TOOLS,
};
use aura_core::{ExecuteContext, decode_tool_effect};
use aura_reasoner::{ContentBlock, Message, ModelProvider, ToolResultContent};
use aura_store::Store;
use aura_tools::ToolRegistry;
use std::collections::HashMap;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, warn};

impl<P, S, R> TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    /// Execute tool calls from a model message concurrently, with caching.
    ///
    /// Policy checks are performed synchronously first. Cacheable read-only
    /// tools are checked against `tool_cache`; cache hits are returned without
    /// re-execution. Permitted tools are then executed in parallel via
    /// `futures::future::join_all`. Successful cacheable results are stored
    /// back into the cache for future steps within the same turn.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn execute_tool_calls(
        &self,
        message: &Message,
        agent_id: AgentId,
        tool_cache: &mut ToolCache,
    ) -> anyhow::Result<Vec<ExecutedToolCall>> {
        let workspace = self.agent_workspace(&agent_id);

        if let Err(e) = tokio::fs::create_dir_all(&workspace).await {
            error!(error = %e, "Failed to create workspace");
        }

        // Phase 1: policy checks + cache lookups
        let mut denied = Vec::new();
        let mut cached = Vec::new();
        let mut to_execute: Vec<(String, String, serde_json::Value)> = Vec::new();

        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, id = %id, "Checking tool permission");

                let permission = self.policy.check_tool_permission(name);
                match permission {
                    PermissionLevel::Deny => {
                        warn!(tool = %name, "Tool denied by policy");
                        denied.push(ExecutedToolCall {
                            tool_use_id: id.clone(),
                            tool_name: name.clone(),
                            tool_args: input.clone(),
                            result: ToolResultContent::text(format!(
                                "Tool '{name}' is not allowed"
                            )),
                            is_error: true,
                            metadata: HashMap::default(),
                        });
                        continue;
                    }
                    PermissionLevel::AlwaysAsk => {
                        debug!(tool = %name, "Tool requires approval (AlwaysAsk)");
                    }
                    PermissionLevel::AskOnce => {
                        debug!(tool = %name, "Tool allowed (AskOnce)");
                    }
                    PermissionLevel::AlwaysAllow => {
                        debug!(tool = %name, "Tool allowed (AlwaysAllow)");
                    }
                }

                if CACHEABLE_TOOLS.contains(&name.as_str()) {
                    let cache_key = tool_result_cache_key(name, input);
                    if let Some(hit) = tool_cache.get(&cache_key) {
                        debug!(tool = %name, "Cache hit — returning cached result");
                        let mut cloned = hit.clone();
                        cloned.tool_use_id.clone_from(id);
                        cached.push(cloned);
                        continue;
                    }
                }

                to_execute.push((id.clone(), name.clone(), input.clone()));
            }
        }

        // Phase 2: execute permitted tools in parallel
        let tool_timeout = Duration::from_millis(self.config.tool_timeout_ms);
        let futures: Vec<_> = to_execute
            .into_iter()
            .map(|(id, name, input)| {
                let workspace = workspace.clone();
                async move {
                    let tool_call = ToolCall::new(name.clone(), input.clone());
                    let action = match Action::delegate_tool(&tool_call) {
                        Ok(a) => a,
                        Err(e) => {
                            return ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: ToolResultContent::text(format!(
                                    "Failed to create action: {e}"
                                )),
                                is_error: true,
                                metadata: HashMap::default(),
                            };
                        }
                    };
                    let ctx = ExecuteContext::new(agent_id, action.action_id, workspace);

                    let effect = match timeout(
                        tool_timeout,
                        self.executor.execute(&ctx, &action),
                    )
                    .await
                    {
                        Ok(effect) => effect,
                        Err(_) => {
                            return ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: ToolResultContent::text(format!(
                                    "Tool execution timed out after {}ms",
                                    tool_timeout.as_millis()
                                )),
                                is_error: true,
                                metadata: HashMap::default(),
                            };
                        }
                    };

                    let decoded = decode_tool_effect(&effect);
                    ExecutedToolCall {
                        tool_use_id: id,
                        tool_name: name,
                        tool_args: input,
                        result: ToolResultContent::text(decoded.content),
                        is_error: decoded.is_error,
                        metadata: decoded.metadata,
                    }
                }
            })
            .collect();

        let executed = futures_util::future::join_all(futures).await;

        // Phase 3: populate cache with successful cacheable results
        for result in &executed {
            if !result.is_error && CACHEABLE_TOOLS.contains(&result.tool_name.as_str()) {
                let cache_key = tool_result_cache_key(&result.tool_name, &result.tool_args);
                tool_cache.insert(cache_key, result.clone());
            }
        }

        let mut results = denied;
        results.extend(cached);
        results.extend(executed);
        Ok(results)
    }
}
