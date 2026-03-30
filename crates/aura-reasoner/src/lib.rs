//! # aura-reasoner
//!
//! Provider-agnostic model interface for Aura.
//!
//! This crate provides:
//! - Normalized conversation types (`Message`, `ContentBlock`, `ToolDefinition`)
//! - `ModelProvider` trait for provider-agnostic completions
//! - `AnthropicProvider` implementation using `reqwest` for HTTP communication with the Anthropic API
//! - `MockProvider` for testing
//!
//! ## Architecture
//!
//! The reasoner abstraction separates AURA's deterministic kernel from
//! probabilistic model calls. All model interactions go through the
//! `ModelProvider` trait, enabling:
//!
//! - Provider switching (Anthropic, `OpenAI`, local models)
//! - Recording/replay of model outputs for determinism
//! - Testing with mock providers

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod anthropic;
mod error;
mod mock;
mod request;
mod retry;
pub mod types;

pub(crate) fn truncate_body(body: &str, max_len: usize) -> String {
    if body.len() <= max_len {
        body.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &body[..end])
    }
}

pub use anthropic::{AnthropicConfig, AnthropicProvider, RoutingMode};
pub use error::ReasonerError;
pub use mock::{MockProvider, MockResponse};
pub use request::{ProposeLimits, ProposeRequest, RecordSummary};
pub use types::{
    AccumulatedToolUse, CacheControl, ContentBlock, ImageSource, Message, ModelRequest,
    ModelResponse, ProviderTrace, Role, StopReason, StreamAccumulator, StreamContentType,
    StreamEvent, ThinkingConfig, ToolChoice, ToolDefinition, ToolResultContent, Usage,
};

use futures_util::Stream;
use std::pin::Pin;

use async_trait::async_trait;

// ============================================================================
// ModelProvider Trait (New in Spec-02)
// ============================================================================

/// Type alias for a boxed stream of streaming events.
pub type StreamEventStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send + 'static>>;

/// Provider-agnostic interface for model completions.
///
/// This trait abstracts over different LLM providers (Anthropic, `OpenAI`, etc.)
/// allowing the kernel to work with any provider that implements this interface.
///
/// # Recording and Replay
///
/// During normal operation, the kernel calls `complete()` and records the
/// `ModelResponse`. During replay, the kernel loads the recorded response
/// instead of calling `complete()`, ensuring deterministic state reconstruction.
///
/// # Tool Use
///
/// When the model wants to use tools, it returns with `StopReason::ToolUse`.
/// The kernel extracts tool calls from the response message, executes them,
/// and continues the conversation with tool results.
///
/// # Streaming
///
/// For real-time output, use `complete_streaming()` which returns a stream
/// of `StreamEvent`s. This allows displaying text as it's generated.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider name (e.g., "anthropic", "openai", "mock").
    fn name(&self) -> &'static str;

    /// Complete a conversation, potentially with tool use.
    ///
    /// # Arguments
    ///
    /// * `request` - The model request containing system prompt, messages, and tools
    ///
    /// # Returns
    ///
    /// * `Ok(ModelResponse)` - The model's response with stop reason and content
    /// * `Err(_)` - If the request fails (network, auth, rate limit, etc.)
    ///
    /// # Errors
    ///
    /// Returns error if the provider request fails.
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError>;

    /// Complete a conversation with streaming output.
    ///
    /// Returns a stream of `StreamEvent`s that can be processed in real-time.
    /// Use `StreamAccumulator` to collect events into a final `ModelResponse`.
    ///
    /// # Arguments
    ///
    /// * `request` - The model request containing system prompt, messages, and tools
    ///
    /// # Returns
    ///
    /// A stream of events. The stream ends with either `MessageStop` or `Error`.
    ///
    /// # Default Implementation
    ///
    /// Falls back to non-streaming `complete()` if not overridden.
    ///
    /// # Errors
    ///
    /// Returns error if the provider request fails.
    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        // Default: fall back to non-streaming
        let response = self.complete(request).await?;

        // Convert response to stream events
        let events = vec![
            Ok(StreamEvent::MessageStart {
                message_id: response.trace.request_id.clone().unwrap_or_default(),
                model: response.trace.model.clone(),
                input_tokens: Some(response.usage.input_tokens),
                cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
                cache_read_input_tokens: response.usage.cache_read_input_tokens,
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamContentType::Text,
            }),
            Ok(StreamEvent::TextDelta {
                text: response.message.text_content(),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(response.stop_reason),
                output_tokens: response.usage.output_tokens,
            }),
            Ok(StreamEvent::MessageStop),
        ];

        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    /// Check if the provider is available.
    ///
    /// This can be used for health checks and load balancing.
    async fn health_check(&self) -> bool;
}
