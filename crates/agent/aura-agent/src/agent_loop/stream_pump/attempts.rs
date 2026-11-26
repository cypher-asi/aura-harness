//! Retry bookkeeping for [`super::run_stream_pump`]: what the last failed
//! attempt left behind, the backoff before the next one, and the final
//! outcome once the retry budget is spent.
//!
//! Runs once per sampling attempt, not per streamed event.

use std::time::{Duration, Instant};

use aura_model::{ModelProvider, ModelRequest, ReasonerError, ResponseEventStream};
use tracing::{error, warn};

use crate::events::AgentLoopEvent;
use crate::AgentError;

use super::retry::{
    effort_after_cut, reset_reason, stream_retry_params, update_partial_retry_state,
    PartialRetryState,
};
use super::{emit_event, StreamPumpCtx, StreamPumpOutcome};

/// What opening the response stream produced.
pub(super) enum Opened {
    /// The stream is open; drive it.
    Stream(ResponseEventStream),
    /// The open failed retryably; go to the next attempt.
    Retry,
    /// Cancellation fired while opening.
    Cancelled,
    /// The open failed with a non-retryable error.
    Failed(ReasonerError),
}

/// State carried between the attempts of one sampling request.
pub(super) struct Attempts {
    pub(super) max_retries: u32,
    backoff_initial_ms: u64,
    backoff_cap_ms: u64,
    retry_state: Option<PartialRetryState>,
    transport_error: Option<AgentError>,
    reset_bytes: (usize, usize),
    /// How long the last failed attempt ran, and what ended it.
    last_cut: (Duration, String),
}

impl Attempts {
    pub(super) fn new() -> Self {
        let (max_retries, backoff_initial_ms, backoff_cap_ms) = stream_retry_params();
        Self {
            max_retries,
            backoff_initial_ms,
            backoff_cap_ms,
            retry_state: None,
            transport_error: None,
            reset_bytes: (0, 0),
            last_cut: (Duration::ZERO, String::new()),
        }
    }

    /// Announce the retry, back off, and reset the displayed response.
    /// `Err` carries the outcome the pump returns instead.
    pub(super) async fn prepare_retry(
        &mut self,
        ctx: StreamPumpCtx<'_>,
        attempt: u32,
        request: &mut ModelRequest,
    ) -> Result<(), StreamPumpOutcome> {
        if self.retry_state.is_none() && self.transport_error.is_none() {
            // Every retry must carry the preceding failure.
            // Reaching this arm without one is a partition
            // contract violation; surface it as a fatal
            // `LlmCallError` rather than panicking (Rule 4.1).
            return Err(StreamPumpOutcome::Error(AgentError::Reason(
                ReasonerError::Internal(
                    "stream retry requested without an interrupted request".to_string(),
                ),
            )));
        }
        let delay = aura_model::anthropic::exp_backoff_with_jitter(
            attempt - 1,
            self.backoff_initial_ms,
            self.backoff_cap_ms,
        );
        // Saturating conversion: backoff is bounded by `backoff_cap_ms`
        // (default 30s), so `delay.as_millis()` always fits in `u64`.
        // The `unwrap_or(u64::MAX)` is purely defensive against a
        // future cap raise.
        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        self.announce_retry(ctx, attempt, delay_ms);
        if let Some(token) = ctx.cancellation_token {
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(StreamPumpOutcome::Cancelled),
                () = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
            }
        } else {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        self.reset_stream(ctx, attempt, request);
        Ok(())
    }

    fn announce_retry(&self, ctx: StreamPumpCtx<'_>, attempt: u32, delay_ms: u64) {
        let max_retries = self.max_retries;
        if let Some(state) = self.retry_state.as_ref() {
            warn!(
                attempt,
                max_attempts = max_retries,
                delay_ms,
                tool_use_id = %state.tool_use_id,
                tool_name = %state.tool_name,
                reason = %state.reason,
                "Per-tool-call streaming retry scheduled after pump stream abort"
            );
            emit_event(
                ctx.event_tx,
                AgentLoopEvent::ToolCallRetrying {
                    tool_use_id: state.tool_use_id.clone(),
                    tool_name: state.tool_name.clone(),
                    attempt,
                    max_attempts: max_retries,
                    delay_ms,
                    reason: state.reason.clone(),
                },
            );
        } else if let Some(err) = self.transport_error.as_ref() {
            warn!(attempt, max_retries, delay_ms, error = %err, "Retrying interrupted model stream");
            emit_event(
                ctx.event_tx,
                AgentLoopEvent::Progress {
                    stage: "model_retrying".to_string(),
                    tool_name: None,
                    elapsed_ms: None,
                    message: Some(format!(
                        "Connection interrupted; retrying ({attempt}/{max_retries})"
                    )),
                },
            );
        }
    }

