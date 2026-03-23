//! Shared type definitions for the turn processor.

use aura_reasoner::{ModelResponse, StopReason, ToolResultContent};
use std::collections::HashMap;

/// Per-turn cache for tool results.
///
/// Keyed by `"tool_name\0canonical_args_json"`. Only populated for
/// read-only tools (`fs_ls`, `fs_read`, `fs_stat`, `fs_find`, `search_code`)
/// to avoid suppressing side-effectful calls.
pub type ToolCache = HashMap<String, ExecutedToolCall>;

/// Result of processing a single step (one model call + tool execution).
#[derive(Debug)]
pub struct StepResult {
    /// The model's response for this step.
    pub response: ModelResponse,
    /// Tool calls that were executed during this step.
    pub executed_tools: Vec<ExecutedToolCall>,
    /// Why the model stopped generating.
    pub stop_reason: StopReason,
    /// Whether any tool executions failed.
    pub had_failures: bool,
}

/// Result of processing a turn.
#[derive(Debug)]
pub struct TurnResult {
    /// Record entries created during the turn
    pub entries: Vec<TurnEntry>,
    /// Final assistant message
    pub final_message: Option<aura_reasoner::Message>,
    /// Total tokens used
    pub total_input_tokens: u64,
    /// Total output tokens
    pub total_output_tokens: u64,
    /// Number of steps taken
    pub steps: u32,
    /// Whether any tools failed
    pub had_failures: bool,
    /// Whether the turn was cancelled.
    pub cancelled: bool,
    /// Model identifier used for this turn.
    pub model: String,
    /// Provider name (e.g., "anthropic").
    pub provider: String,
}

/// Information about an executed tool call.
#[derive(Debug, Clone)]
pub struct ExecutedToolCall {
    /// Tool use ID from the model
    pub tool_use_id: String,
    /// Tool name
    pub tool_name: String,
    /// Tool arguments (JSON)
    pub tool_args: serde_json::Value,
    /// Tool result
    pub result: ToolResultContent,
    /// Whether the tool failed
    pub is_error: bool,
    /// Metadata from the tool result (e.g. `file_existed`, `bytes_written`).
    pub metadata: HashMap<String, String>,
}

/// A single step entry in a turn.
#[derive(Debug, Clone)]
pub struct TurnEntry {
    /// Step number within the turn (0-indexed)
    pub turn_step: u32,
    /// Model response for this step
    pub model_response: ModelResponse,
    /// Tool results from this step (if any) - legacy format for backwards compatibility
    pub tool_results: Vec<(String, ToolResultContent, bool)>,
    /// Executed tool calls with full information
    pub executed_tools: Vec<ExecutedToolCall>,
    /// Stop reason for this step
    pub stop_reason: StopReason,
}
