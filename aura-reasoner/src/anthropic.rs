//! Anthropic provider implementation.
//!
//! Uses `reqwest` directly to communicate with the Anthropic API.
//! This approach is more reliable than SDK wrappers which may have
//! incompatible or private internal types.
//!
//! Supports both synchronous and streaming completions via SSE.

use crate::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace, Role,
    StopReason, StreamContentType, StreamEvent, StreamEventStream, ToolChoice, ToolDefinition,
    ToolResultContent, Usage,
};
use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;
use tracing::{debug, error, instrument, trace};

// ============================================================================
// Configuration
// ============================================================================

/// Anthropic provider configuration.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// API key
    pub api_key: String,
    /// Default model to use
    pub default_model: String,
    /// Request timeout in milliseconds
    pub timeout_ms: u64,
    /// Maximum retries
    pub max_retries: u32,
    /// API base URL
    pub base_url: String,
}

impl AnthropicConfig {
    /// Create a new config from environment variables.
    ///
    /// Reads:
    /// - `AURA_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY`
    /// - `AURA_ANTHROPIC_MODEL` (defaults to "claude-sonnet-4-20250514")
    ///
    /// # Errors
    ///
    /// Returns error if API key is not set.
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = std::env::var("AURA_ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .map_err(|_| anyhow::anyhow!("AURA_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY not set"))?;

        let default_model = std::env::var("AURA_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

        let timeout_ms = std::env::var("AURA_MODEL_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60_000);

        let base_url = std::env::var("AURA_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        Ok(Self {
            api_key,
            default_model,
            timeout_ms,
            max_retries: 2,
            base_url,
        })
    }

    /// Create a config with explicit values.
    #[must_use]
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            default_model: model.into(),
            timeout_ms: 60_000,
            max_retries: 2,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Anthropic model provider.
///
/// Implements `ModelProvider` for the Anthropic API using direct HTTP calls.
pub struct AnthropicProvider {
    client: reqwest::Client,
    config: AnthropicConfig,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    ///
    /// # Errors
    ///
    /// Returns error if client creation fails.
    pub fn new(config: AnthropicConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self { client, config })
    }

    /// Create from environment variables.
    ///
    /// # Errors
    ///
    /// Returns error if configuration or client creation fails.
    pub fn from_env() -> anyhow::Result<Self> {
        let config = AnthropicConfig::from_env()?;
        Self::new(config)
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let start = Instant::now();

        // Build the API request
        let api_request = ApiRequest {
            model: request.model.clone(),
            system: request.system.clone(),
            messages: convert_messages_to_api(&request.messages),
            tools: if request.tools.is_empty() {
                None
            } else {
                Some(convert_tools_to_api(&request.tools))
            },
            tool_choice: convert_tool_choice(&request.tool_choice),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
        };

        debug!(
            messages = api_request.messages.len(),
            tools = api_request.tools.as_ref().map_or(0, Vec::len),
            "Sending request to Anthropic"
        );

        // Make the API call
        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Anthropic API request failed");
                anyhow::anyhow!("Anthropic API request failed: {e}")
            })?;

        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Check for HTTP errors
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Anthropic API error");
            anyhow::bail!("Anthropic API error: {status} - {body}");
        }

        // Parse response
        let api_response: ApiResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Anthropic response");
            anyhow::anyhow!("Failed to parse Anthropic response: {e}")
        })?;

        // Convert response to AURA types
        let message = convert_response_to_aura(&api_response.content);
        let stop_reason = match api_response.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        };

        debug!(
            stop_reason = ?stop_reason,
            latency_ms = latency_ms,
            input_tokens = api_response.usage.input_tokens,
            output_tokens = api_response.usage.output_tokens,
            "Received response from Anthropic"
        );

        Ok(ModelResponse {
            stop_reason,
            message,
            usage: Usage {
                input_tokens: api_response.usage.input_tokens,
                output_tokens: api_response.usage.output_tokens,
            },
            trace: ProviderTrace {
                request_id: Some(api_response.id),
                latency_ms,
                model: api_response.model,
            },
        })
    }

    async fn health_check(&self) -> bool {
        // Simple health check - could be improved with a lightweight API call
        true
    }

    #[instrument(skip(self, request), fields(model = %request.model))]
    async fn complete_streaming(&self, request: ModelRequest) -> anyhow::Result<StreamEventStream> {
        // Check if the model supports extended thinking (claude-3-7 and above, or opus-4, sonnet-4)
        let supports_thinking = request.model.contains("claude-3-7")
            || request.model.contains("claude-opus-4")
            || request.model.contains("claude-sonnet-4");

        // Configure thinking if supported - budget_tokens MUST be less than max_tokens
        // and at least 1024. We need max_tokens > budget_tokens + some room for output.
        let thinking = if supports_thinking && request.max_tokens > 2048 {
            // Use half of max_tokens for thinking, clamped to valid range
            let budget = (request.max_tokens / 2).clamp(1024, 16000);
            Some(ThinkingConfig {
                thinking_type: "enabled".to_string(),
                budget_tokens: budget,
            })
        } else {
            None
        };

        // Build the API request with streaming enabled
        let api_request = StreamingApiRequest {
            model: request.model.clone(),
            system: request.system.clone(),
            messages: convert_messages_to_api(&request.messages),
            tools: if request.tools.is_empty() {
                None
            } else {
                Some(convert_tools_to_api(&request.tools))
            },
            tool_choice: convert_tool_choice(&request.tool_choice),
            max_tokens: request.max_tokens,
            // Temperature must be 1 when using extended thinking
            temperature: if thinking.is_some() { Some(1.0) } else { request.temperature },
            stream: true,
            thinking,
        };

        debug!(
            messages = api_request.messages.len(),
            tools = api_request.tools.as_ref().map_or(0, Vec::len),
            "Sending streaming request to Anthropic"
        );

        // Make the streaming API call
        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Anthropic streaming API request failed");
                anyhow::anyhow!("Anthropic streaming API request failed: {e}")
            })?;

        // Check for HTTP errors
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Anthropic streaming API error");
            anyhow::bail!("Anthropic streaming API error: {status} - {body}");
        }

        // Create the SSE stream
        let byte_stream = response.bytes_stream();
        let sse_stream = SseStream::new(byte_stream);

        Ok(Box::pin(sse_stream))
    }
}

