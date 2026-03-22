//! Normalized provider-agnostic conversation types.
//!
//! These are AURA canonical types for model interactions.
//! Every provider adapter maps to/from these types.

use serde::{Deserialize, Serialize};

// ============================================================================
// Role and Content Types
// ============================================================================

/// Role in conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// User message
    User,
    /// Assistant (model) message
    Assistant,
}

/// Content block in a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content
    Text { text: String },

    /// Thinking content (extended thinking from Claude)
    /// When echoing back to the API, the signature must be included.
    Thinking {
        thinking: String,
        /// Signature for the thinking block - required when echoing back to API
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Model requesting tool use (assistant only)
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// Result of tool execution (user only, in response to `tool_use`)
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        is_error: bool,
    },
}

impl ContentBlock {
    /// Create a text content block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a tool use content block.
    #[must_use]
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    /// Create a tool result content block.
    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: ToolResultContent,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content,
            is_error,
        }
    }

    /// Get the text content if this is a text block.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Check if this is a tool use block.
    #[must_use]
    pub const fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse { .. })
    }
}

// ============================================================================
// Tool Result Content
// ============================================================================

/// Content of a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Plain text result
    Text(String),
    /// Structured JSON result
    Json(serde_json::Value),
}

impl ToolResultContent {
    /// Create text content.
    #[must_use]
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Create JSON content.
    #[must_use]
    pub const fn json(value: serde_json::Value) -> Self {
        Self::Json(value)
    }
}

impl From<String> for ToolResultContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for ToolResultContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl From<serde_json::Value> for ToolResultContent {
    fn from(v: serde_json::Value) -> Self {
        Self::Json(v)
    }
}

// ============================================================================
// Message
// ============================================================================

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message sender
    pub role: Role,
    /// Content blocks in the message
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Create a new message.
    #[must_use]
    pub const fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    /// Create a user message with text content.
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Create an assistant message with text content.
    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Create a user message with tool results.
    #[must_use]
    pub fn tool_results(results: Vec<(String, ToolResultContent, bool)>) -> Self {
        Self {
            role: Role::User,
            content: results
                .into_iter()
                .map(|(id, content, is_error)| ContentBlock::ToolResult {
                    tool_use_id: id,
                    content,
                    is_error,
                })
                .collect(),
        }
    }

    /// Get all text content concatenated.
    #[must_use]
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("")
    }

    /// Get all tool use blocks.
    #[must_use]
    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content.iter().filter(|b| b.is_tool_use()).collect()
    }

    /// Check if this message contains tool use.
    #[must_use]
    pub fn has_tool_use(&self) -> bool {
        self.content.iter().any(ContentBlock::is_tool_use)
    }
}

// ============================================================================
// Tool Definition
// ============================================================================

/// Tool definition for the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (e.g., "fs.read", "search.code")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

// ============================================================================
// Tool Choice
// ============================================================================

/// How the model should choose tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to use tools
    #[default]
    Auto,
    /// Model should not use any tools
    None,
    /// Model must use a tool
    Required,
    /// Model must use the specified tool
    Tool { name: String },
}

impl ToolChoice {
    /// Create a tool choice for a specific tool.
    #[must_use]
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool { name: name.into() }
    }
}

// ============================================================================
// Thinking Configuration
// ============================================================================

/// Per-request extended thinking configuration.
///
/// When set on a `ModelRequest`, the provider will enable extended thinking
/// with the specified budget. When `None`, the provider may apply its own
/// heuristic (e.g., auto-enable for capable models).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Token budget allocated for the thinking phase.
    /// Must be >= 1024 and < `max_tokens`.
    pub budget_tokens: u32,
}

// ============================================================================
// Model Request
// ============================================================================

