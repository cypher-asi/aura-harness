//! Shared wire protocol types for the Aura harness WebSocket API.
//!
//! Defines the inbound (client → server) and outbound (server → client)
//! message format for the `/stream` WebSocket endpoint.
//!
//! This crate is consumed by both the harness server (`aura-node`) and
//! any client implementation (e.g. `aura-os-link`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "typescript")]
use ts_rs::TS;

// ============================================================================
// Inbound Messages (Client → Server)
// ============================================================================

/// Top-level inbound message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub enum InboundMessage {
    /// Initialize the session (must be the first message).
    SessionInit(Box<SessionInit>),
    /// Send a user message for processing.
    UserMessage(UserMessage),
    /// Cancel the current turn.
    Cancel,
    /// Respond to an approval request.
    ApprovalResponse(ApprovalResponse),
}

/// A prior conversation message used to hydrate session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
}

/// Payload for `session_init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SessionInit {
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Model identifier (e.g., "claude-opus-4-6").
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum tokens per model response.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Maximum agentic steps per turn.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Installed tools to register for this session.
    #[serde(default)]
    pub installed_tools: Option<Vec<InstalledTool>>,
    /// Workspace directory path (must be under the server's workspace base).
    #[serde(default)]
    pub workspace: Option<String>,
    /// Absolute path to the real project directory on the host filesystem.
    /// When set, tool execution happens directly in this directory instead of
    /// the sandboxed `aura_data/workspaces/` tree.
    #[serde(default)]
    pub project_path: Option<String>,
    /// JWT auth token for proxy routing.
    #[serde(default)]
    pub token: Option<String>,
    /// Project ID for domain tool calls (specs, tasks, etc.).
    #[serde(default)]
    pub project_id: Option<String>,
    /// Prior conversation messages to restore into session history.
    #[serde(default)]
    pub conversation_messages: Option<Vec<ConversationMessage>>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    #[serde(default)]
    pub aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    #[serde(default)]
    pub aura_session_id: Option<String>,
    /// Organization UUID for X-Aura-Org-Id billing header.
    #[serde(default)]
    pub aura_org_id: Option<String>,
    /// Harness-level agent ID for per-agent skill lookup.
    /// Set by the caller (e.g. aura-os) so the harness can resolve which
    /// skills are installed for this agent.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Payload for `user_message`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct UserMessage {
    pub content: String,
    /// Optional list of tool names the user wants prioritized for this message.
    /// When set, the agent loop will filter tools and set `tool_choice` on the
    /// first iteration to explicitly direct the model toward these tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_hints: Option<Vec<String>>,
    /// Optional image/text attachments (base64-encoded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<MessageAttachment>>,
}

/// A user-supplied attachment (image or text file) sent with a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct MessageAttachment {
    /// `"image"` or `"text"`.
    #[serde(rename = "type")]
    pub type_: String,
    /// MIME type (e.g. `"image/png"`).
    pub media_type: String,
    /// Base64-encoded payload.
    pub data: String,
    /// Optional filename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Payload for `approval_response`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ApprovalResponse {
    pub tool_use_id: String,
    pub approved: bool,
}

// ============================================================================
// Outbound Messages (Server → Client)
// ============================================================================

/// Top-level outbound message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub enum OutboundMessage {
    /// Session initialized and ready.
    SessionReady(SessionReady),
    /// Start of an assistant message.
    AssistantMessageStart(AssistantMessageStart),
    /// Incremental text content from the model.
    TextDelta(TextDelta),
    /// Incremental thinking content from the model.
    ThinkingDelta(ThinkingDelta),
    /// A tool use has started.
    ToolUseStart(ToolUseStart),
    /// Snapshot of a tool call with accumulated input (streamed incrementally).
    ToolCallSnapshot(ToolCallSnapshot),
    /// Result of a tool execution.
    ToolResult(ToolResultMsg),
    /// End of an assistant message (turn complete).
    AssistantMessageEnd(AssistantMessageEnd),
    /// An error occurred.
    Error(ErrorMsg),
}

/// Payload for `session_ready`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SessionReady {
    pub session_id: String,
    pub tools: Vec<ToolInfo>,
    /// Skills that are active (installed + resolved) for this session's agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillInfo>,
}

/// Minimal tool info for the `session_ready` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// Minimal skill info surfaced in `session_ready`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

/// Payload for `assistant_message_start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct AssistantMessageStart {
    pub message_id: String,
}

/// Payload for `text_delta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct TextDelta {
    pub text: String,
}

/// Payload for `thinking_delta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ThinkingDelta {
    pub thinking: String,
}

/// Payload for `tool_use_start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolUseStart {
    pub id: String,
    pub name: String,
}

/// Payload for `tool_call_snapshot` -- incrementally accumulated tool input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolCallSnapshot {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Payload for `tool_result`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolResultMsg {
    pub name: String,
    pub result: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// Payload for `assistant_message_end`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct AssistantMessageEnd {
    pub message_id: String,
    pub stop_reason: String,
    pub usage: SessionUsage,
    pub files_changed: FilesChanged,
}

/// Token usage information for a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
    /// Fraction of the model's context window consumed (0.0–1.0).
    pub context_utilization: f32,
    /// Model identifier used for this turn.
    pub model: String,
    /// Provider name (e.g., "anthropic").
    pub provider: String,
}

/// A single file mutation observed during a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct FileOp {
    pub path: String,
    pub operation: String,
}

/// Summary of file mutations during a turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct FilesChanged {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl FilesChanged {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// Payload for `error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ErrorMsg {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

// ============================================================================
// Installed Tool Types (self-contained, wire-compatible with aura-core)
// ============================================================================

/// Authentication configuration for installed tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
#[derive(Default)]
pub enum ToolAuth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    ApiKey {
        header: String,
        key: String,
    },
    Headers {
        headers: HashMap<String, String>,
    },
}

/// Definition for an installed tool, sent over the wire in `session_init`.
///
/// Wire-compatible with `aura_core::InstalledToolDefinition` but
/// self-contained so this crate has no dependency on `aura-core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct InstalledTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub endpoint: String,
    #[serde(default)]
    pub auth: ToolAuth,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

// ============================================================================
// TypeScript export (behind `typescript` feature)
// ============================================================================

#[cfg(all(test, feature = "typescript"))]
mod ts_export {
    use super::*;

    #[test]
    fn export_typescript_bindings() {
        InboundMessage::export_all().unwrap();
        OutboundMessage::export_all().unwrap();
    }
}
