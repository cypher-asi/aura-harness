//! Streaming sampling pump with stream-level tool overlap (Layer E.3).
//!
//! The pump is the per-sampling-request loop body that consumes a
//! [`aura_model::ResponseEventStream`] one event at a time, spawning
//! tool futures into a [`futures_util::stream::FuturesUnordered`] the
//! moment a tool-use item arrives instead of waiting for the entire
//! model response to buffer. After the model emits
//! [`aura_model::ResponseEvent::Completed`], the pump drains the
//! calls as they finish and sorts them back into submission order so
//! tool results appear in the conversation history in the same order
//! the model emitted them (codex's
//! `drain_in_flight` contract,
//! [codex-rs/core/src/session/turn.rs:2141 analog](
//! https://github.com/.../codex-rs/core/src/session/turn.rs)).
//!
//! # Module layout (Phase 3 split)
//!
//! Before Phase 3 this lived in a single ~1.5K-line file. The split
//! carves the pump into purpose-driven submodules so no file exceeds
//! ~600 lines and `unwrap()` sweeps stay scoped to the file that
//! owns the invariant:
//!
//! - [`retry`] — shared retry classifier + budget. Phase 1's report
//!   flagged the `stream_retry_params` duplication between this pump
//!   and the legacy buffered streaming retry loop; both now read
//!   `aura_config::reasoner().llm_retry` through
//!   [`retry::stream_retry_params`].
//! - [`driver`] — `drive_stream` event loop + in-flight tool drain.
//!   Owns the per-event timeout / per-tool timeout / FIFO ordering
//!   invariants documented below.
//! - Post-pump dispatch tail collapsed into the unified
//!   [`super::tool_pipeline::dispatch`] entry point in Phase 4 (the
//!   previous `dispatch` submodule is gone).
//! - [`synthesize`] — `synthesize_response`: rebuilds a
//!   [`aura_model::ModelResponse`] from the per-block chunks the
//!   pump observed. Includes the corrected `end_turn = Some(false)`
//!   stop-reason interpretation (Phase 3).
//! - [`tests`] — pump-scope unit tests; transport / executor parity
//!   tests live alongside `sampling.rs` integration tests.
//!
//! # Module-level invariants (Rule 13)
//!
//! - **FIFO**: tool results return in the order the model emitted
//!   the corresponding `OutputItemDone(tool_use)` events.
//! - **Atomic cancellation (Rule 6.3)**: when the cancellation token
//!   fires the pump returns [`StreamPumpOutcome::Cancelled`]
//!   *without* mutating `state.messages` — in-flight tools are
//!   aborted by dropping the `FuturesUnordered`. The downstream
//!   sampling driver then bails without running the post-execution
//!   path, so the conversation history never half-writes a
//!   partially-drained tool batch.
//! - **Per-event timeout (Rule 6.2)**: each `stream.try_next()` call
//!   is wrapped with `tokio::time::timeout(stream_event_timeout, …)`
//!   so a *genuinely* silent stream surfaces as
//!   [`crate::AgentError::StreamTimeout`] instead of hanging. Pings
//!   and intra-block deltas arrive as
//!   [`aura_model::ResponseEvent::Keepalive`] and reset the
//!   window (and drive the phase-transition transcript lines), so a
//!   slow-but-alive thinking block is not mistaken for a dead stream.
//! - **Per-tool timeout (Rule 6.2)**: each spawned tool future is
//!   wrapped with `tokio::time::timeout(per_tool_timeout, …)` and a
//!   hung tool resolves to a synthetic
//!   [`crate::ToolCallResult`] error — the FIFO continues to drain
//!   so a single bad tool never poisons the rest of the batch.
//! - **Input-queue drain semantic**: the driver drains the optional
//!   [`InputQueue`] once per `OutputItemDone(tool_use)` (codex
//!   parity / richer mid-batch steering). Drained inputs are
//!   applied to `state.messages` immediately so they become visible
//!   to the next sampling request without dropping the current
//!   one's tool batch.
//! - **No `unwrap()` outside `#[cfg(test)]`** (Rule 4.1): every
//!   fallible step routes through `Result<_, AgentError>` or a
//!   match on `Option<ResponseEvent>` arms. The handful of
//!   `expect()` calls that remain inside `#[cfg(test)]` are
//!   intentional — test fixtures are exempt per Rule 4.1.

use aura_model::{ModelProvider, ModelRequest, ModelResponse, PartialToolUse};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;
use crate::session::input_queue::InputQueue;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use crate::AgentError;

use super::AgentLoopConfig;

mod attempts;
mod driver;
mod retry;
mod synthesize;

#[cfg(test)]
mod tests;

pub(in crate::agent_loop) use driver::TOOL_INPUT_SNAPSHOT_INTERVAL;
pub(in crate::agent_loop) use retry::stream_retry_params;

use attempts::{Attempts, Opened};
use driver::drive_stream;

/// Phase 8 context wrapper bundling the long-lived borrows that the
/// streaming pump driver and its inner event-loop helpers all carry.
///
/// Previously every helper in `stream_pump/` (`run_stream_pump`,
/// `drive_stream`, `handle_tool_use_event`) took the same five
/// borrows as separate parameters and lived under a
/// `clippy::too-many-arguments` allow. Bundling them into a
/// single `&'a` borrow at the transport seam keeps every helper
/// under the clippy ceiling without adding a `Clone` impl: every
/// field is itself a borrow so the wrapper is `Copy` for free.
///
/// `provider` is intentionally NOT in the bundle: it is only
/// consumed by [`run_stream_pump`] to open the
/// [`aura_model::ResponseEventStream`]; once the stream is open
/// the driver never touches the provider again. Keeping it out lets
/// the driver-scope unit tests construct a `StreamPumpCtx` without
/// having to stub a full `ModelProvider`.
#[derive(Clone, Copy)]
pub(super) struct StreamPumpCtx<'a> {
    pub(super) config: &'a AgentLoopConfig,
    pub(super) executor: &'a dyn AgentToolExecutor,
    pub(super) cancellation_token: Option<&'a CancellationToken>,
    pub(super) input_queue: Option<&'a InputQueue>,
    pub(super) event_tx: Option<&'a Sender<AgentLoopEvent>>,
    /// Called with the response once the model finishes streaming and
    /// before its tool calls are drained, so the call is on record while
    /// its tools run (or wait for an approval).
    pub(super) on_response: Option<&'a (dyn Fn(&ModelResponse) + Send + Sync)>,
}