// ============================================================================
// API Types (matching Anthropic's JSON schema)
// ============================================================================

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    system: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ApiToolChoice>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiContent {
    Text {
        text: String,
    },
    /// Thinking content block - required when extended thinking is enabled.
    /// Must be echoed back to the API in multi-turn conversations.
    Thinking {
        thinking: String,
        /// Signature is required when echoing thinking blocks back to the API
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    id: String,
    model: String,
    content: Vec<ApiContent>,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ============================================================================
// Streaming API Types
// ============================================================================

/// Request with streaming enabled.
#[derive(Debug, Serialize)]
struct StreamingApiRequest {
    model: String,
    system: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ApiToolChoice>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    /// Extended thinking configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

/// Configuration for extended thinking.
#[derive(Debug, Serialize)]
struct ThinkingConfig {
    /// Type of thinking (always "enabled")
    #[serde(rename = "type")]
    thinking_type: String,
    /// Budget tokens for thinking (between 1024 and `max_tokens`)
    budget_tokens: u32,
}

/// SSE event types from Anthropic.
///
/// These types are used for deserializing SSE events from the Anthropic API.
/// Some fields are parsed but not directly used (they're used for proper deserialization).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum SseEvent {
    MessageStart {
        message: SseMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: SseContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: SseDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: SseMessageDeltaContent,
        usage: Option<SseUsageDelta>,
    },
    MessageStop,
    Ping,
    Error {
        error: SseError,
    },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SseMessageStart {
    id: String,
    model: String,
    #[serde(default)]
    usage: Option<SseUsageStart>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SseUsageStart {
    input_tokens: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum SseContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SseDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct SseMessageDeltaContent {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseUsageDelta {
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct SseError {
    message: String,
}

// ============================================================================
// SSE Stream Implementation
// ============================================================================

/// A stream that parses SSE events from an HTTP byte stream.
struct SseStream<S> {
    inner: S,
    buffer: String,
    finished: bool,
}

impl<S> SseStream<S> {
    const fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: String::new(),
            finished: false,
        }
    }
}

impl<S, E> Stream for SseStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = anyhow::Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        loop {
            // Try to parse a complete event from the buffer
            if let Some(event) = self.parse_next_event() {
                // Check for terminal events
                if matches!(event, StreamEvent::MessageStop | StreamEvent::Error { .. }) {
                    self.finished = true;
                }
                return Poll::Ready(Some(Ok(event)));
            }

            // Need more data
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(s);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {e}"))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> SseStream<S> {
    /// Try to parse the next complete SSE event from the buffer.
    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        // SSE format: "event: <type>\ndata: <json>\n\n"
        // Find a complete event (ends with \n\n or \r\n\r\n)
        let event_end = self
            .buffer
            .find("\n\n")
            .or_else(|| self.buffer.find("\r\n\r\n"));

        let event_end = event_end?;
        let event_str = self.buffer[..event_end].to_string();
        
        // Remove the event from buffer (including the delimiter)
        let delimiter_len = if self.buffer[event_end..].starts_with("\r\n\r\n") {
            4
        } else {
            2
        };
        self.buffer = self.buffer[event_end + delimiter_len..].to_string();

        // Parse the event
        parse_sse_event(&event_str)
    }
}

/// Parse an SSE event string into a `StreamEvent`.
fn parse_sse_event(event_str: &str) -> Option<StreamEvent> {
    let mut event_type = None;
    let mut data = None;

    for line in event_str.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(suffix) = line.strip_prefix("event:") {
            event_type = Some(suffix.trim().to_string());
        } else if let Some(suffix) = line.strip_prefix("data:") {
            data = Some(suffix.trim().to_string());
        }
    }

    let data = data?;

    // Handle ping events specially (they don't have JSON data)
    if event_type.as_deref() == Some("ping") {
        return Some(StreamEvent::Ping);
    }

    // Parse the JSON data
    let sse_event: SseEvent = match serde_json::from_str(&data) {
        Ok(e) => e,
        Err(e) => {
            trace!(data = %data, error = %e, "Failed to parse SSE event");
            return None;
        }
    };

    // Convert to our StreamEvent type
    match sse_event {
        SseEvent::MessageStart { message } => Some(StreamEvent::MessageStart {
            message_id: message.id,
            model: message.model,
        }),
        SseEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            let content_type = match content_block {
                SseContentBlock::Text { .. } => StreamContentType::Text,
                SseContentBlock::Thinking { .. } => StreamContentType::Thinking,
                SseContentBlock::ToolUse { id, name } => StreamContentType::ToolUse { id, name },
            };
            Some(StreamEvent::ContentBlockStart {
                index,
                content_type,
            })
        }
        SseEvent::ContentBlockDelta { delta, .. } => match delta {
            SseDelta::Text { text } => Some(StreamEvent::TextDelta { text }),
            SseDelta::Thinking { thinking } => Some(StreamEvent::ThinkingDelta { thinking }),
            SseDelta::Signature { signature } => Some(StreamEvent::SignatureDelta { signature }),
            SseDelta::InputJson { partial_json } => {
                Some(StreamEvent::InputJsonDelta { partial_json })
            }
        },
        SseEvent::ContentBlockStop { index } => Some(StreamEvent::ContentBlockStop { index }),
        SseEvent::MessageDelta { delta, usage } => {
            let stop_reason = delta.stop_reason.as_deref().map(|s| match s {
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                "stop_sequence" => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            });
            Some(StreamEvent::MessageDelta {
                stop_reason,
                output_tokens: usage.map_or(0, |u| u.output_tokens),
            })
        }
        SseEvent::MessageStop => Some(StreamEvent::MessageStop),
        SseEvent::Ping => Some(StreamEvent::Ping),
        SseEvent::Error { error } => Some(StreamEvent::Error {
            message: error.message,
        }),
    }
}

// ============================================================================
// Conversion Functions
// ============================================================================

fn convert_messages_to_api(messages: &[Message]) -> Vec<ApiMessage> {
    messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            let content: Vec<ApiContent> = msg
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => {
                        Some(ApiContent::Text { text: text.clone() })
                    }
                    ContentBlock::ToolUse { id, name, input } => Some(ApiContent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    }),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let content_text = match content {
                            ToolResultContent::Text(s) => s.clone(),
                            ToolResultContent::Json(v) => {
                                serde_json::to_string(v).unwrap_or_default()
                            }
                        };
                        Some(ApiContent::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content_text,
                            is_error: Some(*is_error),
                        })
                    }
                    // Thinking blocks MUST be echoed back to the API in multi-turn conversations
                    // when extended thinking is enabled. Include the signature if available.
                    ContentBlock::Thinking { thinking, signature } => {
                        Some(ApiContent::Thinking {
                            thinking: thinking.clone(),
                            signature: signature.clone(),
                        })
                    }
                })
                .collect();

            ApiMessage {
                role: role.to_string(),
                content,
            }
        })
        .collect()
}

