//! Streaming model calls and event emission.

use std::time::Instant;

use aura_model::{ModelProvider, ModelRequest, ModelResponse, ReasonerError, StreamAccumulator};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;

use super::iteration::LlmCallError;
use super::AgentLoop;

mod drain;
mod emit;
mod partial_retry;

use drain::{
    complete_and_emit_as_deltas, drain_remaining_stream, report_stream_transport_error,
    DrainOutcome,
};
pub(super) use emit::{emit, emit_debug_llm_call, emit_iteration_complete};

/// Phase 8 bundle of the borrowed coordination state threaded
/// through [`AgentLoop::finalize_after_stream`].
///
/// The post-pump finalisation step needs five borrows that all
/// flow from the same `complete_with_streaming` caller (the optional
/// event sink, the optional cancellation token, the per-call start
/// timestamp, and the provider/model name pair used for tracing
/// and telemetry). Bundling them into one struct keeps
/// `finalize_after_stream` under the clippy `too_many_arguments`
/// ceiling without diluting the per-step argument list with
/// "ambient" identifiers.
pub(super) struct FinalizeStreamCtx<'a> {
    pub(super) event_tx: Option<&'a Sender<AgentLoopEvent>>,
    pub(super) cancellation_token: Option<&'a CancellationToken>,
    pub(super) start: Instant,
    pub(super) provider_name: &'a str,
    pub(super) model_name: &'a str,
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

        let stream = provider
            .complete_streaming(request.clone())
            .await
            .map_err(|e| LlmCallError::from_reasoner_error(&e))?;

        match drain_remaining_stream(stream, event_tx, cancellation_token).await {
            DrainOutcome::Cancelled => Err(LlmCallError::Fatal("Cancelled".to_string())),
            DrainOutcome::Transport(e) => {
                report_stream_transport_error(&e, event_tx, start, provider_name, &model_name);
                complete_and_emit_as_deltas(provider, request, event_tx, provider_name, &model_name)
                    .await
            }
            DrainOutcome::Completed(accumulator) => {
                self.finalize_after_stream(
                    provider,
                    request,
                    *accumulator,
                    FinalizeStreamCtx {
                        event_tx,
                        cancellation_token,
                        start,
                        provider_name,
                        model_name: &model_name,
                    },
                )
                .await
            }
        }
    }

    /// Classify the post-pump accumulator into the next action.
    ///
    /// Three buckets:
    /// 1. `Ok(response)` — emit the debug-llm-call frame and return.
    /// 2. `Err(StreamAbortedWithPartial)` — re-issue the streaming
    ///    request via [`Self::retry_streaming_for_partial_tool_use`]
    ///    so the in-flight tool call is preserved across the retry.
    /// 3. `Err(retryable)` — fall back to the non-streaming
    ///    `provider.complete()` path via [`complete_and_emit_as_deltas`].
    /// 4. Anything else — propagate as a fatal LLM call error.
    async fn finalize_after_stream(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        accumulator: StreamAccumulator,
        ctx: FinalizeStreamCtx<'_>,
    ) -> Result<ModelResponse, LlmCallError> {
        let FinalizeStreamCtx {
            event_tx,
            cancellation_token,
            start,
            provider_name,
            model_name,
        } = ctx;
        // The streaming pump's per-iteration latency is driven by the
        // wall-clock since the pump kicked off, capped at `u64::MAX`
        // ms (the pump never runs longer than that). The cap is
        // documented and the truncation is benign — `latency_ms` is
        // observability-only.
        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;
        match accumulator.into_response(0, latency_ms) {
            Ok(response) => {
                aura_model::console::emit_response_block(&response, latency_ms, 200, "OK");
                emit_debug_llm_call(event_tx, provider_name, model_name, &response, latency_ms);
                Ok(response)
            }
            Err(ReasonerError::StreamAbortedWithPartial {
                reason,
                partial_tool_use,
            }) => {
                // Per-tool-call streaming retry: re-issue a fresh
                // streaming request (up to N attempts) instead of
                // dropping down to the non-streaming fallback, which
                // has no memory of the in-flight tool call and would
                // effectively drop the Write/Edit the model was
                // mid-way through. Matches `fix_4.6-class_failures`
                // plan § `harness-retry-streaming`.
                self.retry_streaming_for_partial_tool_use(
                    provider,
                    request,
                    event_tx,
                    cancellation_token,
                    reason,
                    partial_tool_use,
                )
                .await
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
                        reason: format!("Mid-stream SSE error, retrying without streaming: {e}"),
                        text_bytes: 0,
                        thinking_bytes: 0,
                    },
                );
                complete_and_emit_as_deltas(provider, request, event_tx, provider_name, model_name)
                    .await
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
        | ReasonerError::RateLimited { .. }
        | ReasonerError::Timeout
        | ReasonerError::Parse(_)
        | ReasonerError::Request(_) => false,
        // `Transient` is the structured "retryable upstream 5xx"
        // classification — by definition retryable.
        ReasonerError::Transient { .. } => true,
        ReasonerError::Api { status, .. } => matches!(status, 500 | 502 | 503 | 504),
        ReasonerError::Internal(message) => looks_like_transient_stream_error(message),
        // `StreamAbortedWithPartial` is handled by the dedicated
        // retry loop in `complete_with_streaming` above, not by
        // the legacy non-streaming fallback classifier. Returning
        // `true` here would cause a redundant second fallback
        // path; returning `false` keeps responsibility where it
        // belongs.
        ReasonerError::StreamAbortedWithPartial { .. }
        | ReasonerError::ModelRequestContractViolation(_) => false,
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
mod retry_classifier_tests;
