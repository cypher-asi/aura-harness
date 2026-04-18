//! `agent_lifecycle` — phase 5 cross-agent tool.
//!
//! Apply a lifecycle transition (`pause`, `resume`, `stop`, `restart`) to a
//! target agent within the caller's scope. Requires
//! [`Capability::ControlAgent`].
//!
//! Runtime effect is delegated to
//! [`crate::AgentControlHook::lifecycle`] when wired.

use crate::agents::send_to_agent::{evaluate_control_gate, parse_agent_id};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{AgentId, Capability, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const AGENT_LIFECYCLE_TOOL_NAME: &str = "agent_lifecycle";

const VALID_ACTIONS: &[&str] = &["pause", "resume", "stop", "restart"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLifecycleInput {
    pub agent_id: String,
    /// One of: `pause`, `resume`, `stop`, `restart`.
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentLifecycleOutcome {
    pub target_agent_id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
    pub applied: bool,
}

pub struct AgentLifecycleTool;

impl AgentLifecycleTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            AGENT_LIFECYCLE_TOOL_NAME,
            "Apply a lifecycle transition (pause|resume|stop|restart) to another \
             agent within the caller's scope. Requires Capability::ControlAgent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "action": {
                        "type": "string",
                        "enum": ["pause", "resume", "stop", "restart"]
                    }
                },
                "required": ["agent_id", "action"]
            }),
        )
    }

    pub fn evaluate(
        ctx: &ToolContext,
        input: &AgentLifecycleInput,
    ) -> Result<AgentLifecycleOutcome, ToolError> {
        if !VALID_ACTIONS.contains(&input.action.as_str()) {
            return Err(ToolError::InvalidArguments(format!(
                "agent_lifecycle: unsupported action '{}' (expected one of {:?})",
                input.action, VALID_ACTIONS
            )));
        }
        evaluate_control_gate(ctx, &input.agent_id, "agent_lifecycle")?;
        Ok(AgentLifecycleOutcome {
            target_agent_id: input.agent_id.clone(),
            action: input.action.clone(),
            parent_agent_id: ctx.caller_agent_id.map(|id| id.to_string()),
            originating_user_id: ctx.originating_user_id.clone(),
            applied: false,
        })
    }
}

#[async_trait]
impl Tool for AgentLifecycleTool {
    fn name(&self) -> &str {
        AGENT_LIFECYCLE_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::ControlAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: AgentLifecycleInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("agent_lifecycle: {e}")))?;

        let mut outcome = match Self::evaluate(ctx, &input) {
            Ok(o) => o,
            Err(err) => {
                return Ok(ToolResult::failure(
                    AGENT_LIFECYCLE_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        if let Some(hook) = ctx.agent_control_hook.as_ref() {
            let target = parse_agent_id(&input.agent_id, "agent_lifecycle")?;
            let parent = ctx.caller_agent_id.unwrap_or_else(AgentId::generate);
            match hook
                .lifecycle(
                    &target,
                    &parent,
                    ctx.originating_user_id.as_deref(),
                    &input.action,
                )
                .await
            {
                Ok(()) => outcome.applied = true,
                Err(err) => {
                    return Ok(ToolResult::failure(
                        AGENT_LIFECYCLE_TOOL_NAME,
                        Bytes::from(format!("agent_lifecycle hook: {err}").into_bytes()),
                    ));
                }
            }
        }

        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("agent_lifecycle outcome: {e}")))?;
        Ok(ToolResult::success(AGENT_LIFECYCLE_TOOL_NAME, body)
            .with_metadata("target_agent_id", outcome.target_agent_id.clone())
            .with_metadata("action", outcome.action.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Sandbox;
    use crate::ToolConfig;
    use aura_core::{AgentPermissions, AgentScope};

    fn ctx(caller: AgentPermissions) -> ToolContext {
        let dir = std::env::temp_dir();
        let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
        ctx.caller_permissions = Some(caller);
        ctx.caller_agent_id = Some(AgentId::generate());
        ctx.originating_user_id = Some("user-root".into());
        ctx
    }

    fn controller_with_scope(scope: AgentScope) -> AgentPermissions {
        AgentPermissions {
            scope,
            capabilities: vec![Capability::ControlAgent],
        }
    }

    #[test]
    fn rejects_unknown_action() {
        let caller = controller_with_scope(AgentScope::default());
        let input = AgentLifecycleInput {
            agent_id: "t".into(),
            action: "launch_missiles".into(),
        };
        let err = AgentLifecycleTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("unsupported action"), "got: {err}");
    }

    #[test]
    fn rejects_missing_capability() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ReadAgent],
        };
        let input = AgentLifecycleInput {
            agent_id: "t".into(),
            action: "pause".into(),
        };
        let err = AgentLifecycleTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
    }

    #[test]
    fn rejects_out_of_scope_target() {
        let caller = controller_with_scope(AgentScope {
            agent_ids: vec!["allowed".into()],
            ..AgentScope::default()
        });
        let input = AgentLifecycleInput {
            agent_id: "not-allowed".into(),
            action: "pause".into(),
        };
        let err = AgentLifecycleTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
    }

    #[test]
    fn allows_valid_action_in_scope() {
        let caller = controller_with_scope(AgentScope::default());
        let input = AgentLifecycleInput {
            agent_id: "any".into(),
            action: "restart".into(),
        };
        let outcome = AgentLifecycleTool::evaluate(&ctx(caller), &input).unwrap();
        assert_eq!(outcome.action, "restart");
        assert!(!outcome.applied);
    }
}
