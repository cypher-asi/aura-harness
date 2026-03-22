//! Tool call execution logic with policy checks and caching.

use super::{ExecutedToolCall, ToolCache, TurnProcessor};
use crate::policy::PermissionLevel;
use aura_core::{Action, AgentId, EffectStatus, ToolCall, ToolResult};
use aura_executor::ExecuteContext;
use aura_reasoner::{ContentBlock, Message, ModelProvider, ToolResultContent};
use aura_store::Store;
use aura_tools::ToolRegistry;
use std::collections::HashMap;
use tracing::{debug, error, warn};

/// Tools whose results are safe to cache (no side effects).
const CACHEABLE_TOOLS: &[&str] = &["fs_ls", "fs_read", "fs_stat", "fs_find", "search_code"];

/// Build a deterministic cache key from tool name and arguments.
///
/// Uses canonical JSON serialization so equivalent argument objects
/// produce the same key regardless of property ordering.
fn make_cache_key(tool_name: &str, args: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(args).unwrap_or_default();
    format!("{tool_name}\0{canonical}")
}

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
                    let cache_key = make_cache_key(name, input);
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
                                result: ToolResultContent::text(format!("Failed to create action: {e}")),
                                is_error: true,
                                metadata: HashMap::default(),
                            };
                        }
                    };
                    let ctx = ExecuteContext::new(agent_id, action.action_id, workspace);

                    let effect = self.executor.execute(&ctx, &action).await;

                    if effect.status == EffectStatus::Committed {
                        if let Ok(tool_result) =
                            serde_json::from_slice::<ToolResult>(&effect.payload)
                        {
                            let content = if tool_result.stdout.is_empty() {
                                ToolResultContent::text("Success (no output)")
                            } else {
                                ToolResultContent::text(
                                    String::from_utf8_lossy(&tool_result.stdout).to_string(),
                                )
                            };
                            ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: content,
                                is_error: !tool_result.ok,
                                metadata: tool_result.metadata,
                            }
                        } else {
                            ExecutedToolCall {
                                tool_use_id: id,
                                tool_name: name,
                                tool_args: input,
                                result: ToolResultContent::text("Tool executed successfully"),
                                is_error: false,
                                metadata: HashMap::default(),
                            }
                        }
                    } else {
                        let error_msg = if let Ok(tool_result) =
                            serde_json::from_slice::<ToolResult>(&effect.payload)
                        {
                            String::from_utf8_lossy(&tool_result.stderr).to_string()
                        } else {
                            "Tool execution failed".to_string()
                        };
                        ExecutedToolCall {
                            tool_use_id: id,
                            tool_name: name,
                            tool_args: input,
                            result: ToolResultContent::text(error_msg),
                            is_error: true,
                            metadata: HashMap::default(),
                        }
                    }
                }
            })
            .collect();

        let executed = futures_util::future::join_all(futures).await;

        // Phase 3: populate cache with successful cacheable results
        for result in &executed {
            if !result.is_error && CACHEABLE_TOOLS.contains(&result.tool_name.as_str()) {
                let cache_key = make_cache_key(&result.tool_name, &result.tool_args);
                tool_cache.insert(cache_key, result.clone());
            }
        }

        let mut results = denied;
        results.extend(cached);
        results.extend(executed);
        Ok(results)
    }
}
