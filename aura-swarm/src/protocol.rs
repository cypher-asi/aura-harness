//! WebSocket session protocol message types.
//!
//! Defines the inbound (client → server) and outbound (server → client)
//! message format for the `/stream` WebSocket endpoint.

use aura_core::ExternalToolDefinition;
use aura_reasoner::ToolDefinition;
use serde::{Deserialize, Serialize};

// ============================================================================
// Inbound Messages (Client → Server)
// ============================================================================

/// Top-level inbound message envelope.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboundMessage {
    /// Initialize the session (must be the first message).
    SessionInit(SessionInit),
    /// Send a user message for processing.
    UserMessage(UserMessage),
    /// Cancel the current turn.
    Cancel,
    /// Respond to an approval request.
    ApprovalResponse(ApprovalResponse),
}

/// Payload for `session_init`.
#[derive(Debug, Deserialize)]
pub struct SessionInit {
    /// Override the default system prompt.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Model identifier (e.g., "claude-sonnet-4-20250514").
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
    /// External tools to register for this session.
    #[serde(default)]
    pub external_tools: Option<Vec<ExternalToolDefinition>>,
    /// Workspace directory path.
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Payload for `user_message`.
#[derive(Debug, Deserialize)]
pub struct UserMessage {
    /// The user's message text.
    pub content: String,
}

/// Payload for `approval_response`.
#[derive(Debug, Deserialize)]
pub struct ApprovalResponse {
    /// ID of the tool use being approved/denied.
    pub tool_use_id: String,
    /// Whether the tool use is approved.
    pub approved: bool,
}

// ============================================================================
// Outbound Messages (Server → Client)
// ============================================================================

/// Top-level outbound message envelope.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    /// Result of a tool execution.
    ToolResult(ToolResultMsg),
    /// End of an assistant message (turn complete).
    AssistantMessageEnd(AssistantMessageEnd),
    /// An error occurred.
    Error(ErrorMsg),
}

/// Payload for `session_ready`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionReady {
    /// Unique session identifier.
    pub session_id: String,
    /// Tools available in this session.
    pub tools: Vec<ToolInfo>,
}

/// Minimal tool info for the session_ready response.
#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

impl From<ToolDefinition> for ToolInfo {
    fn from(td: ToolDefinition) -> Self {
        Self {
            name: td.name,
            description: td.description,
        }
    }
}

/// Payload for `assistant_message_start`.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessageStart {
    pub message_id: String,
}

/// Payload for `text_delta`.
#[derive(Debug, Clone, Serialize)]
pub struct TextDelta {
    pub text: String,
}

/// Payload for `thinking_delta`.
#[derive(Debug, Clone, Serialize)]
pub struct ThinkingDelta {
    pub thinking: String,
}

/// Payload for `tool_use_start`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolUseStart {
    pub id: String,
    pub name: String,
}

/// Payload for `tool_result`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResultMsg {
    pub name: String,
    pub result: String,
    pub is_error: bool,
}

/// Payload for `assistant_message_end`.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessageEnd {
    pub message_id: String,
    pub stop_reason: String,
    pub usage: SessionUsage,
}

/// Token usage information for a session.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
}

/// Payload for `error`.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorMsg {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}
