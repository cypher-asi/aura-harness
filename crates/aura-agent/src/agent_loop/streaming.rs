//! Streaming model calls and event emission.

use std::time::Instant;

use aura_reasoner::{
    ModelProvider, ModelRequest, ModelResponse, ReasonerError, StreamAccumulator,
    StreamContentType, StreamEvent,
};
use chrono::Utc;
use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::events::{AgentLoopEvent, DebugEvent};

use super::iteration::LlmCallError;
use super::AgentLoop;

/// Send an event through the channel if present.
pub(super) fn emit(tx: Option<&Sender<AgentLoopEvent>>, event: AgentLoopEvent) {
    if let Some(tx) = tx {
        if let Err(e) = tx.try_send(event) {
            tracing::warn!("agent event channel full or closed: {e}");
        }
    }
}

/// Emit an [`AgentLoopEvent::IterationComplete`] event along with the
/// matching [`DebugEvent::Iteration`] frame for the `aura-os` run
/// bundle. `duration_ms` reflects wall-clock time since the start of
/// the current iteration (model call + tool dispatch); `tool_calls` is
/// the number of `ContentBlock::ToolUse` blocks in the response.
pub(super) fn emit_iteration_complete(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    iteration: usize,
    response: &ModelResponse,
    iteration_started_at: Instant,
) {
    emit(
        event_tx,
        AgentLoopEvent::IterationComplete {
            iteration,
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    );

    let tool_calls = response
        .message
        .content
        .iter()
        .filter(|b| matches!(b, aura_reasoner::ContentBlock::ToolUse { .. }))
        .count();

    let duration_ms = u64::try_from(iteration_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    let index = u32::try_from(iteration).unwrap_or(u32::MAX);
    let tool_calls = u32::try_from(tool_calls).unwrap_or(u32::MAX);

    emit(
        event_tx,
        AgentLoopEvent::Debug(DebugEvent::Iteration {
            timestamp: Utc::now(),
            index,
            tool_calls,
            duration_ms,
            task_id: None,
        }),
    );
}

/// Emit a [`DebugEvent::LlmCall`] frame. Called at the end of every
/// completed provider call (streaming happy path, non-streaming
/// fallback path, and the compact-and-retry path).
fn emit_debug_llm_call(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    provider_name: &str,
    model_name: &str,
    response: &ModelResponse,
    duration_ms: u64,
) {
    let model = if response.trace.model.is_empty() {
        model_name.to_string()
    } else {
        response.trace.model.clone()
    };
    emit(
        event_tx,
        AgentLoopEvent::Debug(DebugEvent::LlmCall {
            timestamp: Utc::now(),
            provider: provider_name.to_string(),
            model,
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            duration_ms,
            task_id: None,
            agent_instance_id: None,
            request_id: {
                #[allow(deprecated)]
                response.trace.request_id()
            },
        }),
    );
}

/// Map a [`StreamEvent`] to the corresponding [`AgentLoopEvent`] and emit it.
fn emit_stream_event(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    stream_event: &StreamEvent,
    accumulator: &StreamAccumulator,
) {
    if event_tx.is_none() {
        return;
    }

    match stream_event {
        StreamEvent::TextDelta { text } => {
            emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
        }
        StreamEvent::ThinkingDelta { thinking } => {
            emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
        }
        StreamEvent::ContentBlockStart {
            content_type: StreamContentType::ToolUse { id, name },
            ..
        } => {
            emit(
                event_tx,
                AgentLoopEvent::ToolStart {
                    id: id.clone(),
                    name: name.clone(),
                },
            );
        }
        StreamEvent::InputJsonDelta { .. } => {
            if let Some(ref tool) = accumulator.current_tool_use {
                emit(
                    event_tx,
                    AgentLoopEvent::ToolInputSnapshot {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        input: tool.input_json.clone(),
                    },
                );
            }
        }
        StreamEvent::Error { message, .. } => {
            emit(
                event_tx,
                AgentLoopEvent::Error {
                    code: "stream_error".to_string(),
                    message: message.clone(),
                    recoverable: true,
                },
            );
        }
        _ => {}
    }
}

impl AgentLoop {
    /// Perform a model completion using streaming, emitting events as they arrive.
    ///
    /// Falls back to non-streaming `provider.complete()` only for mid-stream
    /// transport errors. Request-level failures (e.g. 4xx validation errors)
    /// are propagated directly — retrying with a different request format
    /// would not fix them and produces confusing double errors.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) async fn complete_with_streaming(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ModelResponse, LlmCallError> {
        let start = Instant::now();
        let provider_name = provider.name();
        let model_name = request.model.as_ref().to_string();

        let mut stream = provider
            .complete_streaming(request.clone())
            .await
            .map_err(|e| LlmCallError::from_reasoner_error(&e))?;

        let mut accumulator = StreamAccumulator::new();

        loop {
            let next = if let Some(token) = cancellation_token {
                tokio::select! {
                    () = token.cancelled() => {
                        return Err(LlmCallError::Fatal("Cancelled".to_string()));
                    }
                    item = stream.next() => item,
                }
            } else {
                stream.next().await
            };

            match next {
                Some(Ok(event)) => {
                    accumulator.process(&event);
                    emit_stream_event(event_tx, &event, &accumulator);
                }
                Some(Err(e)) => {
                    debug!("Stream error, falling back to non-streaming: {e}");
                    emit(
                        event_tx,
                        AgentLoopEvent::StreamReset {
                            reason: format!("Stream error, retrying without streaming: {e}"),
                        },
                    );
                    let fallback_start = Instant::now();
                    let response = provider
                        .complete(request)
                        .await
                        .map_err(|e| LlmCallError::from_reasoner_error(&e))?;
                    for block in &response.message.content {
                        match block {
                            aura_reasoner::ContentBlock::Text { text } => {
                                emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
                            }
                            aura_reasoner::ContentBlock::Thinking { thinking, .. } => {
                                emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
                            }
                            _ => {}
                        }
                    }
                    let duration_ms =
                        u64::try_from(fallback_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                    emit_debug_llm_call(
                        event_tx,
                        provider_name,
                        &model_name,
                        &response,
                        duration_ms,
                    );
                    return Ok(response);
                }
                None => break,
            }
        }

        let latency_ms = start.elapsed().as_millis() as u64;
        match accumulator.into_response(0, latency_ms) {
            Ok(response) => {
                emit_debug_llm_call(event_tx, provider_name, &model_name, &response, latency_ms);
                Ok(response)
            }
            Err(e) if stream_error_is_retryable(&e) => {
                // Upstream emitted a mid-stream SSE `error` event (HTTP 200
                // body, not an HTTP 5xx status). The SSE transport layer
                // has no retry of its own, so without this fallback a
                // single transient provider / proxy blip terminates the
                // whole turn. Re-issuing the call non-streaming routes
                // through `AnthropicProvider::complete`, which has a
                // full retry loop with exponential backoff for 429/529
                // and generic 5xx.
                tracing::warn!(
                    error = %e,
                    "Mid-stream SSE error looks transient; falling back to non-streaming"
                );
                emit(
                    event_tx,
                    AgentLoopEvent::StreamReset {
                        reason: format!(
                            "Mid-stream SSE error, retrying without streaming: {e}"
                        ),
                    },
                );
                let fallback_start = Instant::now();
                let response = provider
                    .complete(request)
                    .await
                    .map_err(|e| LlmCallError::from_reasoner_error(&e))?;
                for block in &response.message.content {
                    match block {
                        aura_reasoner::ContentBlock::Text { text } => {
                            emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
                        }
                        aura_reasoner::ContentBlock::Thinking { thinking, .. } => {
                            emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
                        }
                        _ => {}
                    }
                }
                let duration_ms =
                    u64::try_from(fallback_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                emit_debug_llm_call(
                    event_tx,
                    provider_name,
                    &model_name,
                    &response,
                    duration_ms,
                );
                Ok(response)
            }
            Err(e) => Err(LlmCallError::from_reasoner_error(&e)),
        }
    }
}

/// Decide whether an error produced by [`StreamAccumulator::into_response`]
/// warrants a fallback to the non-streaming `complete()` path.
///
/// We retry:
///   - mid-stream SSE `error` frames (surfaced as
///     `ReasonerError::Internal("stream terminated with error: ...")`),
///     including the Anthropic `overloaded_error` / `api_error` shapes
///     and the generic `Internal server error` proxies often inject;
///   - context-overflow signals — these are handled upstream but we
///     should not treat them as the "stream died" retry class.
///
/// We deliberately do NOT retry:
///   - `InsufficientCredits` (402): permanent.
///   - `RateLimited` (429/529): the provider has already retried with
///     backoff; the non-streaming path would just hit the same limit.
///   - `Parse` / `Request`: structural issues with the request; re-issuing
///     it verbatim is unlikely to fix anything.
fn stream_error_is_retryable(err: &ReasonerError) -> bool {
    if err.is_context_overflow() {
        return false;
    }
    match err {
        ReasonerError::InsufficientCredits(_)
        | ReasonerError::RateLimited(_)
        | ReasonerError::Timeout
        | ReasonerError::Parse(_)
        | ReasonerError::Request(_) => false,
        ReasonerError::Api { status, .. } => matches!(status, 500 | 502 | 503 | 504),
        ReasonerError::Internal(message) => looks_like_transient_stream_error(message),
    }
}

fn looks_like_transient_stream_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    // Match on the canonical prefix set by `StreamAccumulator::into_response`
    // plus the Anthropic / proxy error-type shapes that ride on top of it.
    lower.contains("stream terminated with error")
        || lower.contains("overloaded_error")
        || lower.contains("api_error")
        || lower.contains("internal server error")
        || lower.contains("bad gateway")
        || lower.contains("service unavailable")
        || lower.contains("gateway timeout")
}

#[cfg(test)]
mod retry_classifier_tests {
    use super::*;

    #[test]
    fn mid_stream_internal_server_error_is_retryable() {
        let err = ReasonerError::Internal(
            "stream terminated with error (model=claude-sonnet, msg_id=msg_01): Internal server error"
                .to_string(),
        );
        assert!(stream_error_is_retryable(&err));
    }

    #[test]
    fn anthropic_overloaded_error_frame_is_retryable() {
        let err = ReasonerError::Internal(
            "stream terminated with error: overloaded_error: service is overloaded".to_string(),
        );
        assert!(stream_error_is_retryable(&err));
    }

    #[test]
    fn insufficient_credits_is_not_retryable() {
        let err = ReasonerError::InsufficientCredits("402".to_string());
        assert!(!stream_error_is_retryable(&err));
    }

    #[test]
    fn context_overflow_is_not_retryable() {
        let err = ReasonerError::Api {
            status: 400,
            message: "prompt is too long".to_string(),
        };
        assert!(!stream_error_is_retryable(&err));
    }

    #[test]
    fn rate_limited_is_not_retryable_here() {
        let err = ReasonerError::RateLimited("429 too many requests".to_string());
        assert!(!stream_error_is_retryable(&err));
    }

    #[test]
    fn api_5xx_is_retryable() {
        assert!(stream_error_is_retryable(&ReasonerError::Api {
            status: 500,
            message: "upstream".to_string(),
        }));
        assert!(stream_error_is_retryable(&ReasonerError::Api {
            status: 502,
            message: "bad gateway".to_string(),
        }));
        assert!(!stream_error_is_retryable(&ReasonerError::Api {
            status: 400,
            message: "bad request".to_string(),
        }));
    }
}
