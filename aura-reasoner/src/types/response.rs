use super::message::Message;
use serde::{Deserialize, Serialize};

// ============================================================================
// Stop Reason
// ============================================================================

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model completed its turn naturally
    #[default]
    EndTurn,
    /// Model wants to use tools
    ToolUse,
    /// Hit the `max_tokens` limit
    MaxTokens,
    /// Hit a stop sequence
    StopSequence,
}

// ============================================================================
// Usage
// ============================================================================

/// Token usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Number of input tokens
    pub input_tokens: u64,
    /// Number of output tokens
    pub output_tokens: u64,
    /// Cache creation input tokens (prompt caching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Cache read input tokens (prompt caching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

impl Usage {
    /// Create new usage information.
    #[must_use]
    pub const fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    /// Total tokens used.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Set cache token counts.
    #[must_use]
    pub const fn with_cache(mut self, creation: Option<u64>, read: Option<u64>) -> Self {
        self.cache_creation_input_tokens = creation;
        self.cache_read_input_tokens = read;
        self
    }
}

// ============================================================================
// Provider Trace
// ============================================================================

/// Provider trace for debugging/logging.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderTrace {
    /// Request ID from the provider
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Latency in milliseconds
    pub latency_ms: u64,
    /// Model that was used
    pub model: String,
}

impl ProviderTrace {
    /// Create a new provider trace.
    #[must_use]
    pub fn new(model: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            request_id: None,
            latency_ms,
            model: model.into(),
        }
    }

    /// Set the request ID.
    #[must_use]
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }
}

// ============================================================================
// Model Response
// ============================================================================

/// Response from the model.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    /// Why the model stopped
    pub stop_reason: StopReason,
    /// The assistant message
    pub message: Message,
    /// Token usage
    pub usage: Usage,
    /// Provider trace information
    pub trace: ProviderTrace,
    /// Which model actually served the request (relevant after fallback).
    pub model_used: String,
}

impl ModelResponse {
    /// Create a new model response.
    #[must_use]
    pub fn new(
        stop_reason: StopReason,
        message: Message,
        usage: Usage,
        trace: ProviderTrace,
    ) -> Self {
        let model_used = trace.model.clone();
        Self {
            stop_reason,
            message,
            usage,
            trace,
            model_used,
        }
    }

    /// Check if the model wants to use tools.
    #[must_use]
    pub fn wants_tool_use(&self) -> bool {
        self.stop_reason == StopReason::ToolUse
    }

    /// Check if the turn is complete.
    #[must_use]
    pub fn is_end_turn(&self) -> bool {
        self.stop_reason == StopReason::EndTurn
    }
}
