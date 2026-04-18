//! `spawn_agent` — phase 5 cross-agent tool.
//!
//! This is the most complex of the phase-5 cross-agent tools and the template
//! for the rest (see `TODO(phase5-part-2)` in [`crate::agents`]).
//!
//! ## Semantics
//!
//! - The caller must hold [`Capability::SpawnAgent`].
//! - Requested child permissions must be a **strict subset** of the caller's
//!   permissions (via [`AgentPermissions::contains`]).
//! - If `agent_id` is provided and already appears in the caller's
//!   [`crate::ToolContext::parent_chain`], the call is rejected (cycle
//!   prevention). There is **no depth cap** — strict-subset plus the
//!   caller's budget are the natural bounds.
//! - On success the tool returns a [`ToolResult`] whose stdout JSON encodes a
//!   [`SpawnAgentOutcome`]. The outcome carries enough data for the kernel
//!   to emit a `Delegate` transaction with `parent_agent_id` /
//!   `originating_user_id` populated from the caller's context.
//!
//! ## What is NOT done in this commit
//!
//! This tool does not persist a new kernel agent record. The persistence hook
//! is deferred — see `TODO(phase5-part-2)` below. The tool returns the new
//! child's id + permissions in `SpawnAgentOutcome`; phase-5-part-2 will add a
//! `SpawnHook` trait that the aura-node wiring implements to actually create
//! the `Identity` + record log + scheduler slot.

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{AgentId, AgentPermissions, ToolDefinition, ToolResult};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// JSON name under which this tool is registered with the model.
pub const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";

/// Input schema for [`SpawnAgentTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentInput {
    pub name: String,
    pub role: String,
    pub permissions: AgentPermissions,
    /// Optional explicit agent id for the child. When omitted a fresh id is
    /// generated. When provided, the id is checked against the caller's
    /// ancestor chain for cycles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Structured outcome of a successful `spawn_agent` call. Embedded in the
/// `ToolResult.stdout` payload and parsed by the kernel-level bridge to emit
/// the `Delegate` transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnAgentOutcome {
    pub child_agent_id: String,
    pub name: String,
    pub role: String,
    pub permissions: AgentPermissions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
}

/// Implementation of the `spawn_agent` tool.
pub struct SpawnAgentTool;

