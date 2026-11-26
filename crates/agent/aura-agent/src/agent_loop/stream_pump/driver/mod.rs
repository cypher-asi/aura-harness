//! Inner stream driver: event-loop body and in-flight tool drain.
//!
//! Separated from the public [`super::run_stream_pump`] entry so the
//! unit tests can hand it a hand-rolled `ResponseEventStream` without
//! a real `ModelProvider`. The retry/backoff envelope that wraps a
//! sequence of `drive_stream` attempts lives one level up in
//! [`super`].
//!
//! Layer E.4: takes an optional `event_tx` and emits the same event
//! surface as the legacy `streaming::emit_stream_event` path
//! (`TextDelta` / `ThinkingDelta` / `ToolStart` / `ToolInputSnapshot`).
//! Text and thinking stream per delta. A tool call is announced with
//! `ToolStart` when its block opens and its partial arguments follow as
//! throttled `ToolInputSnapshot`s, so a long tool input (a whole file
//! for `write_file`) is visible while it streams. The executable call
//! still waits for the block to close.

mod cancel;
mod drain;
mod events;
mod tool_dispatch;

use std::ops::ControlFlow;
use std::pin::Pin;
use std::time::{Duration, Instant};

use aura_model::{ResponseEvent, ResponseEventStream, StopReason, StreamError, StreamPhase, Usage};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::console;
use crate::events::AgentLoopEvent;
use crate::types::{ToolCallInfo, ToolCallResult};
use crate::AgentError;

use super::{emit_event, StreamPumpOutcome};

/// Shortest gap between two `ToolInputSnapshot`s of a streaming tool
/// call. Each snapshot carries the whole input so far, so one per wire
/// delta would copy a large `write_file` body thousands of times.
pub(in crate::agent_loop) const TOOL_INPUT_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);

/// The `tool_use` block currently streaming, announced to the client
/// with `ToolStart` but not yet closed.
struct OpenToolCall {
    id: String,
    name: String,
    input_json: String,
    last_snapshot: tokio::time::Instant,
}

/// Liveness/phase trail for one stream. `started` stamps the `t+` clock
/// on phase-transition lines, and `logged_phase` debounces the trail so
/// only an actual phase change emits a line (not every delta).
///
/// C2: a long, content-free time-to-first-token (or a long thinking /
/// tool-input phase) used to be consumed silently. Each content-free
/// frame now becomes throttled `model_thinking` progress so the aura-os
/// sliding-idle watchdog (`AURA_TURN_MAX_TIMEOUT_SECS`) keeps resetting
/// during slow model turns, without one event per ping.
struct Liveness {
    started: Instant,
    logged_phase: Option<StreamPhase>,
    heartbeat: Duration,
    last_progress: Instant,
}

impl Liveness {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            logged_phase: None,
            heartbeat: super::super::tool_pipeline::tool_heartbeat_interval(),
            last_progress: Instant::now(),
        }
    }

    /// Record a frame in `phase`. Pings are pure liveness and stay off
    /// the transcript.
    fn beat(
        &mut self,
        phase: StreamPhase,
        event_tx: Option<&tokio::sync::mpsc::Sender<AgentLoopEvent>>,
    ) {
        if phase != StreamPhase::Ping && self.logged_phase != Some(phase) {
            console::stream_phase_line(phase, self.started.elapsed());
            self.logged_phase = Some(phase);
        }
        if self.last_progress.elapsed() >= self.heartbeat {
            let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            emit_event(
                event_tx,
                AgentLoopEvent::Progress {
                    stage: "model_thinking".to_string(),
                    tool_name: None,
                    elapsed_ms: Some(elapsed_ms),
                    message: None,
                },
            );
            self.last_progress = Instant::now();
        }
    }
}

/// One finished call: its submission index, the call and its result.
type IndexedResult = (usize, (ToolCallInfo, ToolCallResult));

/// Boxed per-tool future spawned into [`FuturesUnordered`] inside the
/// pump. Carries the submission index and the originating
/// [`ToolCallInfo`] alongside the result so the drain loop can
/// reassemble submission order without an auxiliary lookup.
type ToolFuture<'a> = Pin<Box<dyn std::future::Future<Output = IndexedResult> + Send + 'a>>;

