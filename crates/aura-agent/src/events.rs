//! Unified streaming events emitted during agent execution.
//!
//! `TurnEvent` is the single event type for both `AgentLoop` and
//! `TurnProcessor`. Consumers subscribe by passing an
//! `mpsc::UnboundedSender<TurnEvent>` to the orchestrator.

/// Unified events emitted during agent/turn execution.
///
/// Covers all events previously split between `AgentLoopEvent` and
/// `StreamCallbackEvent`.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Incremental text content from the model.
    TextDelta(String),

    /// Incremental thinking/reasoning content from the model.
    ThinkingDelta(String),

    /// Thinking block completed (end of extended-thinking content).
    ThinkingComplete,

    /// A tool use block started streaming.
    ToolStart {
        /// Tool use ID from the model.
        id: String,
        /// Tool name.
        name: String,
    },

    /// Incremental snapshot of tool input JSON as it streams in.
    ToolInputSnapshot {
        /// Tool use ID.
        id: String,
        /// Tool name.
        name: String,
        /// Accumulated input JSON so far (may be partial/incomplete).
        input: String,
    },

    /// A tool execution completed (with full result).
    ToolComplete {
        /// Tool name.
        name: String,
        /// Tool arguments (JSON), if available.
        args: Option<serde_json::Value>,
        /// Result content (text).
        result: String,
        /// Whether the tool execution failed.
        is_error: bool,
    },

    /// Tool result that will be appended to context.
    ToolResult {
        /// Tool use ID.
        tool_use_id: String,
        /// Tool name.
        tool_name: String,
        /// Result content.
        content: String,
        /// Whether the result is an error.
        is_error: bool,
    },

    /// One iteration (model call + tool execution) completed.
    IterationComplete {
        /// Zero-based iteration index.
        iteration: usize,
        /// Input tokens used in this iteration.
        input_tokens: u64,
        /// Output tokens used in this iteration.
        output_tokens: u64,
    },

    /// Streaming is complete for the current step.
    StepComplete,

    /// A warning was injected into the context.
    Warning(String),

    /// An error occurred during execution.
    Error {
        /// Machine-readable error code.
        code: String,
        /// Human-readable description.
        message: String,
        /// Whether the loop can continue after this error.
        recoverable: bool,
    },
}

/// Backward-compatible alias for `TurnEvent`.
pub type AgentLoopEvent = TurnEvent;
