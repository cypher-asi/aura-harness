//! `send_to_agent` — phase 5 cross-agent tool.
//!
//! Delivers a user-message-shaped payload to a target agent. The caller must
//! hold [`Capability::ControlAgent`] and the target must be in the caller's
//! [`AgentScope::agent_ids`] (universe scope = any target allowed).
//!
//! Runtime effect (message delivery to the target agent's reasoner) is
//! performed by [`crate::AgentControlHook::deliver_message`] when wired.
//! Without a hook the tool still executes the gate and returns a
//! descriptive outcome — see [`crate::agents`] module docs.

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{AgentId, Capability, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const SEND_TO_AGENT_TOOL_NAME: &str = "send_to_agent";

/// Input schema for [`SendToAgentTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendToAgentInput {
    /// Target agent id (hex).
    pub agent_id: String,
    /// Message content to deliver.
    pub content: String,
    /// Optional structured attachments to forward along with the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendToAgentOutcome {
    pub target_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
    pub delivered: bool,
}

pub struct SendToAgentTool;

impl SendToAgentTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            SEND_TO_AGENT_TOOL_NAME,
            "Send a user-message-shaped payload to another agent within the \
             caller's scope. Requires Capability::ControlAgent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "content": { "type": "string" },
                    "attachments": {}
                },
                "required": ["agent_id", "content"]
            }),
        )
    }

    /// Pure gate — evaluates the permission check without performing any
    /// runtime side-effect.
    pub fn evaluate(
        ctx: &ToolContext,
        input: &SendToAgentInput,
    ) -> Result<SendToAgentOutcome, ToolError> {
        evaluate_control_gate(ctx, &input.agent_id, "send_to_agent")?;
        Ok(SendToAgentOutcome {
            target_agent_id: input.agent_id.clone(),
            parent_agent_id: ctx.caller_agent_id.map(|id| id.to_string()),
            originating_user_id: ctx.originating_user_id.clone(),
            delivered: false,
        })
    }
}

#[async_trait]
impl Tool for SendToAgentTool {
    fn name(&self) -> &str {
        SEND_TO_AGENT_TOOL_NAME
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
        let input: SendToAgentInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("send_to_agent: {e}")))?;

        let mut outcome = match Self::evaluate(ctx, &input) {
            Ok(o) => o,
            Err(err) => {
                return Ok(ToolResult::failure(
                    SEND_TO_AGENT_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        if let Some(hook) = ctx.agent_control_hook.as_ref() {
            let target = parse_agent_id(&input.agent_id, "send_to_agent")?;
            let parent = ctx.caller_agent_id.unwrap_or_else(AgentId::generate);
            match hook
                .deliver_message(
                    &target,
                    &parent,
                    ctx.originating_user_id.as_deref(),
                    &input.content,
                    input.attachments.clone(),
                )
                .await
            {
                Ok(()) => outcome.delivered = true,
                Err(err) => {
                    return Ok(ToolResult::failure(
                        SEND_TO_AGENT_TOOL_NAME,
                        Bytes::from(format!("send_to_agent hook: {err}").into_bytes()),
                    ));
                }
            }
        }

        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("send_to_agent outcome: {e}")))?;
        Ok(ToolResult::success(SEND_TO_AGENT_TOOL_NAME, body)
            .with_metadata("target_agent_id", outcome.target_agent_id.clone()))
    }
}

// ---------------------------------------------------------------------------
// Shared permission gate helpers (used by send_to_agent / agent_lifecycle /
// delegate_task / get_agent_state).
// ---------------------------------------------------------------------------

pub(crate) fn evaluate_control_gate(
    ctx: &ToolContext,
    target_agent_id: &str,
    tool_name: &str,
) -> Result<(), ToolError> {
    evaluate_gate(ctx, target_agent_id, tool_name, &Capability::ControlAgent)
}

pub(crate) fn evaluate_read_gate(
    ctx: &ToolContext,
    target_agent_id: &str,
    tool_name: &str,
) -> Result<(), ToolError> {
    evaluate_gate(ctx, target_agent_id, tool_name, &Capability::ReadAgent)
}

fn evaluate_gate(
    ctx: &ToolContext,
    target_agent_id: &str,
    tool_name: &str,
    required: &Capability,
) -> Result<(), ToolError> {
    let caller_permissions = ctx.caller_permissions.as_ref().ok_or_else(|| {
        ToolError::InvalidArguments(format!(
            "{tool_name} requires caller_permissions on the tool context"
        ))
    })?;

    if !caller_permissions.capabilities.contains(required) {
        return Err(ToolError::InvalidArguments(format!(
            "permissions: {tool_name} requires {required:?} capability"
        )));
    }

    let scope = &caller_permissions.scope;
    if !scope.agent_ids.is_empty() && !scope.agent_ids.iter().any(|id| id == target_agent_id) {
        return Err(ToolError::InvalidArguments(format!(
            "permissions: target agent '{target_agent_id}' is not within the caller's AgentScope::agent_ids"
        )));
    }

    Ok(())
}

pub(crate) fn parse_agent_id(s: &str, tool_name: &str) -> Result<AgentId, ToolError> {
    AgentId::from_hex(s).map_err(|e| {
        ToolError::InvalidArguments(format!("{tool_name}: invalid agent_id '{s}': {e}"))
    })
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

    #[test]
    fn send_to_agent_requires_control_capability() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ReadAgent],
        };
        let input = SendToAgentInput {
            agent_id: "aa".into(),
            content: "hello".into(),
            attachments: None,
        };
        let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
        assert!(err.to_string().contains("ControlAgent"), "got: {err}");
    }

    #[test]
    fn send_to_agent_denies_out_of_scope_target() {
        let caller = AgentPermissions {
            scope: AgentScope {
                agent_ids: vec!["allowed".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::ControlAgent],
        };
        let input = SendToAgentInput {
            agent_id: "not-allowed".into(),
            content: "hello".into(),
            attachments: None,
        };
        let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("permissions:"), "got: {err}");
        assert!(err.to_string().contains("AgentScope"), "got: {err}");
    }

    #[test]
    fn send_to_agent_allows_in_scope_target() {
        let caller = AgentPermissions {
            scope: AgentScope {
                agent_ids: vec!["target-id".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::ControlAgent],
        };
        let input = SendToAgentInput {
            agent_id: "target-id".into(),
            content: "hello".into(),
            attachments: None,
        };
        let outcome = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap();
        assert_eq!(outcome.target_agent_id, "target-id");
        assert_eq!(outcome.originating_user_id.as_deref(), Some("user-root"));
        assert!(
            !outcome.delivered,
            "no hook wired — runtime side-effect skipped"
        );
    }

    #[test]
    fn send_to_agent_universe_scope_allows_any_target() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ControlAgent],
        };
        let input = SendToAgentInput {
            agent_id: "anything".into(),
            content: "hi".into(),
            attachments: None,
        };
        assert!(SendToAgentTool::evaluate(&ctx(caller), &input).is_ok());
    }
}
