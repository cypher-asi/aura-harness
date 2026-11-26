//! [`StreamAccumulator`]: folds [`StreamEvent`]s into a [`ModelResponse`].

use super::{PartialToolUse, StreamContentType, StreamEvent};
use crate::error::ReasonerError;
use crate::types::content::{ContentBlock, Role};
use crate::types::message::Message;
use crate::types::response::{ModelResponse, ProviderTrace, StopReason, Usage};

/// Merge a usage total reported on `message_delta` into the value seeded
/// by `message_start`.
///
/// A reported value wins when it is non-zero (Anthropic's delta usage is
/// cumulative and OpenRouter reports the only real total there for
/// non-Anthropic upstreams) or when nothing was known yet. A zero or
/// missing delta value never erases a good `message_start` value.
fn merge_delta_total(current: Option<u64>, delta: Option<u64>) -> Option<u64> {
    match (current, delta) {
        (_, Some(reported)) if reported > 0 => Some(reported),
        (None, Some(reported)) => Some(reported),
        (current, _) => current,
    }
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
    /// Error captured from a `StreamEvent::Error`.
    pub stream_error: Option<String>,
    /// HTTP `x-request-id` captured from the streaming response.
    ///
    /// Populated by the synthetic [`StreamEvent::HttpMeta`] that
    /// `SseStream` yields before the first provider event, and
    /// secondarily from a request_id embedded in an SSE error body
    /// (only when the header-derived value is still `None`). This is
    /// the id operators should use to correlate a failed stream with
    /// provider / router logs — it is distinct from
    /// [`Self::message_id`], which is the Anthropic message id.
    pub provider_request_id: Option<String>,
    /// Plan usage from the [`StreamEvent::HttpMeta`] preamble.
    pub plan_usage: Vec<aura_core_types::PlanUsage>,
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
                self.on_message_start_usage(
                    *input_tokens,
                    *cache_creation_input_tokens,
                    *cache_read_input_tokens,
                );
            }
            StreamEvent::ContentBlockStart { content_type, .. } => {
                self.on_content_block_start(content_type);
            }
            StreamEvent::TextDelta { text } => self.text_content.push_str(text),
            StreamEvent::ThinkingDelta { thinking } => self.thinking_content.push_str(thinking),
            StreamEvent::SignatureDelta { signature } => self.on_signature_delta(signature),
            StreamEvent::InputJsonDelta { partial_json } => self.on_input_json_delta(partial_json),
            StreamEvent::ContentBlockStop { .. } => self.on_content_block_stop(),
            StreamEvent::MessageDelta {
                stop_reason,
                output_tokens,
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                self.stop_reason = *stop_reason;
                self.output_tokens = *output_tokens;
                self.on_message_delta_usage(
                    *input_tokens,
                    *cache_creation_input_tokens,
                    *cache_read_input_tokens,
                );
            }
            StreamEvent::MessageStop | StreamEvent::Ping => {}
            StreamEvent::Error {
                message,
                request_id,
            } => self.on_error(message, request_id.as_ref()),
            StreamEvent::HttpMeta {
                request_id,
                plan_usage,
            } => self.on_http_meta(request_id.as_ref(), plan_usage),
        }
    }

    fn on_message_start_usage(
        &mut self,
        input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
    ) {
        if let Some(tokens) = input_tokens {
            self.input_tokens = tokens;
        }
        // A start frame without cache numbers must not erase
        // numbers an earlier frame already supplied.
        if cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = cache_creation_input_tokens;
        }
        if cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = cache_read_input_tokens;
        }
    }

    fn on_content_block_start(&mut self, content_type: &StreamContentType) {
        match content_type {
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
        }
    }

    fn on_signature_delta(&mut self, signature: &str) {
        if let Some(ref mut sig) = self.thinking_signature {
            sig.push_str(signature);
        } else {
            self.thinking_signature = Some(signature.to_owned());
        }
    }

    fn on_input_json_delta(&mut self, partial_json: &str) {
        if let Some(tool) = &mut self.current_tool_use {
            tool.input_json.push_str(partial_json);
        }
    }

    fn on_content_block_stop(&mut self) {
        if let Some(tool) = self.current_tool_use.take() {
            self.tool_uses.push(tool);
        }
        self.in_thinking_block = false;
    }

    fn on_message_delta_usage(
        &mut self,
        input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
    ) {
        // `input_tokens` uses 0 for "unknown", so only a
        // non-zero delta total can improve on it.
        if let Some(tokens) = input_tokens.filter(|tokens| *tokens > 0) {
            self.input_tokens = tokens;
        }
        self.cache_creation_input_tokens = merge_delta_total(
            self.cache_creation_input_tokens,
            cache_creation_input_tokens,
        );
        self.cache_read_input_tokens =
            merge_delta_total(self.cache_read_input_tokens, cache_read_input_tokens);
    }

    fn on_error(&mut self, message: &str, request_id: Option<&String>) {
        self.stream_error = Some(message.to_owned());
        // Body-level request_id only wins when the HTTP header
        // path did not already supply one — the header value
        // also covers the success path and is therefore the
        // source of truth.
        if self.provider_request_id.is_none() {
            if let Some(id) = request_id {
                self.provider_request_id = Some(id.clone());
            }
        }
    }

    fn on_http_meta(
        &mut self,
        request_id: Option<&String>,
        plan_usage: &[aura_core_types::PlanUsage],
    ) {
        if let Some(id) = request_id {
            self.provider_request_id = Some(id.clone());
        }
        if !plan_usage.is_empty() {
            self.plan_usage = plan_usage.to_vec();
        }
    }

    /// Convert accumulated state to a `ModelResponse`.
    ///
    /// # Errors
    ///
    /// Returns `ReasonerError` if the accumulated state is invalid.
    pub fn into_response(
        mut self,
        input_tokens: u64,
        latency_ms: u64,
    ) -> Result<ModelResponse, ReasonerError> {
        let effective_input_tokens = if self.input_tokens > 0 {
            self.input_tokens
        } else {
            input_tokens
        };

        self.recover_pending_tool_use();

        // Always propagate a mid-stream error, even if partial text
        // or tool_use blocks arrived first. The previous behaviour
        // swallowed the error when any content was accumulated,
        // which caused partial tool-use blocks to be executed as if
        // the stream had finished cleanly -- a correctness bug that
        // could trigger malformed tool calls on the next iteration.
        //
        // NOTE: the caller (`complete_with_streaming`) only relies
        // on the `StreamAbortedWithPartial` variant for the narrow
        // case where a `tool_use` was in flight when the stream
        // died, so it can drive a per-tool-call retry that
        // re-issues a fresh streaming request and preserves the
        // partial input_json (preventing a dropped Write/Edit).
        // When there is NO in-flight tool, that retry loop adds
        // nothing — it just re-fires the identical request with
        // zero tracing output. We instead surface a generic
        // `Transient` so the existing
        // `stream_error_is_retryable` classifier routes the call
        // through `complete_and_emit_as_deltas`, which emits a
        // `StreamReset` to the UI and falls back to non-streaming
        // `provider.complete()` (whose retry loop has proper
        // tracing + body-cap shrink + provider fallback). Without
        // this branch a single Anthropic 5xx blip wedges chat in
        // an invisible 28-second silent-retry storm — see the
        // `fix-silent-stream-retry-storm` plan.
        //
        // Include model + message_id when known so the operator-visible
        // failure string is actionable (operators can correlate
        // `msg_id` with provider / router logs). The `err_msg`
        // string already carries the Anthropic `error.type` prefix
        // (see `anthropic::sse::parse_sse_event`) when the upstream
        // supplied one.
        if let Some(err_msg) = self.stream_error.take() {
            return Err(self.stream_error_result(&err_msg));
        }

        let message = Message {
            role: Role::Assistant,
            content: self.take_content_blocks(),
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
                message_id: if self.message_id.is_empty() {
                    None
                } else {
                    Some(self.message_id)
                },
                provider_request_id: self.provider_request_id,
                plan_usage: self.plan_usage,
                latency_ms,
                model: self.model,
            },
            model_used,
        })
    }

    /// Recover any in-progress tool_use that was not finalized by a
    /// ContentBlockStop (e.g. the stream was truncated). Without this
    /// the tool is silently lost and the response looks like EndTurn
    /// with no tool calls.
    fn recover_pending_tool_use(&mut self) {
        if let Some(pending) = self.current_tool_use.take() {
            tracing::warn!(
                tool_name = %pending.name,
                tool_id = %pending.id,
                json_len = pending.input_json.len(),
                "Stream ended with an in-progress tool_use block — \
                 recovering partial tool call"
            );
            self.tool_uses.push(pending);
            if self.stop_reason.is_none() {
                self.stop_reason = Some(StopReason::MaxTokens);
            }
        }
    }

    /// Turn a captured mid-stream error into the `Err` that
    /// [`Self::into_response`] returns.
    fn stream_error_result(&mut self, err_msg: &str) -> ReasonerError {
        let reason = self.stream_error_reason(err_msg);

        // The recovery path above (pre-stream-error) already pushed
        // a pending `current_tool_use` onto `self.tool_uses` and
        // set `current_tool_use` to None. For the aborted-stream
        // path we undo that: pop the recovered tool off
        // `self.tool_uses` if it is still there so the caller gets
        // it back as `partial_tool_use` instead of seeing it as a
        // completed tool call.
        let partial_tool_use = self.tool_uses.pop().map(|pending| PartialToolUse {
            tool_use_id: pending.id,
            tool_name: pending.name,
            partial_json: pending.input_json,
        });

        match partial_tool_use {
            Some(partial) => ReasonerError::StreamAbortedWithPartial {
                reason,
                partial_tool_use: Some(partial),
            },
            None => ReasonerError::Transient {
                status: 502,
                message: reason,
                retry_after: None,
            },
        }
    }

    /// `stream terminated with error (model=…, msg_id=…, request_id=…): {message}`,
    /// naming whichever of the three ids are known.
    pub(crate) fn stream_error_reason(&self, message: &str) -> String {
        let mut context_parts: Vec<String> = Vec::new();
        if !self.model.is_empty() {
            context_parts.push(format!("model={}", self.model));
        }
        if !self.message_id.is_empty() {
            context_parts.push(format!("msg_id={}", self.message_id));
        }
        if let Some(ref req_id) = self.provider_request_id {
            if !req_id.is_empty() {
                context_parts.push(format!("request_id={req_id}"));
            }
        }
        let context = if context_parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", context_parts.join(", "))
        };
        format!("stream terminated with error{context}: {message}")
    }

    /// The response content in order: thinking, text, then tool uses.
    fn take_content_blocks(&mut self) -> Vec<ContentBlock> {
        let mut content_blocks = Vec::new();

        if !self.thinking_content.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: std::mem::take(&mut self.thinking_content),
                signature: self.thinking_signature.take(),
            });
        }

        if !self.text_content.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: std::mem::take(&mut self.text_content),
            });
        }

        content_blocks.extend(
            std::mem::take(&mut self.tool_uses)
                .into_iter()
                .map(tool_use_block),
        );
        content_blocks
    }
}

/// A finished tool use as a content block; input that is not valid JSON
/// is kept as `{"raw": …}`.
fn tool_use_block(tool: AccumulatedToolUse) -> ContentBlock {
    let input: serde_json::Value = if tool.input_json.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&tool.input_json)
            .unwrap_or_else(|_| serde_json::json!({ "raw": tool.input_json }))
    };

    ContentBlock::ToolUse {
        id: tool.id,
        name: tool.name,
        input,
    }
}
