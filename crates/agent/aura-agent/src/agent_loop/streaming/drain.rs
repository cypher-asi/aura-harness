//! Stream draining and the non-streaming fallback path.

use std::time::Instant;

use aura_model::{
    ModelProvider, ModelRequest, ModelResponse, ReasonerError, StreamAccumulator, StreamEventStream,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::agent_loop::iteration::LlmCallError;
use crate::events::AgentLoopEvent;

use super::emit::{emit, emit_debug_llm_call, emit_stream_event};

/// Outcome of [`drain_remaining_stream`].
///
/// The pump separates "the stream finished cleanly" (the caller now
/// classifies the accumulated response) from "transport blew up" (the
/// caller may want to fall back to non-streaming) and "the user
/// cancelled" (terminal). Returning a typed outcome instead of a
/// `Result<(), Err>` keeps the per-attempt loop callers (the main
/// `complete_with_streaming` orchestrator and the per-tool-call retry
/// path in [`AgentLoop::drive_streaming_once`]) sharing one
/// definition without forcing them to share a single error type.
pub(super) enum DrainOutcome {
    /// Boxed because [`StreamAccumulator`] is ~320 bytes (vs 96 for
    /// `Transport(ReasonerError)`); the large-variant clippy lint
    /// fires hard for an enum dispatched as a single `match` per
    /// attempt. Indirection here is essentially free — the accumulator
    /// is consumed once via `*acc` at the call site.
    Completed(Box<StreamAccumulator>),
    Cancelled,
    Transport(ReasonerError),
}

/// Pump every event from `stream` into a [`StreamAccumulator`], emitting
/// the corresponding wire deltas as they arrive.
///
/// Honours `cancellation_token` so a Ctrl-C does not have to wait out
/// the full stream. Returns the populated accumulator on clean stream
/// termination; transport errors and cancellation surface as their own
/// [`DrainOutcome`] variants so callers can decide whether to fall
/// back to the non-streaming path or propagate the failure.
pub(super) async fn drain_remaining_stream(
    mut stream: StreamEventStream,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    cancellation_token: Option<&CancellationToken>,
) -> DrainOutcome {
    let mut accumulator = StreamAccumulator::new();
    let mut last_snapshot = None;

    loop {
        let next = if let Some(token) = cancellation_token {
            tokio::select! {
                () = token.cancelled() => {
                    return DrainOutcome::Cancelled;
                }
                item = stream.next() => item,
            }
        } else {
            stream.next().await
        };

        match next {
            Some(Ok(event)) => {
                accumulator.process(&event);
                emit_stream_event(event_tx, &event, &accumulator, &mut last_snapshot);
            }
            Some(Err(e)) => return DrainOutcome::Transport(e),
            None => return DrainOutcome::Completed(Box::new(accumulator)),
        }
    }
}

/// Run the non-streaming `provider.complete()` path and replay every
/// textual content block as a delta event.
///
/// Used by both fallback branches in [`AgentLoop::complete_with_streaming`]:
/// the transport-error fallback (mid-stream connection blew up before any
/// usable event arrived) and the mid-stream-SSE-error fallback (stream
/// terminated with a retryable `error` frame). Consumers see incremental
/// deltas even though the request was buffered server-side.
pub(super) async fn complete_and_emit_as_deltas(
    provider: &dyn ModelProvider,
    request: ModelRequest,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    provider_name: &str,
    model_name: &str,
) -> Result<ModelResponse, LlmCallError> {
    let fallback_start = Instant::now();
    let response = provider
        .complete(request)
        .await
        .map_err(|e| LlmCallError::from_reasoner_error(&e))?;
    for block in &response.message.content {
        match block {
            aura_model::ContentBlock::Text { text } => {
                emit(event_tx, AgentLoopEvent::TextDelta(text.clone()));
            }
            aura_model::ContentBlock::Thinking { thinking, .. } => {
                emit(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
            }
            _ => {}
        }
    }
    let duration_ms = u64::try_from(fallback_start.elapsed().as_millis()).unwrap_or(u64::MAX);
    emit_debug_llm_call(event_tx, provider_name, model_name, &response, duration_ms);
    Ok(response)
}

/// Log, print and announce (as [`AgentLoopEvent::StreamReset`]) a
/// mid-stream transport error before the non-streaming fallback.
pub(super) fn report_stream_transport_error(
    e: &ReasonerError,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    start: Instant,
    provider_name: &str,
    model_name: &str,
) {
    warn!(
        error = %e,
        provider = %provider_name,
        model = %model_name,
        "Stream transport error; falling back to non-streaming complete()"
    );
    print_sse_transport_failure(e, start);
    emit(
        event_tx,
        AgentLoopEvent::StreamReset {
            reason: format!("Stream error, retrying without streaming: {e}"),
            text_bytes: 0,
            thinking_bytes: 0,
        },
    );
}

/// Print the console failure block for a mid-stream SSE transport error.
pub(super) fn print_sse_transport_failure(e: &ReasonerError, start: Instant) {
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let err_str = e.to_string();
    aura_model::console::anthropic_failure_block(&aura_model::console::AnthropicFailureView {
        status_code: Some(200),
        status_text: "OK",
        class: "sse_transport",
        elapsed_ms,
        request_id: None,
        retry_after_s: None,
        body_preview: Some(&err_str),
        destination: "aura-network",
    });
}
