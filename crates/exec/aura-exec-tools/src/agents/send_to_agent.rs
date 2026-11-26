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
//!
//! ## Reply delivery contract
//!
//! `send_to_agent` is intentionally non-blocking: the tool returns as soon
//! as the target's `user_message` is persisted (the `x-aura-chat-persisted`
//! header on the SSE response is `true`). The target's reply is delivered
//! **asynchronously**: when its `AssistantMessageEnd` lands, the
//! aura-os-server persist task posts a follow-up `user_message` into the
//! originating agent's session carrying the target's reply text. The LLM
//! then sees that follow-up as a fresh turn and can react to it.
//!
//! The successful `ToolResult` carries a `reply_delivery=async_user_message`
//! metadata tag so the LLM-side prompt copy can stop trying to read the
//! reply out of the synchronous `delivered: true` body. See
//! `apps/aura-os-server/src/handlers/agents/chat/persist_task.rs` in the
//! aura-os repo for the server-side `spawn_cross_agent_reply_callback`
//! that owns the post-back side of this contract.

use crate::error::ToolError;
use crate::tool::{AgentMessage, Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{Capability, ToolDefinition, ToolResult};
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
    /// Short subject naming the conversation. The same subject continues
    /// a thread; a new one starts a thread. `None` continues the last
    /// thread with the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendToAgentOutcome {
    pub target_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
    pub delivered: bool,
    /// Human-readable guidance surfaced to the calling LLM in the tool
    /// result body. Set on a successful delivery to spell out the
    /// async-reply contract so the model waits for the target's reply
    /// to arrive on its own instead of trying to poll the target's
    /// state. Elided (and `None`) on the permission-gate-only
    /// [`SendToAgentTool::evaluate`] path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Guidance string stamped onto a successful [`SendToAgentOutcome`].
/// Kept as a constant so the wording stays consistent across the tool
/// body and is greppable from tests.
pub(crate) const ASYNC_REPLY_NOTE: &str =
    "Delivered. The target agent's reply will be delivered back to you automatically \
     as a new message in this conversation once it finishes responding. Do NOT poll, \
     and do NOT call get_agent_state to fetch the reply — simply end your turn. You \
     will be re-invoked with the reply when it is ready, and can relay it then.";

pub struct SendToAgentTool;

pub(crate) fn missing_runtime_hook(tool_name: &str) -> ToolResult {
    ToolResult::failure(
        tool_name,
        format!("{tool_name}: runtime hook not wired; configure AURA_OS_SERVER_URL on aura-node"),
    )
}

impl SendToAgentTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            SEND_TO_AGENT_TOOL_NAME,
            "Send a message to another agent within the caller's scope. \
             Requires Capability::ControlAgent. Give the message a short \
             subject: reuse the same subject to continue a conversation, and \
             use a new subject for a new topic. Delivery is asynchronous: \
             the call returns once the message is delivered, and the target \
             agent's reply is delivered back to you automatically as a new \
             message in this conversation when it finishes. Do not poll the \
             target or call get_agent_state to retrieve the reply — just end \
             your turn; you will be re-invoked with the reply when it is ready.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "content": { "type": "string" },
                    "subject": {
                        "type": "string",
                        "description": "Short subject of the conversation. Reuse the same \
                                        subject to continue a conversation; use a new subject \
                                        for a new topic."
                    },
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
            parent_agent_id: caller_external_id(ctx),
            originating_user_id: ctx.originating_user_id.clone(),
            delivered: false,
            note: None,
        })
    }
}

/// Resolve the caller's id for cross-agent server callbacks.
///
/// Always prefer the upstream OS UUID (`ctx.caller_external_agent_id`)
/// because it is what `aura-os-server`'s
/// `Path<AgentId = Uuid>` extractor on
/// `/api/agents/{originating_agent_id}/events/stream` expects when the
/// server-side `spawn_cross_agent_reply_callback` posts the recipient's
/// reply back into the originator's session. Falling back to
/// `ctx.caller_agent_id.to_string()` only preserves test scaffolding
/// that constructs `ToolContext` directly without the runtime
/// `with_caller_external_agent_id` wiring; in production the runtime
/// always populates the external id from `SessionState::skill_agent_id`,
/// so the fallback never fires for live chat. See the field doc on
/// `ToolContext::caller_external_agent_id` for the cross-repo
/// rationale.
fn caller_external_id(ctx: &ToolContext) -> Option<String> {
    if let Some(external) = ctx
        .caller_external_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return Some(external.to_string());
    }
    ctx.caller_agent_id.map(|id| id.to_string())
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

        let Some(hook) = ctx.agent_control_hook.as_ref() else {
            return Ok(missing_runtime_hook(SEND_TO_AGENT_TOOL_NAME));
        };

        let parent = caller_external_id(ctx);
        let message = AgentMessage {
            target_agent_id: &input.agent_id,
            parent_agent_id: parent.as_deref(),
            originating_user_id: ctx.originating_user_id.as_deref(),
            project_id: ctx.caller_project_id.as_deref(),
            content: &input.content,
            attachments: input.attachments.clone(),
            model: ctx.caller_model_id.as_deref(),
        };
        match hook
            .deliver_message_in_thread(message, input.subject.as_deref())
            .await
        {
            Ok(()) => {
                outcome.delivered = true;
                outcome.note = Some(ASYNC_REPLY_NOTE.to_string());
            }
            Err(err) => {
                return Ok(ToolResult::failure(
                    SEND_TO_AGENT_TOOL_NAME,
                    Bytes::from(format!("send_to_agent hook: {err}").into_bytes()),
                ));
            }
        }

        let body = serde_json::to_vec(&outcome)
            .map_err(|e| ToolError::Serialization(format!("send_to_agent outcome: {e}")))?;
        // `reply_delivery=async_user_message` documents the cross-repo
        // contract: aura-os-server posts the target's reply back into
        // the caller's session as a new `user_message` once the target's
        // turn finishes (see `spawn_cross_agent_reply_callback` in
        // `apps/aura-os-server/src/handlers/agents/chat/persist_task.rs`).
        // The LLM-side prompt copy can read this hint and stop trying
        // to read the reply out of the synchronous ToolResult body.
        Ok(ToolResult::success(SEND_TO_AGENT_TOOL_NAME, body)
            .with_metadata("target_agent_id", outcome.target_agent_id.clone())
            .with_metadata("reply_delivery", "async_user_message"))
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

#[cfg(test)]
mod tests;
