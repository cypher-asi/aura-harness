//! `task` — foreground awaiting subagent tool.
//!
//! The tool itself is only a permission-checked dispatch surface. Runtime
//! orchestration lives behind [`crate::SubagentDispatchHook`].

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{Capability, SubagentDispatchRequest, SubagentResult, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const TASK_TOOL_NAME: &str = "task";
const MAX_SUBAGENT_DEPTH: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    pub subagent_type: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_addendum: Option<String>,
}

pub struct TaskTool;

impl TaskTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            TASK_TOOL_NAME,
            "Run a foreground subagent with isolated context and return its final summary. \
             Requires Capability::SpawnAgent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "subagent_type": {
                        "type": "string",
                        "description": "Bundled subagent type, for example general_purpose, explore, shell, or code_reviewer."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Task prompt for the subagent."
                    },
                    "model_override": {
                        "type": "string",
                        "description": "Optional runtime-approved model override."
                    },
                    "system_prompt_addendum": {
                        "type": "string",
                        "description": "Optional additional instructions appended by runtime policy."
                    }
                },
                "required": ["subagent_type", "prompt"]
            }),
        )
    }

    pub fn build_request(
        ctx: &ToolContext,
        input: &TaskInput,
    ) -> Result<SubagentDispatchRequest, ToolError> {
        if input.subagent_type.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task: subagent_type must not be empty".into(),
            ));
        }
        if input.prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task: prompt must not be empty".into(),
            ));
        }
        if ctx.parent_chain.len() >= MAX_SUBAGENT_DEPTH {
            return Err(ToolError::InvalidArguments(format!(
                "task: maximum subagent depth {MAX_SUBAGENT_DEPTH} exceeded"
            )));
        }

        let caller_permissions = ctx.caller_permissions.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "task requires caller_permissions on the tool context".into(),
            )
        })?;
        if !caller_permissions
            .capabilities
            .iter()
            .any(|cap| cap.satisfies(&Capability::SpawnAgent))
        {
            return Err(ToolError::InvalidArguments(
                "permissions: task requires SpawnAgent capability".into(),
            ));
        }

        let parent_agent_id = ctx.caller_agent_id.ok_or_else(|| {
            ToolError::InvalidArguments("task requires caller_agent_id on the tool context".into())
        })?;
        if ctx
            .parent_chain
            .iter()
            .any(|ancestor| *ancestor == parent_agent_id)
        {
            return Err(ToolError::InvalidArguments(
                "permissions: ancestor cycle detected in parent_chain".into(),
            ));
        }

        let mut parent_chain = Vec::with_capacity(ctx.parent_chain.len() + 1);
        parent_chain.push(parent_agent_id);
        parent_chain.extend(ctx.parent_chain.iter().copied());

        Ok(SubagentDispatchRequest {
            parent_agent_id,
            subagent_type: input.subagent_type.clone(),
            prompt: input.prompt.clone(),
            originating_user_id: ctx.originating_user_id.clone(),
            parent_chain,
            model_override: input.model_override.clone(),
            system_prompt_addendum: input.system_prompt_addendum.clone(),
            parent_permissions: caller_permissions,
            parent_tool_permissions: ctx.caller_tool_permissions.clone(),
            user_tool_defaults: ctx.user_tool_defaults.clone(),
        })
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        TASK_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpawnAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: TaskInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("task: {e}")))?;

        let request = match Self::build_request(ctx, &input) {
            Ok(request) => request,
            Err(err) => {
                return Ok(ToolResult::failure(
                    TASK_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        let Some(dispatch) = ctx.subagent_dispatch.as_ref() else {
            let result = SubagentResult::rejected("task: subagent dispatch hook is not wired");
            let body = serde_json::to_vec(&result)
                .map_err(|e| ToolError::Serialization(format!("task outcome: {e}")))?;
            return Ok(ToolResult::failure(TASK_TOOL_NAME, Bytes::from(body)));
        };

        let result = match dispatch.dispatch(request).await {
            Ok(result) => result,
            Err(err) => SubagentResult::rejected(format!("task dispatch: {err}")),
        };
        let ok = matches!(result.exit, aura_core::SubagentExit::Completed);
        let body = serde_json::to_vec(&result)
            .map_err(|e| ToolError::Serialization(format!("task outcome: {e}")))?;

        let mut tool_result = if ok {
            ToolResult::success(TASK_TOOL_NAME, Bytes::from(body))
        } else {
            ToolResult::failure(TASK_TOOL_NAME, Bytes::from(body))
        };
        if let Some(child_id) = result.child_agent_id {
            tool_result = tool_result.with_metadata("child_agent_id", child_id.to_string());
        }
        Ok(tool_result.with_metadata("subagent_type", input.subagent_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Sandbox;
    use crate::ToolConfig;
    use aura_core::{AgentId, AgentPermissions, AgentScope};

    fn ctx(caller: AgentPermissions) -> ToolContext {
        let dir = std::env::temp_dir();
        let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
        ctx.caller_permissions = Some(caller);
        ctx.caller_agent_id = Some(AgentId::generate());
        ctx.originating_user_id = Some("user-root".into());
        ctx
    }

    #[test]
    fn task_requires_spawn_capability() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ReadAgent],
        };
        let input = TaskInput {
            subagent_type: "explore".into(),
            prompt: "inspect".into(),
            model_override: None,
            system_prompt_addendum: None,
        };
        let err = TaskTool::build_request(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("SpawnAgent"), "got: {err}");
    }

    #[test]
    fn task_rejects_depth_limit() {
        let mut ctx = ctx(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        });
        ctx.parent_chain = vec![
            AgentId::generate(),
            AgentId::generate(),
            AgentId::generate(),
        ];
        let input = TaskInput {
            subagent_type: "explore".into(),
            prompt: "inspect".into(),
            model_override: None,
            system_prompt_addendum: None,
        };
        let err = TaskTool::build_request(&ctx, &input).unwrap_err();
        assert!(
            err.to_string().contains("maximum subagent depth"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn task_fails_closed_without_dispatch_hook() {
        let ctx = ctx(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        });
        let result = TaskTool
            .execute(
                &ctx,
                serde_json::json!({
                    "subagent_type": "explore",
                    "prompt": "inspect"
                }),
            )
            .await
            .unwrap();
        assert!(!result.ok);
        let outcome: SubagentResult = serde_json::from_slice(&result.stderr).unwrap();
        assert!(matches!(
            outcome.exit,
            aura_core::SubagentExit::Rejected { .. }
        ));
    }
}
