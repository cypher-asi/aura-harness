//! Streaming events emitted by `AgentLoop` during execution.
//!
//! Consumers subscribe by passing an `mpsc::UnboundedSender<AgentLoopEvent>`
//! to `AgentLoop::run()`. Events are emitted in real-time as the loop
//! progresses through model calls, tool executions, and loop-level decisions.

/// Events emitted by the agent loop during execution.
#[derive(Debug, Clone)]
pub enum AgentLoopEvent {
    /// Incremental text content from the model.
    TextDelta(String),

    /// Incremental thinking/reasoning content from the model.
    ThinkingDelta(String),

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

    /// A tool execution completed.
    ToolComplete {
        /// Tool name.
        name: String,
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

    /// A warning was injected into the context.
    Warning(String),

    /// An error occurred during execution.
    Error {
        /// Machine-readable error code (e.g. "rate_limit", "timeout", "llm_error").
        code: String,
        /// Human-readable description.
        message: String,
        /// Whether the loop can continue after this error.
        recoverable: bool,
    },
}