/// Drive a single `ResponseEventStream` to terminal completion.
///
/// One attempt of the retry envelope in [`super::run_stream_pump`].
/// Returns whichever [`StreamPumpOutcome`] terminated the loop body:
/// `Completed` on a clean `Completed` event, `Cancelled` /
/// `AbortedWithPartial` / `Error` on the matching short-circuit
/// arms. Per the module-level invariants in [`super`], no
/// state.messages mutation happens on the cancel or error arms.
pub(super) async fn drive_stream(
    ctx: super::StreamPumpCtx<'_>,
    mut stream: ResponseEventStream,
    state: &mut super::super::LoopState,
    model_name: &str,
) -> StreamPumpOutcome {
    let mut drive = StreamDrive::new(ctx);
    let stop_reason = match drive.pump_events(&mut stream, state, model_name).await {
        Ok(stop_reason) => stop_reason,
        Err(outcome) => return outcome,
    };
    let response = drive.synthesize(stop_reason, model_name);

    // While draining potentially long-running tool calls, emit a
    // periodic `tool_running` progress heartbeat. The spawned tool
    // futures only make progress as `FuturesUnordered` is polled in the
    // drain loop, so this window is the main silent stretch of a
    // turn. The heartbeat keeps the aura-os server's sliding-idle
    // watchdog (`AURA_TURN_MAX_TIMEOUT_SECS`) resetting so only a
    // genuinely hung tool trips `turn_timeout`. No-op when there are no
    // tools (`tool_calls_seen` empty) or no event channel. The guard
    // aborts the ticker as soon as the drain loop ends.
    let _tool_heartbeat = super::super::tool_pipeline::spawn_tool_heartbeat(
        ctx.event_tx,
        &drive.tool_calls_seen,
        super::super::tool_pipeline::tool_heartbeat_interval(),
    );
    let spawned_pairs = match drive.drain_in_flight(state, model_name).await {
        Ok(pairs) => pairs,
        Err(outcome) => return outcome,
    };
    drive.finish(state, response, spawned_pairs)
}

/// Loop state of one [`drive_stream`] attempt: the accumulated blocks,
/// the submitted tool calls and their in-flight futures.
struct StreamDrive<'a> {
    ctx: super::StreamPumpCtx<'a>,
    in_flight: FuturesUnordered<ToolFuture<'a>>,
    text_chunks: Vec<String>,
    thinking_chunks: Vec<(String, Option<String>)>,
    text_bytes: usize,
    thinking_bytes: usize,
    // Per-block dedup flags: set when an incremental `OutputTextDelta`
    // / `OutputThinkingDelta` has already been streamed for the block
    // currently accumulating, so the block-close `OutputItemDone` arm
    // does not re-emit the full text/thinking a second time. Reset at
    // each block close so a subsequent block starts fresh. The flags
    // also cover the buffered/synthesised fallback stream
    // (`response_stream_from_response`), which emits only
    // `OutputItemDone` and no deltas — there the flag stays `false` so
    // the close arm still emits the whole block.
    streamed_text_in_block: bool,
    streamed_thinking_in_block: bool,
    tool_calls_seen: Vec<ToolCallInfo>,
    // Submission index of every future pushed into `in_flight`, in
    // push order. `tool_calls_seen.len()` after the push is the
    // canonical index for each new call.
    spawned_indices: Vec<usize>,
    // Calls whose result the loop already had (cache hit, steering
    // block, invalid tool use). They still go through the executor so
    // the kernel records and gates them, but they must not refresh the
    // per-run cache.
    preset_indices: Vec<usize>,
    end_turn: Option<bool>,
    usage: Usage,
    liveness: Liveness,
    open_tool: Option<OpenToolCall>,
}

impl<'a> StreamDrive<'a> {
    fn new(ctx: super::StreamPumpCtx<'a>) -> Self {
        Self {
            ctx,
            in_flight: FuturesUnordered::new(),
            text_chunks: Vec::new(),
            thinking_chunks: Vec::new(),
            text_bytes: 0,
            thinking_bytes: 0,
            streamed_text_in_block: false,
            streamed_thinking_in_block: false,
            tool_calls_seen: Vec::new(),
            spawned_indices: Vec::new(),
            preset_indices: Vec::new(),
            end_turn: None,
            usage: Usage::default(),
            liveness: Liveness::new(),
            open_tool: None,
        }
    }

