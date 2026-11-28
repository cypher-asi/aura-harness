//! `spawn_agent` — phase 5 cross-agent tool.
//!
//! ## Semantics
//!
//! - The caller must hold [`Capability::SpawnAgent`].
//! - Requested child permissions must be a **strict subset** of the caller's
//!   permissions (via [`AgentPermissions::contains`]).
//! - If `agent_id` is provided and already appears in the caller's
//!   [`crate::ToolContext::parent_chain`], the call is rejected (cycle
//!   prevention). The depth cap is enforced by the host behind the hook.
//! - On success the tool invokes [`aura_exec_traits::SpawnHook`] when one is
//!   attached to the [`ToolContext`]. The engine's agent host creates the
//!   child's `Identity`, appends the `Delegate` transaction on the caller's
//!   log, and keeps the child running; an optional `prompt` starts it
//!   working right away. The tool's stdout carries a [`SpawnAgentOutcome`]
//!   with the final child id and parent chain metadata.
//! - Without a hook the tool still runs the permission gate and returns a
//!   synthetic outcome — useful for unit tests and "dry-run" contexts.

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{
    resolve_effective_permission, AgentId, AgentPermissions, AgentToolPermissions, ToolDefinition,
    ToolResult,
};
use aura_exec_traits::{ChildAgentSpec, NoopSpawnHook};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// JSON name under which this tool is registered with the model.
pub const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";

