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

use std::pin::Pin;
use std::time::{Duration, Instant};

use aura_model_reasoner::{
    OutputItem, ResponseEvent, ResponseEventStream, StreamError, StreamPhase, Usage,
};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::console;
use crate::events::AgentLoopEvent;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use crate::AgentError;

use super::synthesize::synthesize_response;
use super::{emit_event, StreamPumpOutcome};

/// Shortest gap between two `ToolInputSnapshot`s of a streaming tool
/// call. Each snapshot carries the whole input so far, so one per wire
/// delta would copy a large `write_file` body thousands of times.
const TOOL_INPUT_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);

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
    let super::StreamPumpCtx {
        config,
        executor,
        cancellation_token,
        event_tx,
        on_response,
        ..
    } = ctx;
    let mut in_flight: FuturesUnordered<ToolFuture<'_>> = FuturesUnordered::new();
    let mut text_chunks: Vec<String> = Vec::new();
    let mut thinking_chunks: Vec<(String, Option<String>)> = Vec::new();
    let mut text_bytes = 0;
    let mut thinking_bytes = 0;
    // Per-block dedup flags: set when an incremental `OutputTextDelta`
    // / `OutputThinkingDelta` has already been streamed for the block
    // currently accumulating, so the block-close `OutputItemDone` arm
    // does not re-emit the full text/thinking a second time. Reset at
    // each block close so a subsequent block starts fresh. The flags
    // also cover the buffered/synthesised fallback stream
    // (`response_stream_from_response`), which emits only
    // `OutputItemDone` and no deltas — there the flag stays `false` so
    // the close arm still emits the whole block.
    let mut streamed_text_in_block = false;
    let mut streamed_thinking_in_block = false;
    let mut tool_calls_seen: Vec<ToolCallInfo> = Vec::new();
    // Submission index of every future pushed into `in_flight`, in
    // push order. `tool_calls_seen.len()` after the push is the
    // canonical index for each new call.
    let mut spawned_indices: Vec<usize> = Vec::new();
    // Calls whose result the loop already had (cache hit, steering
    // block, invalid tool use). They still go through the executor so
    // the kernel records and gates them, but they must not refresh the
    // per-run cache.
    let mut preset_indices: Vec<usize> = Vec::new();
    let mut end_turn: Option<bool> = None;
    let stop_reason;
    let mut usage = Usage::default();
    let stream_event_timeout = config.stream_event_timeout;
    let per_tool_timeout = config.per_tool_timeout;
    let mut liveness = Liveness::new();
    let mut open_tool: Option<OpenToolCall> = None;

    loop {
        let next_step =
            next_stream_step(&mut stream, stream_event_timeout, cancellation_token).await;
        match next_step {
            StreamStep::Cancelled => {
                // Mid-stream cancellation. If the model has already
                // emitted any `tool_use` blocks we MUST synthesise
                // paired `[CANCELLED]` tool_results so the downstream
                // `dispatch_streamed_response` →
                // `push_tool_result_message_with_context` step can
                // close the Anthropic `tool_use ↔ tool_result`
                // adjacency contract before the loop breaks. Pre-fix
                // this returned a bare `Cancelled` and the conversation
                // history ended with an orphaned assistant `tool_use`
                // block — invalid on the next sampling call.
                // See `pipeline_cancellation_mid_tool_execution_aborts_loop`.
                if tool_calls_seen.is_empty() {
                    return StreamPumpOutcome::Cancelled;
                }
                let snap = CancelledSnapshot {
                    text_chunks: &text_chunks,
                    thinking_chunks: &thinking_chunks,
                    tool_calls_seen: &tool_calls_seen,
                    spawned_indices: &spawned_indices,
                    end_turn,
                    usage: &usage,
                    model_name,
                };
                // Tool futures are only polled in the drain below, so none has run yet.
                return cancelled_outcome(&snap, Vec::new(), false);
            }
            StreamStep::TimedOut => {
                let elapsed_ms = stream_event_timeout
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX);
                // Turn the previously-silent stall into an actionable
                // box: which phase the stream died in and how much
                // content had streamed before it went quiet.
                let thinking_bytes = thinking_chunks.iter().map(|(t, _)| t.len()).sum();
                let text_bytes = text_chunks.iter().map(String::len).sum();
                console::stream_timeout_block(&console::StreamTimeoutView {
                    elapsed_ms,
                    last_phase: liveness.logged_phase,
                    thinking_bytes,
                    text_bytes,
                });
                return StreamPumpOutcome::Error(AgentError::StreamTimeout { elapsed_ms });
            }
            StreamStep::TransportErr(err) => {
                return interrupted_outcome(
                    err,
                    tool_calls_seen.is_empty(),
                    text_bytes,
                    thinking_bytes,
                );
            }
            StreamStep::End => {
                return interrupted_outcome(
                    StreamError::TransportClosed {
                        context: "missing completed event".into(),
                    },
                    tool_calls_seen.is_empty(),
                    text_bytes,
                    thinking_bytes,
                )
            }
            StreamStep::Event(event) => match event {
                ResponseEvent::OutputItemDone(OutputItem::InvalidToolUse { id, name, reason }) => {
                    open_tool = None;
                    let call = ToolCallInfo {
                        id: id.clone(),
                        name,
                        input: serde_json::json!({}),
                    };
                    let index = tool_calls_seen.len();
                    tool_calls_seen.push(call.clone());
                    spawned_indices.push(index);
                    preset_indices.push(index);
                    in_flight.push(spawn_tool_with_timeout(
                        index,
                        executor,
                        call,
                        Some(("invalid_tool_use", ToolCallResult::error(id, reason))),
                        per_tool_timeout,
                        event_tx,
                    ));
                }
                ResponseEvent::OutputItemDone(OutputItem::ToolUse { id, name, input }) => {
                    let announced = open_tool.take().is_some_and(|tool| tool.id == id);
                    if let Some(err) = handle_tool_use_event(
                        ctx,
                        ToolCallInfo { id, name, input },
                        announced,
                        DriveLocals {
                            tool_calls_seen: &mut tool_calls_seen,
                            preset_indices: &mut preset_indices,
                            spawned_indices: &mut spawned_indices,
                            in_flight: &mut in_flight,
                        },
                        per_tool_timeout,
                        state,
                    )
                    .await
                    {
                        return err;
                    }
                }
                ResponseEvent::OutputTextDelta(chunk) => {
                    text_bytes += chunk.len();
                    // Incremental assistant text: stream it to the
                    // client the moment it arrives so a long writing
                    // phase is never a silent gap. The full block is
                    // still accumulated at `OutputItemDone(Message)`
                    // below for final-response synthesis.
                    streamed_text_in_block = true;
                    emit_event(event_tx, AgentLoopEvent::TextDelta(chunk));
                }
                ResponseEvent::OutputThinkingDelta(chunk) => {
                    thinking_bytes += chunk.len();
                    // Incremental extended-thinking: stream it live so
                    // the client can render the current thinking block
                    // instead of waiting out a multi-second think.
                    streamed_thinking_in_block = true;
                    emit_event(event_tx, AgentLoopEvent::ThinkingDelta(chunk));
                }
                ResponseEvent::OutputItemDone(OutputItem::Message { text }) => {
                    // Block close. Only emit the whole block as a delta
                    // when nothing was streamed incrementally for it
                    // (the buffered/synthesised fallback stream, which
                    // emits no `OutputTextDelta`); otherwise the client
                    // already has the text and a re-emit would double
                    // it. Always accumulate for `synthesize_response`.
                    if !streamed_text_in_block {
                        text_bytes += text.len();
                        emit_event(event_tx, AgentLoopEvent::TextDelta(text.clone()));
                    }
                    streamed_text_in_block = false;
                    text_chunks.push(text);
                }
                ResponseEvent::OutputItemDone(OutputItem::Thinking {
                    thinking,
                    signature,
                }) => {
                    // Same dedup rationale as the Message arm. Always
                    // emit the `ThinkingComplete` marker so consumers
                    // that pattern-match on the end-of-thinking event
                    // (e.g. the chat client's "Thought for Xs" pill)
                    // still see the close signal.
                    if !streamed_thinking_in_block {
                        thinking_bytes += thinking.len();
                        emit_event(event_tx, AgentLoopEvent::ThinkingDelta(thinking.clone()));
                    }
                    streamed_thinking_in_block = false;
                    emit_event(event_tx, AgentLoopEvent::ThinkingComplete);
                    thinking_chunks.push((thinking, signature));
                }
                ResponseEvent::Completed {
                    end_turn: et,
                    stop_reason: sr,
                    usage: u,
                } => {
                    end_turn = et;
                    stop_reason = sr;
                    usage = u;
                    break;
                }
                ResponseEvent::Error(err) => {
                    return interrupted_outcome(
                        err,
                        tool_calls_seen.is_empty(),
                        text_bytes,
                        thinking_bytes,
                    );
                }
                ResponseEvent::OutputToolUseStart { id, name } => {
                    liveness.beat(StreamPhase::ToolInput, event_tx);
                    emit_event(
                        event_tx,
                        AgentLoopEvent::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        },
                    );
                    open_tool = Some(OpenToolCall {
                        id,
                        name,
                        input_json: String::new(),
                        last_snapshot: tokio::time::Instant::now(),
                    });
                }
                ResponseEvent::OutputToolInputDelta(chunk) => {
                    liveness.beat(StreamPhase::ToolInput, event_tx);
                    if let Some(tool) = open_tool.as_mut() {
                        tool.input_json.push_str(&chunk);
                        if tool.last_snapshot.elapsed() >= TOOL_INPUT_SNAPSHOT_INTERVAL {
                            tool.last_snapshot = tokio::time::Instant::now();
                            emit_event(
                                event_tx,
                                AgentLoopEvent::ToolInputSnapshot {
                                    id: tool.id.clone(),
                                    name: tool.name.clone(),
                                    input: tool.input_json.clone(),
                                },
                            );
                        }
                    }
                }
                ResponseEvent::Keepalive(phase) => {
                    // A ping or intra-block delta. Reaching this arm
                    // already reset the per-event liveness timeout
                    // (the stream yielded an item, so the loop
                    // re-enters `next_stream_step` with a fresh
                    // window).
                    liveness.beat(phase, event_tx);
                }
            },
        }
    }

    let response = synthesize_response(
        &text_chunks,
        &thinking_chunks,
        &tool_calls_seen,
        end_turn,
        stop_reason,
        &usage,
        model_name,
    );
    if let Some(on_response) = on_response {
        on_response(&response);
    }

    // Drain the in-flight calls (codex `drain_in_flight`). Honours
    // cancellation: a token fired during drain still aborts before we
    // mutate `state.messages` in the caller.
    //
    // Calls are collected as they finish, each tagged with its
    // submission index, and sorted afterwards so the final
    // `tool_results` vec mirrors the order the model emitted the
    // corresponding `OutputItemDone(tool_use)` events. Collecting out of
    // order matters on cancel: a call that already ran (an edit that
    // landed) keeps its real result even when an earlier call is still
    // waiting, e.g. on an approval.
    // While draining potentially long-running tool calls, emit a
    // periodic `tool_running` progress heartbeat. The spawned tool
    // futures only make progress as `FuturesUnordered` is polled in the
    // drain loop below, so this window is the main silent stretch of a
    // turn. The heartbeat keeps the aura-os server's sliding-idle
    // watchdog (`AURA_TURN_MAX_TIMEOUT_SECS`) resetting so only a
    // genuinely hung tool trips `turn_timeout`. No-op when there are no
    // tools (`tool_calls_seen` empty) or no event channel. The guard
    // aborts the ticker as soon as the drain loop ends.
    let _tool_heartbeat = super::super::tool_pipeline::spawn_tool_heartbeat(
        event_tx,
        &tool_calls_seen,
        super::super::tool_pipeline::tool_heartbeat_interval(),
    );

    let mut spawned_pairs: Vec<(usize, (ToolCallInfo, ToolCallResult))> = Vec::new();
    loop {
        let maybe_next = drain_next(&mut in_flight, cancellation_token).await;
        match maybe_next {
            DrainStep::Cancelled => {
                // Mid-drain cancellation. Drop the still-pending
                // `in_flight` futures (cancellation-safe — they were
                // wrapped in `tokio::time::timeout` and the executor
                // borrow ends here) and synthesise `[CANCELLED]`
                // tool_results for every `tool_calls_seen` entry that
                // has NOT yet produced a paired result. The downstream
                // dispatch tail (`dispatch_streamed_response` →
                // `push_tool_result_message_with_context`) then closes
                // the Anthropic `tool_use ↔ tool_result` adjacency
                // contract before the loop breaks on the synthesised
                // `stop_loop = true` flag. Pre-fix this returned a
                // bare `Cancelled` and let `executor.execute(...)` run
                // to natural completion — the regression contract is
                // `pipeline_cancellation_mid_tool_execution_aborts_loop`.
                settle_pending_writes(
                    &mut in_flight,
                    &tool_calls_seen,
                    &spawned_indices,
                    &mut spawned_pairs,
                    WRITE_SETTLE_GRACE,
                )
                .await;
                let snap = CancelledSnapshot {
                    text_chunks: &text_chunks,
                    thinking_chunks: &thinking_chunks,
                    tool_calls_seen: &tool_calls_seen,
                    spawned_indices: &spawned_indices,
                    end_turn,
                    usage: &usage,
                    model_name,
                };
                note_reported(state, event_tx, &spawned_pairs);
                return cancelled_outcome(&snap, spawned_pairs, true);
            }
            DrainStep::Done => break,
            DrainStep::Result(indexed) => spawned_pairs.push(*indexed),
        }
    }

    note_reported(state, event_tx, &spawned_pairs);
    spawned_pairs.sort_by_key(|(idx, _)| *idx);
    let tool_results: Vec<(ToolCallInfo, ToolCallResult)> =
        spawned_pairs.into_iter().map(|(_, pair)| pair).collect();

    // Layer E.4: refresh the per-run cache with any newly-executed
    // results. Mirrors the buffered path's
    // `tool_execution::handle_tool_use` → `update_cache` step so the
    // pump path participates in the same memoization /
    // invalidate-on-write contract. The "uncached" set is the tools
    // the executor actually ran (preset results were already known at
    // submission time).
    let spawned_calls: Vec<ToolCallInfo> = spawned_indices
        .iter()
        .filter(|i| !preset_indices.contains(i))
        .filter_map(|i| tool_calls_seen.get(*i).cloned())
        .collect();
    let spawned_results: Vec<ToolCallResult> = tool_results
        .iter()
        .filter(|(c, _)| spawned_calls.iter().any(|sc| sc.id == c.id))
        .map(|(_, r)| r.clone())
        .collect();
    super::super::tool_execution::update_cache(
        &mut state.tool_cache,
        &spawned_calls,
        &spawned_results,
    );

    StreamPumpOutcome::Completed {
        response,
        tool_results,
    }
}