    /// Pull events until `Completed` (returns its stop reason) or a
    /// short-circuit outcome.
    async fn pump_events(
        &mut self,
        stream: &mut ResponseEventStream,
        state: &mut super::super::LoopState,
        model_name: &str,
    ) -> Result<Option<StopReason>, StreamPumpOutcome> {
        let stream_event_timeout = self.ctx.config.stream_event_timeout;
        loop {
            let next_step =
                next_stream_step(stream, stream_event_timeout, self.ctx.cancellation_token).await;
            match next_step {
                StreamStep::Cancelled => return Err(self.cancelled_mid_stream(model_name)),
                StreamStep::TimedOut => return Err(self.timed_out(stream_event_timeout)),
                StreamStep::TransportErr(err) => return Err(self.interrupted(err)),
                StreamStep::End => {
                    return Err(self.interrupted(StreamError::TransportClosed {
                        context: "missing completed event".into(),
                    }))
                }
                StreamStep::Event(event) => {
                    if let ControlFlow::Break(done) = self.on_event(event, state).await {
                        return done;
                    }
                }
            }
        }
    }

    fn timed_out(&self, stream_event_timeout: Duration) -> StreamPumpOutcome {
        let elapsed_ms = stream_event_timeout
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        // Turn the previously-silent stall into an actionable
        // box: which phase the stream died in and how much
        // content had streamed before it went quiet.
        let thinking_bytes = self.thinking_chunks.iter().map(|(t, _)| t.len()).sum();
        let text_bytes = self.text_chunks.iter().map(String::len).sum();
        console::stream_timeout_block(&console::StreamTimeoutView {
            elapsed_ms,
            last_phase: self.liveness.logged_phase,
            thinking_bytes,
            text_bytes,
        });
        StreamPumpOutcome::Error(AgentError::StreamTimeout { elapsed_ms })
    }

    fn interrupted(&self, err: StreamError) -> StreamPumpOutcome {
        interrupted_outcome(
            err,
            self.tool_calls_seen.is_empty(),
            self.text_bytes,
            self.thinking_bytes,
        )
    }
}

/// Do not replay a request once a completed tool call was admitted: tools
/// may have side effects and user inputs may already have been drained.
/// Partial tool input alone has not executed and retains its retry path.
fn interrupted_outcome(
    err: aura_model::StreamError,
    no_tools_admitted: bool,
    text_bytes: usize,
    thinking_bytes: usize,
) -> StreamPumpOutcome {
    match err {
        aura_model::StreamError::TransportClosed { .. } if no_tools_admitted => {
            StreamPumpOutcome::RetryableTransport {
                error: AgentError::Stream(err),
                text_bytes,
                thinking_bytes,
            }
        }
        aura_model::StreamError::StreamAbortedWithPartial {
            reason,
            partial_tool_use,
        } if no_tools_admitted => StreamPumpOutcome::AbortedWithPartial {
            reason,
            partial_tool_use,
            text_bytes,
            thinking_bytes,
        },
        other => StreamPumpOutcome::Error(AgentError::Stream(other)),
    }
}

enum StreamStep {
    Event(ResponseEvent),
    End,
    Cancelled,
    TimedOut,
    TransportErr(aura_model::StreamError),
}

async fn next_stream_step(
    stream: &mut ResponseEventStream,
    stream_event_timeout: Duration,
    cancellation_token: Option<&CancellationToken>,
) -> StreamStep {
    let polled = if let Some(token) = cancellation_token {
        // `biased` ensures cancellation observed alongside an event
        // always wins (Rule 6.3 cancellation precedence). Even if the
        // stream and the token are both ready, we route to the
        // cancellation arm.
        tokio::select! {
            biased;
            () = token.cancelled() => return StreamStep::Cancelled,
            polled = timeout(stream_event_timeout, stream.next()) => polled,
        }
    } else {
        timeout(stream_event_timeout, stream.next()).await
    };

    match polled {
        Err(_elapsed) => StreamStep::TimedOut,
        Ok(None) => StreamStep::End,
        Ok(Some(Ok(event))) => StreamStep::Event(event),
        Ok(Some(Err(err))) => StreamStep::TransportErr(err),
    }
}
