use super::response::StopReason;

mod accumulator;

pub use accumulator::{AccumulatedToolUse, StreamAccumulator};

/// A streaming event from the model provider.
///
/// These events are emitted during streaming completions, allowing
/// real-time display of model output as it's generated.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Start of a new message
    MessageStart {
        /// Message ID from the provider
        message_id: String,
        /// Model being used
        model: String,
        /// Input tokens (from SSE `message_start` usage)
        input_tokens: Option<u64>,
        /// Cache creation input tokens (prompt caching)
        cache_creation_input_tokens: Option<u64>,
        /// Cache read input tokens (prompt caching)
        cache_read_input_tokens: Option<u64>,
    },

    /// Start of a new content block
    ContentBlockStart {
        /// Index of the content block
        index: u32,
        /// Type of content block (text, `tool_use`, thinking)
        content_type: StreamContentType,
    },

    /// Text delta (incremental text)
    TextDelta {
        /// The text chunk
        text: String,
    },

    /// Thinking delta (incremental thinking content)
    ThinkingDelta {
        /// The thinking text chunk
        thinking: String,
    },

    /// Signature delta (for thinking block signatures)
    SignatureDelta {
        /// The signature chunk
        signature: String,
    },

    /// Tool use input delta (incremental JSON)
    InputJsonDelta {
        /// Partial JSON string
        partial_json: String,
    },

    /// End of a content block
    ContentBlockStop {
        /// Index of the content block
        index: u32,
    },

    /// Final message delta with stop reason
    MessageDelta {
        /// Why the model stopped
        stop_reason: Option<StopReason>,
        /// Output tokens used so far
        output_tokens: u64,
        /// Input-token total from the delta's usage, when reported.
        ///
        /// Anthropic repeats the cumulative `message_start` total here;
        /// OpenRouter's Anthropic-compatible endpoint sends the only
        /// real total here for non-Anthropic upstreams.
        input_tokens: Option<u64>,
        /// Cache creation input tokens from the delta's usage, when reported.
        cache_creation_input_tokens: Option<u64>,
        /// Cache read input tokens from the delta's usage, when reported.
        cache_read_input_tokens: Option<u64>,
    },

    /// Message complete
    MessageStop,

    /// Ping event (keepalive)
    Ping,

    /// Error event
    Error {
        /// Error message
        message: String,
        /// Optional provider / proxy-supplied HTTP request id, pulled
        /// from the SSE error body (some proxies embed
        /// `error.request_id`). This is a fallback; the primary source
        /// of truth is the synthetic [`StreamEvent::HttpMeta`] event
        /// which carries the response-header `x-request-id`.
        #[doc(hidden)]
        request_id: Option<String>,
    },

    /// Synthetic event emitted by the SSE transport on stream open,
    /// before any provider-level events. Carries HTTP-level metadata
    /// (specifically the `x-request-id` response header) that the
    /// provider never emits inside the SSE body and that a failing
    /// stream would otherwise drop. Not part of the Anthropic / OpenAI
    /// wire protocol — consumers that only care about model output
    /// can safely ignore it.
    HttpMeta {
        /// The `x-request-id` (or `request-id`) header value captured
        /// from the streaming response, if the upstream provided one.
        request_id: Option<String>,
        /// Usage of the plan the call ran on, read from the response
        /// headers ([`crate::plan_usage`]); empty when there was none.
        plan_usage: Vec<aura_core_types::PlanUsage>,
    },
}

/// Type of content in a streaming block.
#[derive(Debug, Clone)]
pub enum StreamContentType {
    /// Text content
    Text,
    /// Thinking content (extended thinking)
    Thinking,
    /// Tool use block
    ToolUse {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
    },
}

/// Snapshot of an in-flight `tool_use` block at the moment a mid-stream
/// error truncated the response.
///
/// Returned inside [`crate::error::ReasonerError::StreamAbortedWithPartial`]
/// so the agent loop can surface which tool call was interrupted (and on
/// what partial input) when it decides to retry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PartialToolUse {
    /// Provider-side `tool_use` id (e.g. `toolu_01...`). Empty when the
    /// stream died before `content_block_start` landed.
    pub tool_use_id: String,
    /// Tool name (`write_file`, `edit_file`, ...). Empty when unknown.
    pub tool_name: String,
    /// Accumulated tool-input JSON so far. May be empty or truncated
    /// (not parseable on its own).
    pub partial_json: String,
}