/// Input schema for [`SpawnAgentTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentInput {
    pub name: String,
    pub role: String,
    /// Grants for the child. Omitted means the caller's own permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<AgentPermissions>,
    /// Optional first message. The child starts on it right away and its
    /// answer is delivered back to the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Optional explicit agent id for the child. When omitted a fresh id is
    /// generated. When provided, the id is checked against the caller's
    /// ancestor chain for cycles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional system-prompt override for the child agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Optional per-tool override for the child. Explicit entries must be no
    /// broader than the caller's effective state for the same tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_permissions: Option<AgentToolPermissions>,
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
            "Start a long-lived agent that works for you. It has the same tools you \
             do (so it can start agents of its own), keeps its conversation between \
             messages, and runs until you stop it with agent_lifecycle. Give it a \
             `prompt` to start it working right away; its answer is delivered back to \
             you as a message. Talk to it later with send_to_agent using the returned \
             child_agent_id. Its permissions can only be narrower than yours. For a \
             one-off job whose result you want back in this turn, use `task` instead.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short name for the agent; other agents can address it by this name."
                    },
                    "role": {
                        "type": "string",
                        "description": "What the agent is responsible for."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional first message. The agent starts on it immediately."
                    },
                    "system_prompt": {
                        "type": "string",
                        "description": "Optional system prompt replacing the one inherited from you."
                    },
                    "permissions": {
                        "type": "object",
                        "description": "Optional AgentPermissions bundle; must be a subset of yours. Defaults to your permissions."
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Optional preassigned child agent id. Checked for cycles against the caller's ancestor chain."
                    }
                },
                "required": ["name", "role"]
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

        let permissions = input
            .permissions
            .clone()
            .unwrap_or_else(|| caller_permissions.clone());
        if !caller_permissions.contains(&permissions) {
            return Err(ToolError::InvalidArguments(
                "permissions: requested grants exceed caller (strict subset required)".into(),
            ));
        }

        Self::check_tool_permissions(ctx, input)?;
        Self::check_no_ancestor_cycle(ctx, input)?;

        let child_id = input
            .agent_id
            .clone()
            .unwrap_or_else(|| AgentId::generate().to_string());

        Ok(SpawnAgentOutcome {
            child_agent_id: child_id,
            name: input.name.clone(),
            role: input.role.clone(),
            permissions,
            parent_agent_id: ctx.caller_agent_id.map(|id| id.to_string()),
            originating_user_id: ctx.originating_user_id.clone(),
        })
    }

    /// Every explicit per-tool entry must be no broader than the caller's
    /// effective state for the same tool.
    fn check_tool_permissions(ctx: &ToolContext, input: &SpawnAgentInput) -> Result<(), ToolError> {
        let Some(child_tools) = input.tool_permissions.as_ref() else {
            return Ok(());
        };
        for (tool, child_state) in &child_tools.per_tool {
            let parent_state = resolve_effective_permission(
                &ctx.user_tool_defaults,
                ctx.caller_tool_permissions.as_ref(),
                tool,
            );
            if !child_state.is_subset_of(parent_state) {
                return Err(ToolError::InvalidArguments(format!(
                    "tool permissions: requested '{tool}'={child_state:?} exceeds parent effective {parent_state:?}"
                )));
            }
        }
        Ok(())
    }

    /// A requested `agent_id` must not already be in the caller's parent chain.
    fn check_no_ancestor_cycle(
        ctx: &ToolContext,
        input: &SpawnAgentInput,
    ) -> Result<(), ToolError> {
        let Some(requested_id) = input.agent_id.as_deref() else {
            return Ok(());
        };
        let cycle = ctx
            .parent_chain
            .iter()
            .any(|ancestor| ancestor.to_string() == requested_id);
        if cycle {
            return Err(ToolError::InvalidArguments(format!(
                "permissions: ancestor cycle — requested agent_id '{requested_id}' is already in the caller's parent chain"
            )));
        }
        Ok(())
    }

    /// What the spawn hook needs to create the child for `outcome`.
    fn child_spec(
        ctx: &ToolContext,
        input: &SpawnAgentInput,
        outcome: &SpawnAgentOutcome,
    ) -> ChildAgentSpec {
        let preassigned = input
            .agent_id
            .as_deref()
            .and_then(|s| AgentId::from_hex(s).ok())
            .or_else(|| AgentId::from_hex(&outcome.child_agent_id).ok());

        ChildAgentSpec {
            name: input.name.clone(),
            role: input.role.clone(),
            permissions: outcome.permissions.clone(),
            tool_permissions: input.tool_permissions.clone(),
            parent_tool_permissions: ctx.caller_tool_permissions.clone(),
            system_prompt_override: input.system_prompt.clone(),
            preassigned_agent_id: preassigned,
            initial_prompt: input.prompt.clone(),
            description: None,
        }
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

    fn required_capabilities(&self) -> Vec<aura_core_types::Capability> {
        vec![aura_core_types::Capability::SpawnAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: SpawnAgentInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("spawn_agent: {e}")))?;

        let mut outcome = match Self::evaluate(ctx, &input) {
            Ok(o) => o,
            Err(err) => {
                return Ok(ToolResult::failure(
                    SPAWN_AGENT_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ))
            }
        };

        // Route through the kernel spawn hook when wired. The permission
        // checks have already passed in `evaluate`; the hook's job is pure
        // persistence.
        let hook = ctx
            .spawn_hook
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(NoopSpawnHook));
        let parent = ctx.caller_agent_id.unwrap_or_else(AgentId::generate);
        let spec = Self::child_spec(ctx, &input, &outcome);

        match hook
            .spawn_child(&parent, ctx.originating_user_id.as_deref(), spec)
            .await
        {
            Ok(hook_outcome) => {
                outcome.child_agent_id = hook_outcome
                    .external_agent_id
                    .unwrap_or_else(|| hook_outcome.child_agent_id.to_string());
            }
            Err(err) => {
                return Ok(ToolResult::failure(
                    SPAWN_AGENT_TOOL_NAME,
                    Bytes::from(format!("spawn_agent hook: {err}").into_bytes()),
                ));
            }
        }

        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("spawn_agent outcome: {e}")))?;
        Ok(ToolResult::success(SPAWN_AGENT_TOOL_NAME, body)
            .with_metadata("child_agent_id", outcome.child_agent_id.clone()))
    }
}

#[cfg(test)]
pub(crate) mod tests;
