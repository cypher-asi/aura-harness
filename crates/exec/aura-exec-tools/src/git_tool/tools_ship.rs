//! The `git_ship` `Tool` impl: parses the arguments, logs the dispatch
//! and shapes the JSON result around [`git_ship_impl`].

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use tracing::{info, warn};

use super::push::duration_millis_u64;
use super::sandbox::{opt_str, push_branch_for, push_policy_for, str_arg, workspace_timeout};
use super::{git_ship_impl, PushTarget, ShipRequest};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// `git_ship` — commit everything, pull the target branch, push the
/// current branch and queue it for merging.
pub struct GitShipTool;

#[async_trait]
impl Tool for GitShipTool {
    fn name(&self) -> &str {
        "git_ship"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_ship".into(),
            description: "Finish a task: commit every change, pull the merge queue's target \
                branch (main unless the repo's committed .aura/merge-queue.toml names another) \
                into your branch, push your branch, and put it in the merge queue, which merges \
                it into that branch once the build and tests pass. Call it when your task is done."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Commit message for your changes (non-empty)."
                    },
                    "target": {
                        "type": "string",
                        "description": "Branch the merge queue merges into, pulled before the \
                            push. Defaults to the repo's queue target."
                    },
                    "task": {
                        "type": "string",
                        "description": "A few words naming the task. When your branch was \
                            merged already, your new commits move to a fresh branch named \
                            after it."
                    }
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let workspace = ctx.sandbox.root().to_path_buf();
        let target = match opt_str(&args, "target") {
            Some(target) => target.to_string(),
            None => aura_exec_merge_queue::resolve_target(&workspace).await,
        };
        let request = ShipRequest {
            message: str_arg(&args, "message").map_err(ToolError::from)?,
            target: &target,
            push: PushTarget {
                branch: None,
                force: false,
                only_branch: push_branch_for(ctx),
            },
            timeout: workspace_timeout(ctx),
            push_policy: push_policy_for(ctx),
            task: opt_str(&args, "task"),
        };
        log_ship_dispatch(ctx, &request);
        match git_ship_impl(&workspace, request).await {
            Ok(shipped) => Ok(ToolResult::success(
                "git_ship",
                serde_json::to_string(&shipped.to_json())
                    .unwrap_or_else(|_| format!("shipped {} at {}", shipped.branch, shipped.head)),
            )),
            Err(e) => {
                warn!(error = %e, "git_ship failed");
                Err(e.into())
            }
        }
    }
}

/// Emit the `git tool dispatched` info line before [`git_ship_impl`].
fn log_ship_dispatch(ctx: &ToolContext, request: &ShipRequest<'_>) {
    let agent_id = ctx
        .caller_agent_id
        .map_or_else(|| "unknown".to_string(), |id| id.to_string());
    info!(
        op = "git_ship",
        %agent_id,
        target = request.target,
        pinned_branch = request.push.only_branch.is_some(),
        timeout_ms = duration_millis_u64(request.timeout),
        push_per_attempt_timeout_ms = duration_millis_u64(request.push_policy.per_attempt_timeout),
        push_attempts = request.push_policy.attempts,
        "git tool dispatched"
    );
}