/// Do not replay a request once a completed tool call was admitted: tools
/// may have side effects and user inputs may already have been drained.
/// Partial tool input alone has not executed and retains its retry path.
fn interrupted_outcome(
    err: aura_model_reasoner::StreamError,
    no_tools_admitted: bool,
    text_bytes: usize,
    thinking_bytes: usize,
) -> StreamPumpOutcome {
    match err {
        aura_model_reasoner::StreamError::TransportClosed { .. } if no_tools_admitted => {
            StreamPumpOutcome::RetryableTransport {
                error: AgentError::Stream(err),
                text_bytes,
                thinking_bytes,
            }
        }
        aura_model_reasoner::StreamError::StreamAbortedWithPartial {
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

/// Phase 8 wrapper bundling the four mutable cursor buffers that
/// [`drive_stream`] hands to [`handle_tool_use_event`]: the running
/// `tool_calls_seen` FIFO, the indices of calls with a preset result,
/// the per-call spawn-index trace, and the in-flight futures
/// themselves. Grouping them keeps the helper under the clippy ceiling
/// without splitting the submission-order invariant across helpers —
/// every field stays a borrowed mutation target.
pub(super) struct DriveLocals<'a, 'b> {
    pub(super) tool_calls_seen: &'b mut Vec<ToolCallInfo>,
    pub(super) preset_indices: &'b mut Vec<usize>,
    pub(super) spawned_indices: &'b mut Vec<usize>,
    pub(super) in_flight: &'b mut FuturesUnordered<ToolFuture<'a>>,
}

/// Handle one `OutputItemDone(ToolUse)` event: circling-read gate,
/// per-run cache lookup, spawn-or-serve, and per-tool-result
/// input-queue drain. Returns `Some(StreamPumpOutcome)` only when an
/// invariant-violation early-return must propagate to [`drive_stream`]
/// (e.g. the per-run cache partition contract is broken). On the
/// happy path returns `None` and pushes the call into the FIFO.
/// `announced` is set when `ToolStart` already went out at block start.
async fn handle_tool_use_event<'a, 'b>(
    ctx: super::StreamPumpCtx<'a>,
    call: ToolCallInfo,
    announced: bool,
    locals: DriveLocals<'a, 'b>,
    per_tool_timeout: Duration,
    state: &mut super::super::LoopState,
) -> Option<StreamPumpOutcome> {
    let super::StreamPumpCtx {
        config: _,
        executor,
        cancellation_token: _,
        input_queue,
        event_tx,
        on_response: _,
    } = ctx;
    let DriveLocals {
        tool_calls_seen,
        preset_indices,
        spawned_indices,
        in_flight,
    } = locals;
    let submission_index = tool_calls_seen.len();
    tool_calls_seen.push(call.clone());

    // Layer E.4: emit ToolStart (unless the block start already did) and
    // a final ToolInputSnapshot carrying the full parsed input, so chat
    // UX sees the same event sequence as the buffered streaming path.
    if !announced {
        emit_event(
            event_tx,
            AgentLoopEvent::ToolStart {
                id: call.id.clone(),
                name: call.name.clone(),
            },
        );
    }
    emit_event(
        event_tx,
        AgentLoopEvent::ToolInputSnapshot {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.to_string(),
        },
    );

    // Circling read gate: before cache lookup or executor spawn,
    // reject duplicate `read_file` paths once the steering layer
    // has latched a read-only loop. This keeps the streaming pump
    // in lockstep with the buffered dispatcher.
    let single = std::slice::from_ref(&call);
    let (blocked_reads, allowed_calls) =
        super::super::tool_pipeline::partition_circling_duplicate_reads(single, state);
    if let Some(blocked) = blocked_reads.into_iter().next() {
        let decision = super::super::decisions::implement_now_block(&call, state);
        state.decide(decision);
        spawned_indices.push(submission_index);
        preset_indices.push(submission_index);
        in_flight.push(spawn_tool_with_timeout(
            submission_index,
            executor,
            call,
            Some(("steering_block", blocked)),
            per_tool_timeout,
            event_tx,
        ));
        return None;
    }

    // Layer E.4 tool-result cache: consult the per-run cache before
    // spawning the future. On a hit the cached [`ToolCallResult`] is
    // handed to the executor as a preset, so the kernel still records
    // and gates the call but the tool itself does NOT run again (codex
    // parity for read-only tools).
    //
    // Cache invariant (Rule 4.1 / 4.3): `split_cached` partitions
    // `allowed_calls` (a single-element slice here) into
    // `cached_results` and `uncached_calls` such that every input
    // call ends up in exactly one bucket. So when either bucket is
    // non-empty its `into_iter().next()` MUST yield an element; a
    // `None` here is a true partition invariant violation. Surface
    // it as a fatal `LlmCallError` via `AgentError::Internal` rather
    // than panicking — the `Error` outcome is folded into
    // `LlmCallError::Fatal` upstream in `sampling::run_sampling_request`.
    let (cached_results, uncached_calls) =
        super::super::tool_execution::split_cached(&allowed_calls, &state.tool_cache);
    if !cached_results.is_empty() {
        let Some(first) = cached_results.into_iter().next() else {
            return Some(StreamPumpOutcome::Error(AgentError::Internal(
                "stream pump cache invariant: cached_results was non-empty but yielded no element \
                 (split_cached partition contract broken)"
                    .to_string(),
            )));
        };
        state.decide(super::super::decisions::cache_hit(&call));
        spawned_indices.push(submission_index);
        preset_indices.push(submission_index);
        in_flight.push(spawn_tool_with_timeout(
            submission_index,
            executor,
            call,
            Some(("cache", first)),
            per_tool_timeout,
            event_tx,
        ));
    } else if !uncached_calls.is_empty() {
        let Some(first) = uncached_calls.into_iter().next() else {
            return Some(StreamPumpOutcome::Error(AgentError::Internal(
                "stream pump cache invariant: uncached_calls was non-empty but yielded no element \
                 (split_cached partition contract broken)"
                    .to_string(),
            )));
        };
        spawned_indices.push(submission_index);
        in_flight.push(spawn_tool_with_timeout(
            submission_index,
            executor,
            first,
            None,
            per_tool_timeout,
            event_tx,
        ));
    }

    // E.3 input-drain granularity: once per `OutputItemDone(tool_use)`.
    // See module-level docs.
    if let Some(queue) = input_queue {
        if queue.has_pending() {
            let drained = queue.drain().await;
            if !drained.is_empty() {
                super::super::turn::apply_user_inputs_to_messages(&mut state.messages, drained);
            }
        }
    }
    None
}

enum StreamStep {
    Event(ResponseEvent),
    End,
    Cancelled,
    TimedOut,
    TransportErr(aura_model_reasoner::StreamError),
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

enum DrainStep {
    // Boxed: `ToolCallResult` carries an optional screenshot image, so
    // the pair is large relative to the unit `Done` / `Cancelled`
    // variants. Boxing keeps the enum small (clippy::large_enum_variant).
    Result(Box<IndexedResult>),
    Done,
    Cancelled,
}

async fn drain_next<'a>(
    in_flight: &mut FuturesUnordered<ToolFuture<'a>>,
    cancellation_token: Option<&CancellationToken>,
) -> DrainStep {
    if in_flight.is_empty() {
        return DrainStep::Done;
    }
    if let Some(token) = cancellation_token {
        tokio::select! {
            biased;
            () = token.cancelled() => DrainStep::Cancelled,
            next = in_flight.next() => match next {
                Some(pair) => DrainStep::Result(Box::new(pair)),
                None => DrainStep::Done,
            }
        }
    } else {
        match in_flight.next().await {
            Some(pair) => DrainStep::Result(Box::new(pair)),
            None => DrainStep::Done,
        }
    }
}

/// How long a cancel waits for file-changing calls that are already in
/// flight. A write that has reached the filesystem runs to completion even
/// when its future is dropped, so reporting it as cancelled would tell the
/// model the file is unchanged when it isn't. File operations finish in
/// milliseconds; anything still pending after this (say, waiting on an
/// approval) is reported as possibly applied.
const WRITE_SETTLE_GRACE: Duration = Duration::from_secs(2);

const CANCELLED_CONTENT: &str =
    "[CANCELLED] Tool execution was cancelled by the user before the tool returned.";

const CANCELLED_WRITE_CONTENT: &str =
    "[CANCELLED] Tool execution was cancelled by the user before the tool returned. \
     The change may already have been made: re-read the file before editing it again.";

/// On cancel, keep draining `in_flight` for up to `grace` while any
/// spawned file-changing call is still without a result, so the ones that
/// land keep their real results.
async fn settle_pending_writes(
    in_flight: &mut FuturesUnordered<ToolFuture<'_>>,
    tool_calls_seen: &[ToolCallInfo],
    spawned_indices: &[usize],
    spawned_pairs: &mut Vec<IndexedResult>,
    grace: Duration,
) {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let write_pending = spawned_indices.iter().any(|idx| {
            tool_calls_seen
                .get(*idx)
                .is_some_and(|call| crate::helpers::is_write_tool(&call.name))
                && !spawned_pairs.iter().any(|(done, _)| done == idx)
        });
        if !write_pending {
            return;
        }
        match tokio::time::timeout_at(deadline, in_flight.next()).await {
            Ok(Some(pair)) => spawned_pairs.push(pair),
            Ok(None) | Err(_) => return,
        }
    }
}

/// Build a `Completed` outcome that carries `[CANCELLED]` tool_results
/// for every emitted tool_use that did NOT yet produce a paired
/// result. Drains `cached_pairs` + `spawned_pairs` first (already-
/// resolved entries keep their real result), then synthesises
/// `stop_loop = true` cancelled entries for the rest so the downstream
/// dispatcher breaks the loop on the same iteration.
///
/// Returning `Completed` (instead of `Cancelled`) is deliberate: the
/// sampling driver's `Completed` arm runs
/// `iteration::accumulate_response` (pushes the assistant message with
/// the seen tool_use blocks) and `dispatch_streamed_response` (pushes
/// the paired tool_result-bearing user message). Routing through that
/// path is the only way to satisfy Anthropic's `tool_use ↔ tool_result`
/// adjacency contract on a mid-tool cancel — a bare `Cancelled` would
/// leave an orphaned assistant `tool_use` block at the tail of the
/// transcript and the next sampling call would be structurally invalid.
/// Phase 8 wrapper around the immutable observations [`drive_stream`]
/// accumulated before a mid-flight cancellation. Bundles the four
/// `&[…]` slices plus the response-synthesis state needed to fold a
/// partial transcript into a [`StreamPumpOutcome::Completed`] with
/// `[CANCELLED]` tool_results. Owned `spawned_pairs` stays a
/// positional argument so it can be moved into the merge step.
struct CancelledSnapshot<'a> {
    text_chunks: &'a [String],
    thinking_chunks: &'a [(String, Option<String>)],
    tool_calls_seen: &'a [ToolCallInfo],
    spawned_indices: &'a [usize],
    end_turn: Option<bool>,
    usage: &'a Usage,
    model_name: &'a str,
}

/// `started` says whether the pending calls may have begun running, in
/// which case an unfinished file change is reported as possibly applied.
fn cancelled_outcome(
    snap: &CancelledSnapshot<'_>,
    mut spawned_pairs: Vec<(usize, (ToolCallInfo, ToolCallResult))>,
    started: bool,
) -> StreamPumpOutcome {
    use std::collections::HashSet;
    let &CancelledSnapshot {
        text_chunks,
        thinking_chunks,
        tool_calls_seen,
        spawned_indices,
        end_turn,
        usage,
        model_name,
    } = snap;

    let already: HashSet<usize> = spawned_pairs.iter().map(|(idx, _)| *idx).collect();

    let still_pending: HashSet<usize> = spawned_indices
        .iter()
        .copied()
        .filter(|idx| !already.contains(idx))
        .collect();

    for idx in still_pending {
        if let Some(call) = tool_calls_seen.get(idx) {
            let content = if started && crate::helpers::is_write_tool(&call.name) {
                CANCELLED_WRITE_CONTENT
            } else {
                CANCELLED_CONTENT
            };
            let result = ToolCallResult {
                tool_use_id: call.id.clone(),
                content: content.to_string(),
                is_error: true,
                kind: aura_core_types::ToolResultKind::AgentError,
                stop_loop: true,
                image: None,
                file_changes: Vec::new(),
            };
            spawned_pairs.push((idx, (call.clone(), result)));
        }
    }

    spawned_pairs.sort_by_key(|(idx, _)| *idx);
    let tool_results: Vec<(ToolCallInfo, ToolCallResult)> =
        spawned_pairs.into_iter().map(|(_, pair)| pair).collect();

    let response = synthesize_response(
        text_chunks,
        thinking_chunks,
        tool_calls_seen,
        end_turn,
        None,
        usage,
        model_name,
    );
    StreamPumpOutcome::Completed {
        response,
        tool_results,
    }
}

/// Remember which results the tool futures already reported, so the
/// dispatch tail does not report them again.
fn note_reported(
    state: &mut super::super::LoopState,
    event_tx: Option<&tokio::sync::mpsc::Sender<AgentLoopEvent>>,
    pairs: &[(usize, (ToolCallInfo, ToolCallResult))],
) {
    if event_tx.is_some() {
        state
            .reported_tool_results
            .extend(pairs.iter().map(|(_, (call, _))| call.id.clone()));
    }
}

/// Tell the client a call finished the moment it does. Calls of one batch
/// run together, and one waiting on an approval must not keep the others
/// looking unfinished.
fn report_result(
    event_tx: Option<&tokio::sync::mpsc::Sender<AgentLoopEvent>>,
    call: &ToolCallInfo,
    result: &ToolCallResult,
) {
    emit_event(
        event_tx,
        AgentLoopEvent::ToolCallCompleted {
            tool_use_id: result.tool_use_id.clone(),
            tool_name: call.name.clone(),
            input: call.input.clone(),
            is_error: result.is_error,
        },
    );
    emit_event(
        event_tx,
        AgentLoopEvent::ToolResult {
            tool_use_id: result.tool_use_id.clone(),
            tool_name: call.name.clone(),
            content: result.content.clone(),
            is_error: result.is_error,
            image: result.image.clone(),
        },
    );
}

/// `preset` is a result the loop already has for `call`, with the reason
/// it has it; the executor records and gates it instead of running the
/// tool.
fn spawn_tool_with_timeout<'a>(
    index: usize,
    executor: &'a dyn AgentToolExecutor,
    call: ToolCallInfo,
    preset: Option<(&'static str, ToolCallResult)>,
    per_tool_timeout: Duration,
    event_tx: Option<&'a tokio::sync::mpsc::Sender<AgentLoopEvent>>,
) -> ToolFuture<'a> {
    Box::pin(async move {
        let call_clone = call.clone();
        let id = call_clone.id.clone();
        let name = call_clone.name.clone();
        let fut = match preset {
            Some((source, result)) => executor.spawn_preset(call_clone, source, result),
            None => executor.spawn_tool_call(call_clone),
        };
        let result = match timeout(per_tool_timeout, fut).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let elapsed_ms = per_tool_timeout.as_millis();
                ToolCallResult::error(
                    id,
                    format!("tool '{name}' timed out after {elapsed_ms}ms (per_tool_timeout)"),
                )
            }
        };
        report_result(event_tx, &call, &result);
        (index, (call, result))
    })
}