/// Request to the model.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Model identifier (e.g., "claude-opus-4-5-20251101")
    pub model: String,
    /// System prompt
    pub system: String,
    /// Conversation messages
    pub messages: Vec<Message>,
    /// Available tools
    pub tools: Vec<ToolDefinition>,
    /// Tool choice mode
    pub tool_choice: ToolChoice,
    /// Maximum tokens to generate
    pub max_tokens: u32,
    /// Sampling temperature
    pub temperature: Option<f32>,
    /// Extended thinking configuration. When `Some`, the provider enables
    /// thinking with the given budget. When `None`, provider-default behavior
    /// applies.
    pub thinking: Option<ThinkingConfig>,
}

impl ModelRequest {
    /// Create a new model request builder.
    #[must_use]
    pub fn builder(model: impl Into<String>, system: impl Into<String>) -> ModelRequestBuilder {
        ModelRequestBuilder::new(model, system)
    }
}

/// Builder for `ModelRequest`.
pub struct ModelRequestBuilder {
    model: String,
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    tool_choice: ToolChoice,
    max_tokens: u32,
    temperature: Option<f32>,
    thinking: Option<ThinkingConfig>,
}

impl ModelRequestBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new(model: impl Into<String>, system: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: 4096,
            temperature: None,
            thinking: None,
        }
    }

    /// Set messages.
    #[must_use]
    pub fn messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Add a message.
    #[must_use]
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Set tools.
    #[must_use]
    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    /// Set tool choice.
    #[must_use]
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Set max tokens.
    #[must_use]
    pub const fn max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = max;
        self
    }

    /// Set temperature.
    #[must_use]
    pub const fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set extended thinking configuration.
    #[must_use]
    pub fn thinking(mut self, config: ThinkingConfig) -> Self {
        self.thinking = Some(config);
        self
    }

    /// Build the request.
    #[must_use]
    pub fn build(self) -> ModelRequest {
        ModelRequest {
            model: self.model,
            system: self.system,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            thinking: self.thinking,
        }
    }
}

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
    pub input_tokens: u32,
    /// Number of output tokens
    pub output_tokens: u32,
    /// Cache creation input tokens (prompt caching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    /// Cache read input tokens (prompt caching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    /// Create new usage information.
    #[must_use]
    pub const fn new(input_tokens: u32, output_tokens: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    /// Total tokens used.
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Set cache token counts.
    #[must_use]
    pub const fn with_cache(mut self, creation: Option<u32>, read: Option<u32>) -> Self {
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

// ============================================================================
// Streaming Types
// ============================================================================

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
        input_tokens: Option<u32>,
        /// Cache creation input tokens (prompt caching)
        cache_creation_input_tokens: Option<u32>,
        /// Cache read input tokens (prompt caching)
        cache_read_input_tokens: Option<u32>,
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
        output_tokens: u32,
    },

    /// Message complete
    MessageStop,

    /// Ping event (keepalive)
    Ping,

    /// Error event
    Error {
        /// Error message
        message: String,
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

/// Accumulated state from streaming events.
///
/// This is used to build the final `ModelResponse` from streaming events.
#[derive(Debug, Clone, Default)]
pub struct StreamAccumulator {
    /// Message ID
    pub message_id: String,
    /// Model
    pub model: String,
    /// Accumulated text content
    pub text_content: String,
    /// Accumulated thinking content
    pub thinking_content: String,
    /// Signature for the thinking block (required for echoing back to API)
    pub thinking_signature: Option<String>,
    /// Whether we're currently in a thinking block
    pub in_thinking_block: bool,
    /// Accumulated tool uses
    pub tool_uses: Vec<AccumulatedToolUse>,
    /// Current tool use being built
    pub current_tool_use: Option<AccumulatedToolUse>,
    /// Stop reason
    pub stop_reason: Option<StopReason>,
    /// Input tokens
    pub input_tokens: u32,
    /// Output tokens
    pub output_tokens: u32,
    /// Cache creation input tokens (prompt caching)
    pub cache_creation_input_tokens: Option<u32>,
    /// Cache read input tokens (prompt caching)
    pub cache_read_input_tokens: Option<u32>,
}

/// Tool use being accumulated from streaming events.
#[derive(Debug, Clone, Default)]
pub struct AccumulatedToolUse {
    /// Tool use ID
    pub id: String,
    /// Tool name
    pub name: String,
    /// Accumulated JSON input
    pub input_json: String,
}

impl StreamAccumulator {
    /// Create a new accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a streaming event.
    pub fn process(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::MessageStart {
                message_id,
                model,
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                self.message_id.clone_from(message_id);
                self.model.clone_from(model);
                if let Some(tokens) = input_tokens {
                    self.input_tokens = *tokens;
                }
                self.cache_creation_input_tokens = *cache_creation_input_tokens;
                self.cache_read_input_tokens = *cache_read_input_tokens;
            }
            StreamEvent::ContentBlockStart { content_type, .. } => match content_type {
                StreamContentType::ToolUse { id, name } => {
                    self.current_tool_use = Some(AccumulatedToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input_json: String::new(),
                    });
                    self.in_thinking_block = false;
                }
                StreamContentType::Thinking => {
                    self.in_thinking_block = true;
                }
                StreamContentType::Text => {
                    self.in_thinking_block = false;
                }
            },
            StreamEvent::TextDelta { text } => {
                self.text_content.push_str(text);
            }
            StreamEvent::ThinkingDelta { thinking } => {
                self.thinking_content.push_str(thinking);
            }
            StreamEvent::SignatureDelta { signature } => {
                // Accumulate signature chunks
                if let Some(ref mut sig) = self.thinking_signature {
                    sig.push_str(signature);
                } else {
                    self.thinking_signature = Some(signature.clone());
                }
            }
            StreamEvent::InputJsonDelta { partial_json } => {
                if let Some(tool) = &mut self.current_tool_use {
                    tool.input_json.push_str(partial_json);
                }
            }
            StreamEvent::ContentBlockStop { .. } => {
                if let Some(tool) = self.current_tool_use.take() {
                    self.tool_uses.push(tool);
                }
                self.in_thinking_block = false;
            }
            StreamEvent::MessageDelta {
                stop_reason,
                output_tokens,
            } => {
                self.stop_reason = *stop_reason;
                self.output_tokens = *output_tokens;
            }
            StreamEvent::MessageStop | StreamEvent::Ping | StreamEvent::Error { .. } => {}
        }
    }

    /// Convert accumulated state to a `ModelResponse`.
    ///
    /// # Errors
    ///
    /// Returns error if tool use JSON is invalid.
    pub fn into_response(
        self,
        input_tokens: u32,
        latency_ms: u64,
    ) -> anyhow::Result<ModelResponse> {
        let effective_input_tokens = if self.input_tokens > 0 {
            self.input_tokens
        } else {
            input_tokens
        };

        let mut content_blocks = Vec::new();

        // Add thinking content first if present (it comes before text in the response)
        if !self.thinking_content.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: self.thinking_content,
                signature: self.thinking_signature,
            });
        }

        // Add text content if present
        if !self.text_content.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: self.text_content,
            });
        }

        // Add tool uses
        for tool in self.tool_uses {
            let input: serde_json::Value = if tool.input_json.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&tool.input_json)
                    .unwrap_or_else(|_| serde_json::json!({ "raw": tool.input_json }))
            };

            content_blocks.push(ContentBlock::ToolUse {
                id: tool.id,
                name: tool.name,
                input,
            });
        }

        let message = Message {
            role: Role::Assistant,
            content: content_blocks,
        };

        let model_used = self.model.clone();

        Ok(ModelResponse {
            stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
            message,
            usage: Usage {
                input_tokens: effective_input_tokens,
                output_tokens: self.output_tokens,
                cache_creation_input_tokens: self.cache_creation_input_tokens,
                cache_read_input_tokens: self.cache_read_input_tokens,
            },
            trace: ProviderTrace {
                request_id: Some(self.message_id),
                latency_ms,
                model: self.model,
            },
            model_used,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_user() {
        let msg = Message::user("Hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.text_content(), "Hello");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Hi there");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.text_content(), "Hi there");
    }

    #[test]
    fn test_message_tool_results() {
        let results = vec![
            ("id1".to_string(), ToolResultContent::text("result1"), false),
            ("id2".to_string(), ToolResultContent::text("error"), true),
        ];
        let msg = Message::tool_results(results);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 2);
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition::new(
            "fs.read",
            "Read a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        );
        assert_eq!(tool.name, "fs.read");
    }

    #[test]
    fn test_model_request_builder() {
        let request = ModelRequest::builder("claude-sonnet-4-20250514", "You are helpful")
            .message(Message::user("Hi"))
            .max_tokens(1000)
            .temperature(0.7)
            .build();

        assert_eq!(request.model, "claude-sonnet-4-20250514");
        assert_eq!(request.system, "You are helpful");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.max_tokens, 1000);
        assert_eq!(request.temperature, Some(0.7));
    }

    #[test]
    fn test_content_block_serialization() {
        let text = ContentBlock::text("Hello");
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));

        let tool_use =
            ContentBlock::tool_use("123", "fs.read", serde_json::json!({"path": "test"}));
        let json = serde_json::to_string(&tool_use).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""));
    }

    #[test]
    fn test_usage() {
        let usage = Usage::new(100, 50);
        assert_eq!(usage.total(), 150);
    }

    // ========================================================================
    // Message Tests
    // ========================================================================

    #[test]
    fn test_message_with_multiple_content_blocks() {
        let msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("Let me help you."),
                ContentBlock::tool_use("tool1", "fs_read", serde_json::json!({"path": "test.txt"})),
            ],
        );

        assert!(msg.has_tool_use());
        assert_eq!(msg.tool_uses().len(), 1);
        assert_eq!(msg.text_content(), "Let me help you.");
    }

    #[test]
    fn test_message_text_content_concatenation() {
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Hello "), ContentBlock::text("world!")],
        );

        assert_eq!(msg.text_content(), "Hello world!");
    }

    #[test]
    fn test_message_no_tool_use() {
        let msg = Message::assistant("Just text");
        assert!(!msg.has_tool_use());
        assert!(msg.tool_uses().is_empty());
    }

    // ========================================================================
    // ContentBlock Tests
    // ========================================================================

    #[test]
    fn test_content_block_as_text() {
        let text = ContentBlock::text("hello");
        assert_eq!(text.as_text(), Some("hello"));

        let tool_use = ContentBlock::tool_use("id", "name", serde_json::json!({}));
        assert_eq!(tool_use.as_text(), None);
    }

    #[test]
    fn test_content_block_tool_result() {
        let result =
            ContentBlock::tool_result("tool123", ToolResultContent::text("success"), false);

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool123");
                assert!(!is_error);
                if let ToolResultContent::Text(t) = content {
                    assert_eq!(t, "success");
                } else {
                    panic!("Expected Text content");
                }
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_thinking_block() {
        let thinking = ContentBlock::Thinking {
            thinking: "Let me think about this...".to_string(),
            signature: Some("sig123".to_string()),
        };

        let json = serde_json::to_string(&thinking).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("sig123"));
    }

    // ========================================================================
    // ToolResultContent Tests
    // ========================================================================

    #[test]
    fn test_tool_result_content_text() {
        let content = ToolResultContent::text("result");
        if let ToolResultContent::Text(t) = content {
            assert_eq!(t, "result");
        } else {
            panic!("Expected Text");
        }
    }

    #[test]
    fn test_tool_result_content_json() {
        let content = ToolResultContent::json(serde_json::json!({"key": "value"}));
        if let ToolResultContent::Json(v) = content {
            assert_eq!(v["key"], "value");
        } else {
            panic!("Expected Json");
        }
    }

    #[test]
    fn test_tool_result_content_from_string() {
        let content: ToolResultContent = "hello".into();
        if let ToolResultContent::Text(t) = content {
            assert_eq!(t, "hello");
        } else {
            panic!("Expected Text");
        }
    }

    #[test]
    fn test_tool_result_content_from_value() {
        let content: ToolResultContent = serde_json::json!({"a": 1}).into();
        if let ToolResultContent::Json(v) = content {
            assert_eq!(v["a"], 1);
        } else {
            panic!("Expected Json");
        }
    }

    // ========================================================================
    // ToolChoice Tests
    // ========================================================================

    #[test]
    fn test_tool_choice_variants() {
        let auto = ToolChoice::Auto;
        let none = ToolChoice::None;
        let required = ToolChoice::Required;
        let specific = ToolChoice::tool("fs_read");

        assert!(matches!(auto, ToolChoice::Auto));
        assert!(matches!(none, ToolChoice::None));
        assert!(matches!(required, ToolChoice::Required));
        assert!(matches!(specific, ToolChoice::Tool { name } if name == "fs_read"));
    }

    // ========================================================================
    // ModelRequest Builder Tests
    // ========================================================================

    #[test]
    fn test_model_request_builder_with_tools() {
        let tool = ToolDefinition::new(
            "test_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        );

        let request = ModelRequest::builder("model", "system")
            .tools(vec![tool])
            .tool_choice(ToolChoice::Required)
            .build();

        assert_eq!(request.tools.len(), 1);
        assert!(matches!(request.tool_choice, ToolChoice::Required));
    }

    #[test]
    fn test_model_request_builder_defaults() {
        let request = ModelRequest::builder("model", "system").build();

        assert_eq!(request.max_tokens, 4096);
        assert!(request.temperature.is_none());
        assert!(matches!(request.tool_choice, ToolChoice::Auto));
        assert!(request.messages.is_empty());
        assert!(request.tools.is_empty());
    }

    // ========================================================================
    // ModelResponse Tests
    // ========================================================================

    #[test]
    fn test_model_response_wants_tool_use() {
        let response = ModelResponse::new(
            StopReason::ToolUse,
            Message::assistant(""),
            Usage::new(100, 50),
            ProviderTrace::new("model", 100),
        );

        assert!(response.wants_tool_use());
        assert!(!response.is_end_turn());
    }

    #[test]
    fn test_model_response_end_turn() {
        let response = ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant("Done"),
            Usage::new(100, 50),
            ProviderTrace::new("model", 100),
        );

        assert!(!response.wants_tool_use());
        assert!(response.is_end_turn());
    }

    // ========================================================================
    // StreamAccumulator Tests
    // ========================================================================

    #[test]
    fn test_stream_accumulator_text_only() {
        let mut acc = StreamAccumulator::new();

        acc.process(&StreamEvent::MessageStart {
            message_id: "msg1".to_string(),
            model: "claude".to_string(),
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        });
        acc.process(&StreamEvent::TextDelta {
            text: "Hello ".to_string(),
        });
        acc.process(&StreamEvent::TextDelta {
            text: "world!".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });
        acc.process(&StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 10,
        });
        acc.process(&StreamEvent::MessageStop);

        assert_eq!(acc.message_id, "msg1");
        assert_eq!(acc.model, "claude");
        assert_eq!(acc.text_content, "Hello world!");
        assert_eq!(acc.output_tokens, 10);
        assert_eq!(acc.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn test_stream_accumulator_tool_use() {
        let mut acc = StreamAccumulator::new();

        acc.process(&StreamEvent::MessageStart {
            message_id: "msg1".to_string(),
            model: "claude".to_string(),
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "tool1".to_string(),
                name: "fs_read".to_string(),
            },
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#"{"path":"#.to_string(),
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#""test.txt"}"#.to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });
        acc.process(&StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            output_tokens: 20,
        });

        assert_eq!(acc.tool_uses.len(), 1);
        assert_eq!(acc.tool_uses[0].id, "tool1");
        assert_eq!(acc.tool_uses[0].name, "fs_read");
        assert_eq!(acc.tool_uses[0].input_json, r#"{"path":"test.txt"}"#);
    }

    #[test]
    fn test_stream_accumulator_thinking() {
        let mut acc = StreamAccumulator::new();

        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Thinking,
        });
        acc.process(&StreamEvent::ThinkingDelta {
            thinking: "Let me ".to_string(),
        });
        acc.process(&StreamEvent::ThinkingDelta {
            thinking: "think...".to_string(),
        });
        acc.process(&StreamEvent::SignatureDelta {
            signature: "sig_abc".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });

        assert_eq!(acc.thinking_content, "Let me think...");
        assert_eq!(acc.thinking_signature, Some("sig_abc".to_string()));
    }

    #[test]
    fn test_stream_accumulator_mixed_content() {
        let mut acc = StreamAccumulator::new();

        // Thinking block
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Thinking,
        });
        acc.process(&StreamEvent::ThinkingDelta {
            thinking: "Thinking...".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });

        // Text block
        acc.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: StreamContentType::Text,
        });
        acc.process(&StreamEvent::TextDelta {
            text: "Response text".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 1 });

        // Tool use block
        acc.process(&StreamEvent::ContentBlockStart {
            index: 2,
            content_type: StreamContentType::ToolUse {
                id: "tool1".to_string(),
                name: "fs_ls".to_string(),
            },
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#"{"path":"."}"#.to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 2 });

        assert_eq!(acc.thinking_content, "Thinking...");
        assert_eq!(acc.text_content, "Response text");
        assert_eq!(acc.tool_uses.len(), 1);
    }

    #[test]
    fn test_stream_accumulator_into_response() {
        let mut acc = StreamAccumulator::new();

        acc.process(&StreamEvent::MessageStart {
            message_id: "msg123".to_string(),
            model: "claude-opus-4-5-20251101".to_string(),
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        });
        acc.process(&StreamEvent::TextDelta {
            text: "Hello!".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });
        acc.process(&StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 5,
        });

        let response = acc.into_response(100, 500).unwrap();

        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.message.text_content(), "Hello!");
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.trace.model, "claude-opus-4-5-20251101");
        assert_eq!(response.trace.latency_ms, 500);
    }

    #[test]
    fn test_stream_accumulator_into_response_with_thinking() {
        let mut acc = StreamAccumulator::new();

        acc.thinking_content = "Deep thoughts...".to_string();
        acc.thinking_signature = Some("sig123".to_string());
        acc.text_content = "Here's my answer".to_string();
        acc.stop_reason = Some(StopReason::EndTurn);
        acc.model = "claude".to_string();

        let response = acc.into_response(50, 200).unwrap();

        // Thinking should come before text in content blocks
        assert_eq!(response.message.content.len(), 2);
        assert!(matches!(
            &response.message.content[0],
            ContentBlock::Thinking { .. }
        ));
        assert!(matches!(
            &response.message.content[1],
            ContentBlock::Text { .. }
        ));
    }

    #[test]
    fn test_stream_accumulator_into_response_with_tool() {
        let mut acc = StreamAccumulator::new();

        acc.tool_uses.push(AccumulatedToolUse {
            id: "tool1".to_string(),
            name: "fs_read".to_string(),
            input_json: r#"{"path":"test.txt"}"#.to_string(),
        });
        acc.stop_reason = Some(StopReason::ToolUse);

        let response = acc.into_response(50, 100).unwrap();

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert!(response.message.has_tool_use());

        if let ContentBlock::ToolUse { id, name, input } = &response.message.content[0] {
            assert_eq!(id, "tool1");
            assert_eq!(name, "fs_read");
            assert_eq!(input["path"], "test.txt");
        } else {
            panic!("Expected ToolUse block");
        }
    }

    #[test]
    fn test_stream_accumulator_invalid_json_handling() {
        let mut acc = StreamAccumulator::new();

        acc.tool_uses.push(AccumulatedToolUse {
            id: "tool1".to_string(),
            name: "test".to_string(),
            input_json: "invalid json {{{".to_string(),
        });

        let response = acc.into_response(0, 0).unwrap();

        // Invalid JSON should be wrapped in a "raw" field
        if let ContentBlock::ToolUse { input, .. } = &response.message.content[0] {
            assert!(input.get("raw").is_some());
        } else {
            panic!("Expected ToolUse block");
        }
    }

    #[test]
    fn test_stream_accumulator_empty_tool_json() {
        let mut acc = StreamAccumulator::new();

        acc.tool_uses.push(AccumulatedToolUse {
            id: "tool1".to_string(),
            name: "test".to_string(),
            input_json: String::new(),
        });

        let response = acc.into_response(0, 0).unwrap();

        // Empty JSON should become empty object
        if let ContentBlock::ToolUse { input, .. } = &response.message.content[0] {
            assert!(input.is_object());
            assert!(input.as_object().unwrap().is_empty());
        } else {
            panic!("Expected ToolUse block");
        }
    }

    #[test]
    fn test_stream_accumulator_multiple_tools() {
        let mut acc = StreamAccumulator::new();

        // First tool
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::ToolUse {
                id: "tool1".to_string(),
                name: "fs_ls".to_string(),
            },
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#"{"path":"."}"#.to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });

        // Second tool
        acc.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: StreamContentType::ToolUse {
                id: "tool2".to_string(),
                name: "fs_read".to_string(),
            },
        });
        acc.process(&StreamEvent::InputJsonDelta {
            partial_json: r#"{"path":"file.txt"}"#.to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 1 });

        assert_eq!(acc.tool_uses.len(), 2);
        assert_eq!(acc.tool_uses[0].name, "fs_ls");
        assert_eq!(acc.tool_uses[1].name, "fs_read");
    }

    #[test]
    fn test_stream_accumulator_ping_and_error() {
        let mut acc = StreamAccumulator::new();

        // These should be handled gracefully
        acc.process(&StreamEvent::Ping);
        acc.process(&StreamEvent::Error {
            message: "test error".to_string(),
        });

        // State should be unchanged
        assert!(acc.text_content.is_empty());
    }

    // ========================================================================
    // ProviderTrace Tests
    // ========================================================================

    #[test]
    fn test_provider_trace() {
        let trace = ProviderTrace::new("claude", 500).with_request_id("req123");

        assert_eq!(trace.model, "claude");
        assert_eq!(trace.latency_ms, 500);
        assert_eq!(trace.request_id, Some("req123".to_string()));
    }

    // ========================================================================
    // Usage Cache Tests
    // ========================================================================

    #[test]
    fn test_usage_with_cache_tokens() {
        let usage = Usage::new(100, 50).with_cache(Some(80), Some(20));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, Some(80));
        assert_eq!(usage.cache_read_input_tokens, Some(20));
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn test_usage_default_has_no_cache() {
        let usage = Usage::default();
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, None);
    }

    // ========================================================================
    // StreamAccumulator Input Tokens from MessageStart
    // ========================================================================

    #[test]
    fn test_stream_accumulator_input_tokens_from_message_start() {
        let mut acc = StreamAccumulator::new();

        acc.process(&StreamEvent::MessageStart {
            message_id: "msg_cache".to_string(),
            model: "claude".to_string(),
            input_tokens: Some(200),
            cache_creation_input_tokens: Some(150),
            cache_read_input_tokens: Some(50),
        });
        acc.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamContentType::Text,
        });
        acc.process(&StreamEvent::TextDelta {
            text: "Cached!".to_string(),
        });
        acc.process(&StreamEvent::ContentBlockStop { index: 0 });
        acc.process(&StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 3,
        });

        let response = acc.into_response(0, 100).unwrap();

        // Accumulated input_tokens should take precedence over parameter (0)
        assert_eq!(response.usage.input_tokens, 200);
        assert_eq!(response.usage.cache_creation_input_tokens, Some(150));
        assert_eq!(response.usage.cache_read_input_tokens, Some(50));
        assert_eq!(response.model_used, "claude");
    }
}
