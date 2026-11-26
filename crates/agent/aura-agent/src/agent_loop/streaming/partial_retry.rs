//! Per-tool-call streaming retry loop.
//!
//! Re-drives `provider.complete_streaming` after a mid-stream abort that
//! carried a [`PartialToolUse`], so the in-flight tool call is preserved
//! across the retry instead of being dropped by the non-streaming fallback.

use std::time::{Duration, Instant};

use aura_model::anthropic::exp_backoff_with_jitter;
use aura_model::{
    ModelProvider, ModelRequest, ModelResponse, PartialToolUse, ReasonerError, StreamAccumulator,
    StreamEventStream,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use crate::agent_loop::iteration::LlmCallError;
use crate::agent_loop::AgentLoop;
use crate::events::AgentLoopEvent;

use super::drain::{drain_remaining_stream, print_sse_transport_failure, DrainOutcome};
use super::emit::{emit, emit_debug_llm_call};

/// Tool identity and last abort reason carried across retry attempts.
struct RetryTarget {
    tool_use_id: String,
    tool_name: String,
    last_reason: String,
}

impl RetryTarget {
    /// Preserve tool identity across retries: once a stream starts
    /// and dies before `content_block_start`, subsequent retries may
    /// not carry any partial at all — but the UI still benefits from
    /// seeing the original tool name if we had one.
    fn new(initial_reason: String, initial_partial: Option<&PartialToolUse>) -> Self {
        let tool_use_id =
            initial_partial.map_or_else(|| "<unknown>".to_string(), |p| p.tool_use_id.clone());
        let tool_name =
            initial_partial.map_or_else(|| "<unknown>".to_string(), |p| p.tool_name.clone());
        Self {
            tool_use_id,
            tool_name,
            last_reason: initial_reason,
        }
    }

    /// Record a further mid-stream abort, updating the tool identity
    /// when the new attempt carried a partial.
    fn record_abort(&mut self, reason: String, partial_tool_use: Option<PartialToolUse>) {
        if let Some(p) = partial_tool_use {
            self.tool_use_id = p.tool_use_id;
            self.tool_name = p.tool_name;
        }
        self.last_reason = reason;
    }

    fn announce_retry(
        &self,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
    ) {
        // Emit a tracing record per retry so operators see the
        // storm in `aura-node` logs. Without this, a flaky
        // upstream looks like N identical request bodies with no
        // context between them (see `fix-silent-stream-retry-storm`
        // plan: the original UX bug was a ~28 second invisible
        // wait while 7 paid streaming requests fired).
        warn!(
            attempt,
            max_attempts = max_retries,
            delay_ms,
            tool_use_id = %self.tool_use_id,
            tool_name = %self.tool_name,
            reason = %self.last_reason,
            "Per-tool-call streaming retry scheduled after mid-stream abort"
        );

        emit(
            event_tx,
            AgentLoopEvent::ToolCallRetrying {
                tool_use_id: self.tool_use_id.clone(),
                tool_name: self.tool_name.clone(),
                attempt,
                max_attempts: max_retries,
                delay_ms,
                reason: self.last_reason.clone(),
            },
        );
    }

    fn give_up(
        self,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        max_retries: u32,
        last_err: Option<LlmCallError>,
    ) -> LlmCallError {
        error!(
            attempts = max_retries,
            tool_use_id = %self.tool_use_id,
            tool_name = %self.tool_name,
            reason = %self.last_reason,
            "Per-tool-call streaming retry budget exhausted; giving up"
        );

        emit(
            event_tx,
            AgentLoopEvent::ToolCallFailed {
                tool_use_id: self.tool_use_id,
                tool_name: self.tool_name,
                reason: self.last_reason.clone(),
            },
        );

        last_err.unwrap_or_else(|| LlmCallError::Fatal(self.last_reason.clone()))
    }
}

/// Sleep for the backoff delay. Honours cancellation during the backoff
/// so a Ctrl-C doesn't have to wait out a 30s cap.
async fn backoff_or_cancel(
    cancellation_token: Option<&CancellationToken>,
    delay_ms: u64,
) -> Result<(), LlmCallError> {
    if let Some(token) = cancellation_token {
        tokio::select! {
            () = token.cancelled() => {
                return Err(LlmCallError::Fatal("Cancelled".to_string()));
            }
            () = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
        }
    } else {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    Ok(())
}

impl AgentLoop {
    /// Re-drive `provider.complete_streaming` after a mid-stream abort
    /// that carried a [`PartialToolUse`]. Emits
    /// [`AgentLoopEvent::ToolCallRetrying`] before every sleep and
    /// [`AgentLoopEvent::ToolCallFailed`] when the retry budget is
    /// exhausted. Returns the first successful `ModelResponse` or the
    /// final error classification.
    pub(super) async fn retry_streaming_for_partial_tool_use(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
        initial_reason: String,
        initial_partial: Option<PartialToolUse>,
    ) -> Result<ModelResponse, LlmCallError> {
        // Shared with `agent_loop::stream_pump::run_stream_pump` so
        // both retry surfaces honour the same `AURA_LLM_MAX_RETRIES` /
        // `AURA_LLM_BACKOFF_INITIAL_MS` / `AURA_LLM_BACKOFF_CAP_MS`
        // tuning (Phase 3 dedup).
        let (max_retries, backoff_initial_ms, backoff_cap_ms) =
            crate::agent_loop::stream_pump::stream_retry_params();
        let mut target = RetryTarget::new(initial_reason, initial_partial.as_ref());
        let mut last_err: Option<LlmCallError> = None;

        for attempt in 1..=max_retries {
            let delay = exp_backoff_with_jitter(attempt - 1, backoff_initial_ms, backoff_cap_ms);
            let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
            target.announce_retry(event_tx, attempt, max_retries, delay_ms);
            backoff_or_cancel(cancellation_token, delay_ms).await?;

            match self
                .drive_streaming_once(provider, request.clone(), event_tx, cancellation_token)
                .await
            {
                Ok(response) => return Ok(response),
                Err(DriveStreamingError::AbortedWithPartial {
                    reason,
                    partial_tool_use,
                }) => {
                    target.record_abort(reason, partial_tool_use);
                    // fall through: loop continues to next retry
                }
                Err(DriveStreamingError::Other(err)) => {
                    last_err = Some(err);
                    break;
                }
            }
        }

        Err(target.give_up(event_tx, max_retries, last_err))
    }

    /// Drive a single streaming attempt (one `complete_streaming` +
    /// accumulation + classification). Used by the retry loop above so
    /// we can cleanly distinguish "retryable mid-stream abort" from
    /// "other fatal error".
    async fn drive_streaming_once(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
        event_tx: Option<&Sender<AgentLoopEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ModelResponse, DriveStreamingError> {
        let start = std::time::Instant::now();
        let provider_name = provider.name();
        let model_name = request.model.as_ref().to_string();

        let stream = provider
            .complete_streaming(request)
            .await
            .map_err(|e| DriveStreamingError::Other(LlmCallError::from_reasoner_error(&e)))?;
        let accumulator = drain_retry_attempt(stream, event_tx, cancellation_token, start).await?;
        classify_retry_attempt(accumulator, event_tx, start, provider_name, &model_name)
    }
}

/// Drain one retry attempt's stream into an accumulator.
async fn drain_retry_attempt(
    stream: StreamEventStream,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    cancellation_token: Option<&CancellationToken>,
    start: Instant,
) -> Result<StreamAccumulator, DriveStreamingError> {
    match drain_remaining_stream(stream, event_tx, cancellation_token).await {
        DrainOutcome::Completed(acc) => Ok(*acc),
        DrainOutcome::Cancelled => Err(DriveStreamingError::Other(LlmCallError::Fatal(
            "Cancelled".to_string(),
        ))),
        // Transport-level error on the retry attempt. Don't recurse
        // into another non-streaming fallback here — the outer retry
        // loop will decide whether to try again based on
        // classification.
        DrainOutcome::Transport(e) => {
            print_sse_transport_failure(&e, start);
            Err(DriveStreamingError::Other(
                LlmCallError::from_reasoner_error(&e),
            ))
        }
    }
}

/// Classify a drained retry attempt into a response, a further
/// mid-stream abort, or a fatal error.
fn classify_retry_attempt(
    accumulator: StreamAccumulator,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    start: Instant,
    provider_name: &str,
    model_name: &str,
) -> Result<ModelResponse, DriveStreamingError> {
    let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match accumulator.into_response(0, latency_ms) {
        Ok(response) => {
            aura_model::console::emit_response_block(&response, latency_ms, 200, "OK");
            emit_debug_llm_call(event_tx, provider_name, model_name, &response, latency_ms);
            Ok(response)
        }
        Err(ReasonerError::StreamAbortedWithPartial {
            reason,
            partial_tool_use,
        }) => Err(DriveStreamingError::AbortedWithPartial {
            reason,
            partial_tool_use,
        }),
        Err(e) => Err(DriveStreamingError::Other(
            LlmCallError::from_reasoner_error(&e),
        )),
    }
}

/// Internal classification for a single streaming attempt driven by
/// [`AgentLoop::drive_streaming_once`]. Callers match on this to decide
/// whether the attempt should be retried (AbortedWithPartial) or
/// propagated as-is (Other).
enum DriveStreamingError {
    AbortedWithPartial {
        reason: String,
        partial_tool_use: Option<PartialToolUse>,
    },
    Other(LlmCallError),
}
