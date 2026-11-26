//! Kernel-recorded streaming adapter.
//!
//! [`RecordingStream`] wraps a `StreamEventStream` together with an owned
//! [`ReasonStreamHandle`] and guarantees that Invariant §3 ("every LLM
//! call is recorded") holds even for streaming completions:
//!
//! - on natural end of stream (`MessageStop` or the underlying stream
//!   yielding `None`) the handle is consumed by
//!   [`ReasonStreamHandle::record_completed`];
//! - on any `StreamEvent::Error` or transport `Err(_)` item the handle is
//!   consumed by [`ReasonStreamHandle::record_failed`];
//! - if the `RecordingStream` is dropped before either of the above
//!   fires, the `Drop` impl finalizes the handle with
//!   `record_failed("stream dropped before completion")`.
//!
//! The underlying handle is consumed by value on every finalization path,
//! so double-recording is impossible by construction.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use aura_agent_kernel::ReasonStreamHandle;
use aura_model::{ReasonerError, StreamAccumulator, StreamEvent, StreamEventStream};
use futures_util::Stream;

/// Reason string used when `RecordingStream` is dropped before any
/// terminal event (success, error, or end-of-stream) fires. Tests rely
/// on the exact value to detect mid-stream cancellation.
pub(crate) const STREAM_DROPPED_REASON: &str = "stream dropped before completion";

/// A stream wrapper that records the outcome of a streaming model call
/// through a [`ReasonStreamHandle`] exactly once.
pub(crate) struct RecordingStream {
    inner: StreamEventStream,
    handle: Option<ReasonStreamHandle>,
    accumulator: StreamAccumulator,
    /// First and latest thinking delta, to record how long the model thought
    thinking_span: Option<(Instant, Instant)>,
}

impl RecordingStream {
    pub(crate) fn new(inner: StreamEventStream, handle: ReasonStreamHandle) -> Self {
        Self {
            inner,
            handle: Some(handle),
            accumulator: StreamAccumulator::new(),
            thinking_span: None,
        }
    }

    fn finalize_completed(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let tool_uses: Vec<String> = self
            .accumulator
            .tool_uses
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let model = if self.accumulator.model.is_empty() {
            "unknown".to_string()
        } else {
            self.accumulator.model.clone()
        };
        let stop_reason = self
            .accumulator
            .stop_reason
            .unwrap_or(aura_model::StopReason::EndTurn);
        let usage = aura_model::Usage {
            input_tokens: self.accumulator.input_tokens,
            output_tokens: self.accumulator.output_tokens,
            cache_creation_input_tokens: self.accumulator.cache_creation_input_tokens,
            cache_read_input_tokens: self.accumulator.cache_read_input_tokens,
        };

        let thinking_ms = self.thinking_span.map(|(first, last)| {
            u64::try_from(last.duration_since(first).as_millis()).unwrap_or(u64::MAX)
        });

        let handle = handle.with_plan_usage(self.accumulator.plan_usage.clone());
        if let Err(e) = handle.record_completed(
            &model,
            stop_reason,
            &usage,
            &tool_uses,
            &self.accumulator.thinking_content,
            thinking_ms,
        ) {
            tracing::warn!(error = %e, "RecordingStream failed to record completed reasoning entry");
        }
    }

    fn finalize_failed(&mut self, reason: &str) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        if let Err(e) = handle.record_failed(reason) {
            tracing::warn!(
                error = %e,
                reason,
                "RecordingStream failed to record failed reasoning entry"
            );
        }
    }
}

impl Stream for RecordingStream {
    type Item = Result<StreamEvent, ReasonerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: structural pinning for `inner` only; all other fields
        // are either `Unpin` (`Option<ReasonStreamHandle>`,
        // `StreamAccumulator`) or accessed without moving.
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                this.accumulator.process(&event);
                if matches!(event, StreamEvent::ThinkingDelta { .. }) {
                    let now = Instant::now();
                    let first = this.thinking_span.map_or(now, |(first, _)| first);
                    this.thinking_span = Some((first, now));
                }
                match &event {
                    StreamEvent::MessageStop => {
                        this.finalize_completed();
                    }
                    StreamEvent::Error { message, .. } => {
                        let reason = message.clone();
                        this.finalize_failed(&reason);
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(err))) => {
                let reason = err.to_string();
                this.finalize_failed(&reason);
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                // Natural end without an explicit `MessageStop` — still
                // finalize so Invariant §3 holds.
                if this.handle.is_some() {
                    this.finalize_completed();
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for RecordingStream {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.finalize_failed(STREAM_DROPPED_REASON);
        }
    }
}

#[cfg(test)]
mod tests;
