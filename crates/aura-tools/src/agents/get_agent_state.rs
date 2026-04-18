//! `get_agent_state` — phase 5 cross-agent tool.
//!
//! Returns a read-only snapshot of a target agent's state. Requires
//! [`Capability::ReadAgent`] and target-in-scope.
//!
//! Read logic is delegated to [`crate::AgentReadHook::snapshot`] when wired.

use crate::agents::send_to_agent::{evaluate_read_gate, parse_agent_id};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{Capability, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const GET_AGENT_STATE_TOOL_NAME: &str = "get_agent_state";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAgentStateInput {
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAgentStateOutcome {
    pub target_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
    /// `Some(snapshot)` when an [`crate::AgentReadHook`] is wired; `None`
    /// when running without a hook (gate-only mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<serde_json::Value>,
}

pub struct GetAgentStateTool;

impl GetAgentStateTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            GET_AGENT_STATE_TOOL_NAME,
            "Fetch a read-only snapshot of another agent's state within the \
             caller's scope. Requires Capability::ReadAgent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" }
                },
                "required": ["agent_id"]
            }),
        )
    }

    pub fn evaluate(
        ctx: &ToolContext,
        input: &GetAgentStateInput,
    ) -> Result<GetAgentStateOutcome, ToolError> {
        evaluate_read_gate(ctx, &input.agent_id, "get_agent_state")?;
        Ok(GetAgentStateOutcome {
            target_agent_id: input.agent_id.clone(),
            parent_agent_id: ctx.caller_agent_id.map(|id| id.to_string()),
            originating_user_id: ctx.originating_user_id.clone(),
            snapshot: None,
        })
    }
}

#[async_trait]
impl Tool for GetAgentStateTool {
    fn name(&self) -> &str {
        GET_AGENT_STATE_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::ReadAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: GetAgentStateInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("get_agent_state: {e}")))?;

        let mut outcome = match Self::evaluate(ctx, &input) {
            Ok(o) => o,
            Err(err) => {
                return Ok(ToolResult::failure(
                    GET_AGENT_STATE_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        if let Some(hook) = ctx.agent_read_hook.as_ref() {
            let target = parse_agent_id(&input.agent_id, "get_agent_state")?;
            match hook.snapshot(&target).await {
                Ok(value) => outcome.snapshot = Some(value),
                Err(err) => {
                    return Ok(ToolResult::failure(
                        GET_AGENT_STATE_TOOL_NAME,
                        Bytes::from(format!("get_agent_state hook: {err}").into_bytes()),
                    ));
                }
            }
        }

        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("get_agent_state outcome: {e}")))?;
        Ok(ToolResult::success(GET_AGENT_STATE_TOOL_NAME, body)
            .with_metadata("target_agent_id", outcome.target_agent_id.clone()))
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
    fn requires_read_capability() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ControlAgent],
        };
        let input = GetAgentStateInput {
            agent_id: "t".into(),
        };
        let err = GetAgentStateTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
        assert!(err.to_string().contains("ReadAgent"), "got: {err}");
    }

    #[test]
    fn denies_out_of_scope() {
        let caller = AgentPermissions {
            scope: AgentScope {
                agent_ids: vec!["ok".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::ReadAgent],
        };
        let input = GetAgentStateInput {
            agent_id: "nope".into(),
        };
        let err = GetAgentStateTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
    }

    #[test]
    fn allows_in_scope_without_hook_returns_no_snapshot() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ReadAgent],
        };
        let input = GetAgentStateInput {
            agent_id: "anything".into(),
        };
        let outcome = GetAgentStateTool::evaluate(&ctx(caller), &input).unwrap();
        assert_eq!(outcome.target_agent_id, "anything");
        assert!(outcome.snapshot.is_none());
    }
}