fn convert_tools_to_api(tools: &[ToolDefinition]) -> Vec<ApiTool> {
    tools
        .iter()
        .map(|tool| ApiTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

fn convert_tool_choice(choice: &ToolChoice) -> Option<ApiToolChoice> {
    match choice {
        ToolChoice::Auto => Some(ApiToolChoice::Auto),
        ToolChoice::None => None,
        ToolChoice::Required => Some(ApiToolChoice::Any),
        ToolChoice::Tool { name } => Some(ApiToolChoice::Tool { name: name.clone() }),
    }
}

fn convert_response_to_aura(content: &[ApiContent]) -> Message {
    let blocks: Vec<ContentBlock> = content
        .iter()
        .map(|c| match c {
            ApiContent::Text { text } => ContentBlock::Text { text: text.clone() },
            ApiContent::Thinking { thinking, signature } => ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            },
            ApiContent::ToolUse { id, name, input } => ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            ApiContent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: ToolResultContent::Text(content.clone()),
                is_error: is_error.unwrap_or(false),
            },
        })
        .collect();

    Message {
        role: Role::Assistant,
        content: blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_new() {
        let config = AnthropicConfig::new("test-key", "claude-3-haiku");
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.default_model, "claude-3-haiku");
    }

    #[test]
    fn test_convert_messages() {
        let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];

        let api_msgs = convert_messages_to_api(&messages);
        assert_eq!(api_msgs.len(), 2);
        assert_eq!(api_msgs[0].role, "user");
        assert_eq!(api_msgs[1].role, "assistant");
    }

    #[test]
    fn test_convert_tools() {
        let tools = vec![ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }),
        )];

        let api_tools = convert_tools_to_api(&tools);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].name, "fs.read");
    }

    #[test]
    fn test_convert_tool_choice() {
        assert!(matches!(
            convert_tool_choice(&ToolChoice::Auto),
            Some(ApiToolChoice::Auto)
        ));
        assert!(matches!(
            convert_tool_choice(&ToolChoice::Required),
            Some(ApiToolChoice::Any)
        ));
        assert!(convert_tool_choice(&ToolChoice::None).is_none());
    }
}
