//! `list_agents` — discover agents within the caller's scope.
//!
//! Requires [`Capability::ListAgents`]. Runtime reads are delegated to
//! [`crate::AgentReadHook::list_agents`] when wired.

use crate::agents::send_to_agent::missing_runtime_hook;
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{Capability, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const LIST_AGENTS_TOOL_NAME: &str = "list_agents";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAgentsInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub agents: serde_json::Value,
}

pub struct ListAgentsTool;

impl ListAgentsTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            LIST_AGENTS_TOOL_NAME,
            "List agents visible to the caller. Requires Capability::ListAgents.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "org_id": {
                        "type": "string",
                        "description": "Optional organization id. Required when the caller is scoped to more than one org."
                    }
                },
                "required": []
            }),
        )
    }

    pub fn evaluate(
        ctx: &ToolContext,
        input: &ListAgentsInput,
    ) -> Result<Option<String>, ToolError> {
        let caller_permissions = ctx.caller_permissions.as_ref().ok_or_else(|| {
            ToolError::InvalidArguments(
                "list_agents requires caller_permissions on the tool context".into(),
            )
        })?;

        if !caller_permissions
            .capabilities
            .contains(&Capability::ListAgents)
        {
            return Err(ToolError::InvalidArguments(
                "permissions: list_agents requires ListAgents capability".into(),
            ));
        }

        let scope = &caller_permissions.scope;
        let org_id = input
            .org_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());

        match (scope.orgs.as_slice(), org_id) {
            ([], Some(id)) => Ok(Some(id.to_string())),
            ([id], None) => Ok(Some(id.to_string())),
            ([], None) => Ok(None),
            (allowed, Some(id)) if allowed.iter().any(|allowed| allowed == id) => {
                Ok(Some(id.to_string()))
            }
            (allowed, Some(id)) => Err(ToolError::InvalidArguments(format!(
                "permissions: org '{id}' is not within the caller's AgentScope::orgs {allowed:?}"
            ))),
            (allowed, None) => Err(ToolError::InvalidArguments(format!(
                "permissions: list_agents requires org_id for caller scoped to orgs {allowed:?}"
            ))),
        }
    }

    fn filter_agents_to_scope(ctx: &ToolContext, agents: serde_json::Value) -> serde_json::Value {
        let Some(agent_ids) = ctx
            .caller_permissions
            .as_ref()
            .map(|perms| &perms.scope.agent_ids)
            .filter(|ids| !ids.is_empty())
        else {
            return agents;
        };

        match agents {
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .into_iter()
                    .filter(|agent| {
                        agent
                            .get("agent_id")
                            .or_else(|| agent.get("agentId"))
                            .or_else(|| agent.get("id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|id| agent_ids.iter().any(|allowed| allowed == id))
                    })
                    .collect(),
            ),
            other => other,
        }
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &str {
        LIST_AGENTS_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::ListAgents]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: ListAgentsInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("list_agents: {e}")))?;

        let org_id = match Self::evaluate(ctx, &input) {
            Ok(org_id) => org_id,
            Err(err) => {
                return Ok(ToolResult::failure(
                    LIST_AGENTS_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        let Some(hook) = ctx.agent_read_hook.as_ref() else {
            return Ok(missing_runtime_hook(LIST_AGENTS_TOOL_NAME));
        };

        let agents = match hook.list_agents(org_id.as_deref()).await {
            Ok(value) => Self::filter_agents_to_scope(ctx, value),
            Err(err) => {
                return Ok(ToolResult::failure(
                    LIST_AGENTS_TOOL_NAME,
                    Bytes::from(format!("list_agents hook: {err}").into_bytes()),
                ));
            }
        };

        let outcome = ListAgentsOutcome { org_id, agents };
        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("list_agents outcome: {e}")))?;
        Ok(ToolResult::success(LIST_AGENTS_TOOL_NAME, body))
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
        ctx
    }

    #[test]
    fn requires_list_agents_capability() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ReadAgent],
        };
        let err = ListAgentsTool::evaluate(&ctx(caller), &ListAgentsInput::default()).unwrap_err();
        assert!(err.to_string().contains("ListAgents"), "got: {err}");
    }

    #[test]
    fn allows_universe_scope_without_org() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::ListAgents],
        };
        let org_id = ListAgentsTool::evaluate(&ctx(caller), &ListAgentsInput::default()).unwrap();
        assert_eq!(org_id, None);
    }

    #[test]
    fn defaults_single_scoped_org() {
        let caller = AgentPermissions {
            scope: AgentScope {
                orgs: vec!["org-1".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::ListAgents],
        };
        let org_id = ListAgentsTool::evaluate(&ctx(caller), &ListAgentsInput::default()).unwrap();
        assert_eq!(org_id.as_deref(), Some("org-1"));
    }

    #[test]
    fn denies_out_of_scope_org() {
        let caller = AgentPermissions {
            scope: AgentScope {
                orgs: vec!["org-1".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::ListAgents],
        };
        let input = ListAgentsInput {
            org_id: Some("org-2".into()),
        };
        let err = ListAgentsTool::evaluate(&ctx(caller), &input).unwrap_err();
        assert!(err.to_string().contains("not within"), "got: {err}");
    }
}
