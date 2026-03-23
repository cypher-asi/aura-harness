use super::content::{ContentBlock, Role};
use super::message::Message;
use super::response::{ModelResponse, ProviderTrace, StopReason, Usage};

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
    pub input_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Cache creation input tokens (prompt caching)
    pub cache_creation_input_tokens: Option<u64>,
    /// Cache read input tokens (prompt caching)
    pub cache_read_input_tokens: Option<u64>,
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
        input_tokens: u64,
        latency_ms: u64,
    ) -> anyhow::Result<ModelResponse> {
        let effective_input_tokens = if self.input_tokens > 0 {
            self.input_tokens
        } else {
            input_tokens
        };

        let mut content_blocks = Vec::new();

        if !self.thinking_content.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: self.thinking_content,
                signature: self.thinking_signature,
            });
        }

        if !self.text_content.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: self.text_content,
            });
        }

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