/// Outcome of [`run_stream_pump`].
///
/// `Completed` carries a synthesised [`ModelResponse`] (so downstream
/// callers can reuse the existing `accumulate_response` /
/// `dispatch_stop_reason` helpers) plus the per-tool results gathered
/// from the FIFO drain. `Cancelled` is the atomic-no-state-mutation
/// path (Rule 6.3). `Error` carries the typed [`AgentError`] so the
/// caller can route it through the existing `iteration::LlmCallError`
/// surfacing helpers.
pub(super) enum StreamPumpOutcome {
    Completed {
        /// Synthesised assistant response. Stop reason is derived
        /// from `Completed.end_turn` and whether any tool calls were
        /// emitted; usage comes straight from the provider.
        response: ModelResponse,
        /// Tool calls observed in this stream, paired with their
        /// executed results in submission order. Empty when the
        /// response contained no `tool_use` blocks.
        tool_results: Vec<(ToolCallInfo, ToolCallResult)>,
    },
    /// External or in-band cancellation observed inside the pump
    /// loop. No `state.messages` mutation; in-flight tool futures
    /// were dropped (cancellation safety, Rule 6.3).
    Cancelled,
    /// Pump aborted on a typed error (stream timeout, transport
    /// closed, etc.). Carries the [`AgentError`] so the caller can
    /// fold it into the loop result without re-wrapping.
    Error(AgentError),
    /// A connection failure before any completed tool call was admitted.
    /// Only this outcome is safe to replay as a fresh sampling request.
    RetryableTransport {
        error: AgentError,
        text_bytes: usize,
        thinking_bytes: usize,
    },
    /// The response stream aborted while a `tool_use` block was
    /// still being accumulated. `run_stream_pump` consumes this
    /// internal outcome to drive the per-tool-call retry loop.
    AbortedWithPartial {
        reason: String,
        partial_tool_use: Option<PartialToolUse>,
        text_bytes: usize,
        thinking_bytes: usize,
    },
}

/// Drive one sampling request through the streaming pump.
///
/// Opens `provider.complete_response_stream(request)`, then drives
/// the resulting [`aura_model::ResponseEventStream`] to completion
/// with per-event timeout, biased cancellation, and per-tool
/// concurrency via [`futures_util::stream::FuturesUnordered`]. See the
/// module-level docs for the full invariant list.
pub(super) async fn run_stream_pump(
    ctx: StreamPumpCtx<'_>,
    provider: &dyn ModelProvider,
    mut request: ModelRequest,
    state: &mut super::LoopState,
) -> StreamPumpOutcome {
    let model_name = request.model.as_ref().to_string();
    let mut attempts = Attempts::new();

    for attempt in 0..=attempts.max_retries {
        if ctx
            .cancellation_token
            .is_some_and(CancellationToken::is_cancelled)
        {
            return StreamPumpOutcome::Cancelled;
        }
        if attempt > 0 {
            if let Err(outcome) = attempts.prepare_retry(ctx, attempt, &mut request).await {
                return outcome;
            }
        }

        let attempt_started = std::time::Instant::now();
        let stream = match attempts
            .open(ctx, provider, &request, attempt_started)
            .await
        {
            Opened::Stream(stream) => stream,
            Opened::Retry => continue,
            Opened::Cancelled => return StreamPumpOutcome::Cancelled,
            Opened::Failed(err) => return StreamPumpOutcome::Error(AgentError::Reason(err)),
        };
        let outcome = drive_stream(ctx, stream, state, &model_name).await;
        if let Some(outcome) = attempts.settle(ctx, provider, &model_name, attempt_started, outcome)
        {
            return outcome;
        }
    }

    attempts.exhausted(ctx)
}

/// Drop an advisory event onto the inner agent-event channel.
///
/// Phase 4: thin re-export of [`super::event_sink::emit`] so the
/// pump's hot per-`OutputItemDone` path shares the single unified
/// policy with [`super::streaming::emit`]. The previous local
/// `try_send + debug!` block diverged from the buffered path's
/// `try_send + warn!` policy; both now route through the same
/// function so the policy cannot drift apart again. See
/// [`super::event_sink`] module docs for the rationale.
pub(super) fn emit_event(
    tx: Option<&Sender<crate::events::AgentLoopEvent>>,
    event: crate::events::AgentLoopEvent,
) {
    super::event_sink::emit(tx, event);
}

// `Debug` impls for the outcome enum so tests and tracing diagnostics
// can format without us hand-rolling matchers everywhere.
impl std::fmt::Debug for StreamPumpOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed { tool_results, .. } => f
                .debug_struct("Completed")
                .field("tool_results_len", &tool_results.len())
                .finish(),
            Self::Cancelled => write!(f, "Cancelled"),
            Self::Error(e) => write!(f, "Error({e:?})"),
            Self::RetryableTransport { error, .. } => write!(f, "RetryableTransport({error:?})"),
            Self::AbortedWithPartial { reason, .. } => f
                .debug_struct("AbortedWithPartial")
                .field("reason", reason)
                .finish(),
        }
    }
}
