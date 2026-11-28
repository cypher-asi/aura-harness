//! Outbound (server → client) wire messages and their payloads.
//!
//! [`OutboundMessage`] is the top-level enum streamed from the harness to a
//! websocket client. It covers session-level events (ready / start / end),
//! incremental text and tool deltas, tool-result and tool-approval prompts,
//! errors, and image / video / 3D generation events.

use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

use crate::common::ToolStateWire;

mod generation;
mod policy;
mod subagent;
#[cfg(test)]
mod tests;
mod turn_end;

pub use generation::{
    GenerationCompleted, GenerationErrorMsg, GenerationPartialImage, GenerationProgressMsg,
    GenerationStart,
};
pub use policy::{PolicyCheckWire, PolicyRuleWire, ToolApprovalPrompt, ToolPolicyDecision};
pub use subagent::{SubagentSpawned, SubagentStatus};
pub use turn_end::{
    AssistantMessageEnd, ContextBreakdown, FileDiff, FileOp, FilesChanged, SessionUsage,
};

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
    /// Ask the client to approve or deny a live tool call.
    ToolApprovalPrompt(ToolApprovalPrompt),
    /// The kernel recorded its policy decision for a tool call. Sent
    /// after the decision is persisted and before the tool runs.
    ToolPolicyDecision(ToolPolicyDecision),
    /// End of an assistant message (turn complete).
    ///
    /// Boxed because [`AssistantMessageEnd`] carries a full
    /// [`SessionUsage`] (including [`ContextBreakdown`]) plus a
    /// [`FilesChanged`] payload. Holding it inline would make every
    /// other variant 360+ bytes wide via Rust's union layout, even
    /// though most (`TextDelta`, `ToolUseStart`, `ProgressMsg`) are
    /// far smaller. Boxing isolates the cost on the rarer terminal
    /// frame.
    AssistantMessageEnd(Box<AssistantMessageEnd>),
    /// An error occurred.
    Error(ErrorMsg),
    /// Progress heartbeat. Strictly additive: the harness emits these
    /// during long tool calls (`stage = "tool_running"`, every
    /// `AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS`) so the aura-os
    /// sliding-idle watchdog and the client-side stuck-stream
    /// watchdog see forward motion and don't trip a `turn_timeout` on
    /// a turn that is actually working. Older clients ignore unknown
    /// SSE event types (the chat handler already does — see
    /// `interface/src/hooks/use-chat-stream/build-stream-handler.ts`
    /// `EventType.Progress` branch) so adding the variant is
    /// wire-compatible in both directions.
    Progress(ProgressMsg),
    /// Generation started.
    GenerationStart(GenerationStart),
    /// Generation progress update.
    GenerationProgress(GenerationProgressMsg),
    /// Partial image data (progressive rendering).
    GenerationPartialImage(GenerationPartialImage),
    /// Generation completed successfully.
    GenerationCompleted(GenerationCompleted),
    /// Generation failed.
    GenerationError(GenerationErrorMsg),
    /// A subagent (`task` tool) child run has been spawned and is now
    /// observable as its own live thread. Emitted on the PARENT stream
    /// before the blocking `task` tool returns, so a client can render
    /// a clickable thread card and lazily attach to `child_run_id`.
    SubagentSpawned(SubagentSpawned),
    /// Terminal (or transitional) status update for a previously
    /// announced subagent child run. Emitted on the parent stream so
    /// the thread card reflects running/completed/failed/etc.
    SubagentStatus(SubagentStatus),
    /// A title for the chat, written from the turn's prompt by the
    /// model the request named as
    /// [`crate::ModelSelection::title_model`]. Sent before the turn's
    /// `assistant_message_end`, and only when the model gave one.
    ChatTitle(ChatTitle),
    /// One model call of the turn ended, with the tool calls it made.
    /// Text after it is the next call's, a reply of its own.
    IterationComplete(IterationComplete),
}

/// Payload for `chat_title`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ChatTitle {
    pub title: String,
}