impl SpawnAgentTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            SPAWN_AGENT_TOOL_NAME,
            "Spawn a subordinate agent whose scope + capabilities are a strict \
             subset of the caller's. Returns the new child's agent id. Requires \
             Capability::SpawnAgent on the caller.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "role": { "type": "string" },
                    "permissions": {
                        "type": "object",
                        "description": "AgentPermissions bundle; must be a subset of caller's."
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Optional preassigned child agent id. Checked for cycles against the caller's ancestor chain."
                    }
                },
                "required": ["name", "role", "permissions"]
            }),
        )
    }

    /// Pure core — extracted so unit tests can drive it without an
    /// async runtime or a `Sandbox`.
    pub fn evaluate(
        ctx: &ToolContext,
        input: &SpawnAgentInput,
    ) -> Result<SpawnAgentOutcome, ToolError> {
        let caller_permissions = ctx.caller_permissions.as_ref().ok_or_else(|| {
            ToolError::InvalidArguments(
                "spawn_agent requires caller_permissions on the tool context".into(),
            )
        })?;

        if !caller_permissions.contains(&input.permissions) {
            return Err(ToolError::InvalidArguments(
                "permissions: requested grants exceed caller (strict subset required)".into(),
            ));
        }

        if let Some(requested_id) = input.agent_id.as_deref() {
            let cycle = ctx
                .parent_chain
                .iter()
                .any(|ancestor| ancestor.to_string() == requested_id);
            if cycle {
                return Err(ToolError::InvalidArguments(format!(
                    "permissions: ancestor cycle — requested agent_id '{requested_id}' is already in the caller's parent chain"
                )));
            }
        }

        let child_id = input
            .agent_id
            .clone()
            .unwrap_or_else(|| AgentId::generate().to_string());

        // TODO(phase5-part-2): invoke a SpawnHook here to (a) create the
        // kernel `Identity` record for the new agent, (b) seed its record log
        // with a SessionStart, and (c) emit the `Delegate` transaction with
        // parent_agent_id + originating_user_id on the *caller's* log so the
        // record chain captures the spawn. The test harness below asserts the
        // outcome payload a hook would consume.

        Ok(SpawnAgentOutcome {
            child_agent_id: child_id,
            name: input.name.clone(),
            role: input.role.clone(),
            permissions: input.permissions.clone(),
            parent_agent_id: ctx.caller_agent_id.map(|id| id.to_string()),
            originating_user_id: ctx.originating_user_id.clone(),
        })
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn name(&self) -> &str {
        SPAWN_AGENT_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: SpawnAgentInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("spawn_agent: {e}")))?;

        match Self::evaluate(ctx, &input) {
            Ok(outcome) => {
                let body = serde_json::to_vec(&outcome).map_err(|e| {
                    ToolError::Serialization(format!("spawn_agent outcome: {e}"))
                })?;
                Ok(ToolResult::success(SPAWN_AGENT_TOOL_NAME, body)
                    .with_metadata("child_agent_id", outcome.child_agent_id))
            }
            Err(err) => Ok(ToolResult::failure(
                SPAWN_AGENT_TOOL_NAME,
                Bytes::from(err.to_string().into_bytes()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Sandbox;
    use crate::ToolConfig;
    use aura_core::{AgentScope, Capability};

    fn ctx_with(
        caller: AgentPermissions,
        caller_agent_id: Option<AgentId>,
        parent_chain: Vec<AgentId>,
    ) -> ToolContext {
        let dir = std::env::temp_dir();
        let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
        ctx.caller_permissions = Some(caller);
        ctx.caller_agent_id = caller_agent_id;
        ctx.parent_chain = parent_chain;
        ctx.originating_user_id = Some("user-root".into());
        ctx
    }

    fn ceo() -> AgentPermissions {
        AgentPermissions::ceo_preset()
    }

    #[test]
    fn spawn_agent_denies_scope_escalation() {
        let caller = AgentPermissions {
            scope: AgentScope {
                orgs: vec!["a".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::SpawnAgent],
        };
        let ctx = ctx_with(caller, Some(AgentId::generate()), vec![]);

        let input = SpawnAgentInput {
            name: "child".into(),
            role: "worker".into(),
            permissions: AgentPermissions {
                scope: AgentScope {
                    orgs: vec!["a".into(), "b".into()],
                    ..AgentScope::default()
                },
                capabilities: vec![Capability::SpawnAgent],
            },
            agent_id: None,
        };

        let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
        assert!(err.to_string().contains("strict subset"), "got: {err}");
    }

    #[test]
    fn spawn_agent_denies_capability_escalation() {
        let caller = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        };
        let ctx = ctx_with(caller, Some(AgentId::generate()), vec![]);

        let input = SpawnAgentInput {
            name: "child".into(),
            role: "worker".into(),
            permissions: AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![Capability::SpawnAgent, Capability::ManageBilling],
            },
            agent_id: None,
        };

        let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
        assert!(err.to_string().contains("strict subset"), "got: {err}");
    }

    #[test]
    fn spawn_agent_rejects_ancestor_cycle() {
        let ancestor = AgentId::generate();
        let ctx = ctx_with(ceo(), Some(AgentId::generate()), vec![ancestor]);

        let input = SpawnAgentInput {
            name: "child".into(),
            role: "worker".into(),
            permissions: AgentPermissions::empty(),
            agent_id: Some(ancestor.to_string()),
        };

        let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
        assert!(err.to_string().contains("cycle"), "got: {err}");
    }

    #[test]
    fn spawn_agent_allows_proper_subset() {
        let caller_id = AgentId::generate();
        let ctx = ctx_with(ceo(), Some(caller_id), vec![]);

        let input = SpawnAgentInput {
            name: "child".into(),
            role: "worker".into(),
            permissions: AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![Capability::SpawnAgent],
            },
            agent_id: None,
        };

        let outcome = SpawnAgentTool::evaluate(&ctx, &input).unwrap();
        assert_eq!(outcome.name, "child");
        assert_eq!(outcome.role, "worker");
        assert_eq!(outcome.parent_agent_id, Some(caller_id.to_string()));
        assert_eq!(outcome.originating_user_id.as_deref(), Some("user-root"));
        assert!(!outcome.child_agent_id.is_empty());
    }

    #[test]
    fn spawn_agent_requires_caller_permissions_on_ctx() {
        let dir = std::env::temp_dir();
        let ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
        let input = SpawnAgentInput {
            name: "x".into(),
            role: "x".into(),
            permissions: AgentPermissions::empty(),
            agent_id: None,
        };
        let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
        assert!(
            err.to_string().contains("caller_permissions"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn spawn_agent_tool_wraps_outcome_in_tool_result() {
        let caller_id = AgentId::generate();
        let ctx = ctx_with(ceo(), Some(caller_id), vec![]);
        let args = serde_json::json!({
            "name": "child",
            "role": "worker",
            "permissions": AgentPermissions::empty()
        });
        let result = SpawnAgentTool.execute(&ctx, args).await.unwrap();
        assert!(result.ok);
        let outcome: SpawnAgentOutcome = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(outcome.parent_agent_id, Some(caller_id.to_string()));
    }
}
