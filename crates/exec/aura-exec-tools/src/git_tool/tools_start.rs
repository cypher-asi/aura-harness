//! The `git_start_task` `Tool` impl: a fresh branch off the latest
//! target for each task ([`super::branches`]).

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use tracing::{info, warn};

use super::branches::{agent_code, start_task_branch, task_branch};
use super::push::current_branch;
use super::sandbox::{opt_str, push_branch_for, str_arg, workspace_timeout};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// The code a branch gets when neither the pin nor the current branch
/// names the agent.
const NO_CODE: &str = "agent";

/// `git_start_task` — start the task's own branch.
pub struct GitStartTaskTool;

#[async_trait]
impl Tool for GitStartTaskTool {
    fn name(&self) -> &str {
        "git_start_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_start_task".into(),
            description: "Start a task on its own fresh branch, `aura/<task>-<your code>`, made \
                from the latest merge queue target (main). Call it before changing anything for \
                a new task; never reuse an earlier task's branch. A name already taken gets a \
                number. Refuses while uncommitted changes or unpushed commits would be left \
                behind: ship them first."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["task"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "A few words naming the task; they become the branch name."
                    },
                    "target": {
                        "type": "string",
                        "description": "Branch to start from. Defaults to the repo's queue target."
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
        let task = str_arg(&args, "task").map_err(ToolError::from)?;
        let target = match opt_str(&args, "target") {
            Some(target) => target.to_string(),
            None => aura_exec_merge_queue::resolve_target(&workspace).await,
        };
        let timeout = workspace_timeout(ctx);
        let current = current_branch(&workspace, timeout)
            .await
            .map_err(ToolError::from)?;
        let code = push_branch_for(ctx)
            .and_then(agent_code)
            .or_else(|| current.as_deref().and_then(agent_code))
            .unwrap_or(NO_CODE)
            .to_string();
        let wanted = task_branch(task, &code);
        info!(op = "git_start_task", %wanted, %target, "git tool dispatched");
        match start_task_branch(&workspace, &wanted, &target, timeout).await {
            Ok(started) => Ok(ToolResult::success(
                "git_start_task",
                serde_json::json!({
                    "started": true,
                    "branch": started.branch,
                    "base": started.base,
                    "from": started.from,
                    "previous": current,
                })
                .to_string(),
            )),
            Err(e) => {
                warn!(error = %e, "git_start_task failed");
                Err(e.into())
            }
        }
    }
}