/// Payload for `iteration_complete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct IterationComplete {
    /// The model call's index in the turn, from 0.
    pub iteration: u64,
    /// Input tokens the call used.
    pub input_tokens: u64,
    /// Output tokens the call wrote.
    pub output_tokens: u64,
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
    #[serde(default = "default_tool_state_on")]
    pub effective_state: ToolStateWire,
}

const fn default_tool_state_on() -> ToolStateWire {
    ToolStateWire::On
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
///
/// `result` carries the textual tool output (the string-only contract
/// used by every existing tool). Computer-use / vision tools may
/// additionally attach a single rendered frame via the optional
/// `image_base64` + `image_media_type` pair: the harness (the
/// producer of this message) base64-encodes a PNG/JPEG screenshot and
/// stamps its media type so the server can persist it and replay it to
/// the model as an Anthropic `image` content block inside the
/// `tool_result`.
///
/// Both image fields are strictly additive and backward compatible:
/// `#[serde(default)]` lets older producers omit them on the wire, and
/// `skip_serializing_if = "Option::is_none"` keeps the JSON
/// byte-identical to today's shape when no image is present. A message
/// with neither field behaves exactly as before — a string result.
///
/// Never log the base64 payload; log `image_media_type` and the
/// encoded byte length only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolResultMsg {
    pub name: String,
    pub result: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Base64-encoded PNG/JPEG payload of an image tool-result (e.g. a
    /// computer-use screenshot). `None` for the ordinary string-only
    /// path. Pairs with [`Self::image_media_type`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    /// IANA media type of [`Self::image_base64`] (e.g. `"image/png"`).
    /// `None` when no image is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_media_type: Option<String>,
}

/// Payload for `error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ErrorMsg {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
    /// Short opaque id (12 lowercase hex chars) used to correlate this
    /// error across server logs, client breadcrumbs, and user-pasted
    /// support reports. Strictly additive on the wire:
    ///
    /// - Older clients that don't know the field deserialize fine
    ///   (`#[serde(default)]` keeps the field optional inbound), and
    ///   the sender omits it from the JSON when `None`
    ///   (`skip_serializing_if = "Option::is_none"`) so older receivers
    ///   never see an unexpected key.
    /// - Older harness builds simply leave it `None`. The aura-os SSE
    ///   remap boundary in `apps/aura-os-server/src/handlers/agents/chat/errors.rs`
    ///   keeps stamping a `(support_id=<id>)` suffix into `message`
    ///   for that case so existing clients still get a usable id.
    /// - Newer harness in-process emit sites (e.g. the watchdog
    ///   `stream_stalled` / `turn_timeout` synth, the agent loop's
    ///   `agent_stalled` terminal event) can pre-populate the field so
    ///   the same id appears on both the structured field and the
    ///   message suffix; the SSE remap path leaves a prepopulated id
    ///   untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_id: Option<String>,
}

/// Payload for `progress` heartbeat / status events.
///
/// `stage` is the only required field and carries a free-form short
/// label (`"tool_running"`, `"lagged"`, `"forked_for_context"`, …).
/// Unknown stages flow straight through to the client which renders
/// them as a generic progress label, so we can introduce new stages
/// without coordinating a wire bump.
///
/// Optional fields are omitted from the JSON when `None`
/// (`skip_serializing_if = "Option::is_none"`) and default to `None`
/// inbound (`#[serde(default)]`) so older harness/client pairs that
/// don't know about them deserialize cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ProgressMsg {
    /// Short machine-readable stage tag. The aura-os chat client
    /// renders unknown values as the literal label, so adding new
    /// stages does not require a coordinated client release.
    pub stage: String,
    /// Tool whose long-running execution is producing this heartbeat.
    /// Set on `stage = "tool_running"`; left `None` for stages that
    /// don't refer to a single tool (e.g. `"lagged"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Wall-clock milliseconds since the heartbeat's reference event
    /// (tool start for `"tool_running"`). Optional — older clients
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// Optional human-readable label / detail string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// UTF-8 suffix lengths to discard for `stage = "stream_reset"`.
    /// Only the interrupted sampling attempt is rolled back, not the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_text_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_thinking_bytes: Option<u64>,
}
