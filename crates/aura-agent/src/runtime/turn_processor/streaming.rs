//! Streaming event types and model completion with streaming.

use super::TurnProcessor;
use aura_reasoner::{ModelProvider, ModelRequest, ModelResponse, StreamAccumulator, StreamEvent};
use aura_store::Store;
use aura_tools::ToolRegistry;
use futures_util::StreamExt;
use tokio::time::{timeout, Duration};
use tracing::{error, info};

/// Callback type for streaming text events.
///
/// This callback is invoked whenever a text delta is received from the model,
/// allowing real-time display of the response as it's generated.
pub type StreamCallback = Box<dyn Fn(StreamCallbackEvent) + Send + Sync>;

/// Events that can be sent via the streaming callback.
#[derive(Debug, Clone)]
pub enum StreamCallbackEvent {
    /// A chunk of thinking content was received
    ThinkingDelta(String),
    /// Thinking block completed
    ThinkingComplete,
    /// A chunk of text was received
    TextDelta(String),
    /// A tool use started
    ToolStart {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
    },
    /// Incremental snapshot of tool input JSON as it streams in.
    ToolInputSnapshot {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
        /// Accumulated input JSON so far (may be partial/incomplete)
        input: String,
    },
    /// A tool use completed
    ToolComplete {
        /// Tool name
        name: String,
        /// Tool arguments (JSON)
        args: serde_json::Value,
        /// Tool result text
        result: String,
        /// Whether the tool failed
        is_error: bool,
    },
    /// Streaming is complete for this step
    StepComplete,
    /// An error occurred during the turn (LLM, tool callback, timeout, etc.).
    Error {
        /// Machine-readable error code.
        code: String,
        /// Human-readable description.
        message: String,
        /// Whether the session can continue after this error.
        recoverable: bool,
    },
}

/// Classify an LLM error message into a machine-readable code and recoverability.
///
/// Returns `(code, recoverable)`. Common codes:
/// - `"rate_limit"` — 429 / rate-limited, recoverable (retry after backoff)
/// - `"auth_error"` — 401/403, not recoverable without config change
/// - `"overloaded"` — 529 / server overloaded, recoverable
/// - `"timeout"` — request timed out, recoverable
/// - `"llm_error"` — catch-all for other LLM failures
fn classify_llm_error(message: &str) -> (&'static str, bool) {
    let lower = message.to_lowercase();
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("too many requests")
    {
        ("rate_limit", true)
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("authentication")
    {
        ("auth_error", false)
    } else if lower.contains("overloaded") || lower.contains("529") || lower.contains("503") {
        ("overloaded", true)
    } else if lower.contains("timeout") || lower.contains("timed out") {
        ("timeout", true)
    } else {
        ("llm_error", true)
    }
}

impl<P, S, R> TurnProcessor<P, S, R>
where
    P: ModelProvider,
    S: Store,
    R: ToolRegistry,
{
    /// Complete a model request with streaming, emitting events to the callback.
    pub(super) async fn complete_with_streaming(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<ModelResponse> {
        let timeout_duration = Duration::from_millis(self.config.model_timeout_ms);

        match timeout(
            timeout_duration,
            self.complete_with_streaming_inner(request),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let msg = format!(
                    "Model call timed out after {}ms",
                    self.config.model_timeout_ms
                );
                self.emit_stream_event(StreamCallbackEvent::Error {
                    code: "timeout".to_string(),
                    message: msg.clone(),
                    recoverable: true,
                });
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn complete_with_streaming_inner(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<ModelResponse> {
        let start = std::time::Instant::now();

        let mut stream = match self.provider.complete_streaming(request).await {
            Ok(s) => s,
            Err(e) => {
                let err_msg = e.to_string();
                let (code, recoverable) = classify_llm_error(&err_msg);
                self.emit_stream_event(StreamCallbackEvent::Error {
                    code: code.to_string(),
                    message: err_msg,
                    recoverable,
                });
                return Err(e.into());
            }
        };

        let mut accumulator = StreamAccumulator::new();
        let input_tokens = 0u64;
        let mut in_thinking_block = false;

        loop {
            let event_result = if let Some(token) = &self.cancellation_token {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        info!("Stream cancelled by cancellation token");
                        break;
                    }
                    next = stream.next() => next,
                }
            } else {
                stream.next().await
            };

            let Some(event_result) = event_result else {
                break;
            };

            match event_result {
                Ok(event) => {
                    match &event {
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::Thinking,
                            ..
                        } => {
                            in_thinking_block = true;
                        }
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::ToolUse { id, name },
                            ..
                        } => {
                            if in_thinking_block {
                                self.emit_stream_event(StreamCallbackEvent::ThinkingComplete);
                                in_thinking_block = false;
                            }
                            self.emit_stream_event(StreamCallbackEvent::ToolStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                        }
                        StreamEvent::ContentBlockStart {
                            content_type: aura_reasoner::StreamContentType::Text,
                            ..
                        }
                        | StreamEvent::ContentBlockStop { .. } => {
                            if in_thinking_block {
                                self.emit_stream_event(StreamCallbackEvent::ThinkingComplete);
                                in_thinking_block = false;
                            }
                        }
                        StreamEvent::ThinkingDelta { thinking } => {
                            self.emit_stream_event(StreamCallbackEvent::ThinkingDelta(
                                thinking.clone(),
                            ));
                        }
                        StreamEvent::TextDelta { text } => {
                            self.emit_stream_event(StreamCallbackEvent::TextDelta(text.clone()));
                        }
                        StreamEvent::Error { message } => {
                            error!(error = %message, "Stream error from provider");
                            let (code, recoverable) = classify_llm_error(message);
                            self.emit_stream_event(StreamCallbackEvent::Error {
                                code: code.to_string(),
                                message: message.clone(),
                                recoverable,
                            });
                            anyhow::bail!("Stream error: {message}");
                        }
                        _ => {}
                    }

                    accumulator.process(&event);

                    if matches!(event, StreamEvent::InputJsonDelta { .. }) {
                        if let Some(tool) = &accumulator.current_tool_use {
                            self.emit_stream_event(StreamCallbackEvent::ToolInputSnapshot {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                                input: tool.input_json.clone(),
                            });
                        }
                    }

                    if matches!(event, StreamEvent::MessageStop) {
                        break;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Stream error");
                    let err_msg = e.to_string();
                    let (code, recoverable) = classify_llm_error(&err_msg);
                    self.emit_stream_event(StreamCallbackEvent::Error {
                        code: code.to_string(),
                        message: err_msg.clone(),
                        recoverable,
                    });
                    anyhow::bail!("Stream error: {err_msg}");
                }
            }
        }

        self.emit_stream_event(StreamCallbackEvent::StepComplete);

        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        accumulator
            .into_response(input_tokens, latency_ms)
            .map_err(Into::into)
    }
}