    fn reset_stream(&mut self, ctx: StreamPumpCtx<'_>, attempt: u32, request: &mut ModelRequest) {
        let (cut_after, error) = std::mem::take(&mut self.last_cut);
        let lowered = effort_after_cut(request.thinking_effort, cut_after);
        if lowered.is_some() {
            request.thinking_effort = lowered;
        }
        let reason = reset_reason(cut_after, &error, lowered);
        warn!(attempt, %reason, "Restarting the model response");
        // Discard deltas from the failed attempt before displaying the
        // fresh response. Never splice two independent generations.
        emit_event(
            ctx.event_tx,
            AgentLoopEvent::StreamReset {
                reason,
                text_bytes: self.reset_bytes.0,
                thinking_bytes: self.reset_bytes.1,
            },
        );
    }

    /// Open the response stream for one attempt, recording a retryable
    /// failure.
    pub(super) async fn open(
        &mut self,
        ctx: StreamPumpCtx<'_>,
        provider: &dyn ModelProvider,
        request: &ModelRequest,
        attempt_started: Instant,
    ) -> Opened {
        let opened = if let Some(token) = ctx.cancellation_token {
            tokio::select! {
                biased;
                () = token.cancelled() => return Opened::Cancelled,
                opened = provider.complete_response_stream(request.clone()) => opened,
            }
        } else {
            provider.complete_response_stream(request.clone()).await
        };
        match opened {
            Ok(s) => Opened::Stream(s),
            Err(ReasonerError::StreamAbortedWithPartial {
                reason,
                partial_tool_use,
            }) => {
                self.record_partial((0, 0), attempt_started, reason, partial_tool_use);
                Opened::Retry
            }
            Err(err @ (ReasonerError::Request(_) | ReasonerError::Timeout)) => {
                self.record_transport((0, 0), attempt_started, AgentError::Reason(err));
                Opened::Retry
            }
            Err(err) => Opened::Failed(err),
        }
    }

    /// Record a stream that aborted inside a `tool_use` block.
    fn record_partial(
        &mut self,
        reset_bytes: (usize, usize),
        attempt_started: Instant,
        reason: String,
        partial_tool_use: Option<aura_model::PartialToolUse>,
    ) {
        self.reset_bytes = reset_bytes;
        self.transport_error = None;
        self.last_cut = (attempt_started.elapsed(), reason.clone());
        self.retry_state = Some(update_partial_retry_state(
            self.retry_state.take(),
            reason,
            partial_tool_use,
        ));
    }

    /// Record a retryable connection failure.
    fn record_transport(
        &mut self,
        reset_bytes: (usize, usize),
        attempt_started: Instant,
        err: AgentError,
    ) {
        self.reset_bytes = reset_bytes;
        self.retry_state = None;
        self.last_cut = (attempt_started.elapsed(), err.to_string());
        self.transport_error = Some(err);
    }

    /// Fold a driven attempt's outcome: `None` retries, `Some` is final.
    pub(super) fn settle(
        &mut self,
        ctx: StreamPumpCtx<'_>,
        provider: &dyn ModelProvider,
        model_name: &str,
        attempt_started: Instant,
        outcome: StreamPumpOutcome,
    ) -> Option<StreamPumpOutcome> {
        match outcome {
            StreamPumpOutcome::AbortedWithPartial {
                reason,
                partial_tool_use,
                text_bytes,
                thinking_bytes,
            } => {
                let bytes = (text_bytes, thinking_bytes);
                self.record_partial(bytes, attempt_started, reason, partial_tool_use);
                None
            }
            StreamPumpOutcome::RetryableTransport {
                error: err,
                text_bytes,
                thinking_bytes,
            } => {
                let bytes = (text_bytes, thinking_bytes);
                self.record_transport(bytes, attempt_started, err);
                None
            }
            StreamPumpOutcome::Completed {
                response,
                tool_results,
            } => {
                let duration_ms =
                    u64::try_from(attempt_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                super::super::streaming::emit_debug_llm_call(
                    ctx.event_tx,
                    provider.name(),
                    model_name,
                    &response,
                    duration_ms,
                );
                Some(StreamPumpOutcome::Completed {
                    response,
                    tool_results,
                })
            }
            other => Some(other),
        }
    }

    /// The outcome once every attempt failed.
    pub(super) fn exhausted(self, ctx: StreamPumpCtx<'_>) -> StreamPumpOutcome {
        if let Some(err) = self.transport_error {
            return StreamPumpOutcome::Error(err);
        }
        let state = self.retry_state.unwrap_or_else(|| PartialRetryState {
            tool_use_id: "<unknown>".to_string(),
            tool_name: "<unknown>".to_string(),
            reason: "stream aborted while tool_use was in flight".to_string(),
        });
        error!(
            attempts = self.max_retries,
            tool_use_id = %state.tool_use_id,
            tool_name = %state.tool_name,
            reason = %state.reason,
            "Per-tool-call streaming retry budget exhausted in pump; giving up"
        );
        emit_event(
            ctx.event_tx,
            AgentLoopEvent::ToolCallFailed {
                tool_use_id: state.tool_use_id,
                tool_name: state.tool_name,
                reason: state.reason.clone(),
            },
        );
        StreamPumpOutcome::Error(AgentError::Reason(
            ReasonerError::StreamAbortedWithPartial {
                reason: state.reason,
                partial_tool_use: None,
            },
        ))
    }
}
