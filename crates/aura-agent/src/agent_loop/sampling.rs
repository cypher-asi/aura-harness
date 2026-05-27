//! Single sampling request driver (Phase 4 unified, Phase 7 pruned).
//!
//! A *sampling request* is one round-trip with the model provider:
//! one pre-call compaction pass, one
//! [`super::transport::ModelTransport::sample`] call (the pump
//! transport returned by [`super::transport::active_transport`]),
//! one response-accumulation pass, one `iteration_complete` event,
//! and one unified [`super::tool_pipeline::dispatch`] step that may
//! execute (or pass through) one batch of tool calls.
//!
//! Phase 4 collapsed the previously-duplicated
//! `run_sampling_request` / `run_sampling_request_streaming` pair
//! behind a single function. Phase 7 then deleted the legacy
//! buffered transport (parity proven, no caller flipped off the
//! pump) so the single tail now runs:
//! cancellation check → `accumulate_response` →
//! `emit_iteration_complete` → `tool_pipeline::dispatch`.
//!
//! Invariants:
//!
//! - Cancellation observed before the transport call short-circuits
//!   to a `needs_follow_up = false`, `broke_for_error = true`
//!   outcome so the turn loop unwinds without paying for one more
//!   model call.
//! - On [`super::iteration::LlmCallError`], the error is applied to
//!   [`crate::types::AgentLoopResult`] via
//!   [`super::iteration::LlmCallError::apply`] and
//!   `broke_for_error = true` instructs the turn loop to break
//!   (preserves the pre-Phase-4 behavior where a fatal model error
//!   terminated the loop immediately).
//! - `state.result.iterations` is incremented inside this function
//!   (the counter is "completed sampling requests", matching the
//!   pre-Phase-4 shape).
//! - Mid-tool cancellation inside the pump folds `[CANCELLED]`
//!   tool_results into a `Streamed` outcome with `stop_loop = true`
//!   markers so the Anthropic `tool_use ↔ tool_result` adjacency
//!   contract stays intact through `dispatch`. The post-sample
//!   cancellation probe therefore bails ONLY when the batch is
//!   empty (no in-flight tools to repair).

use std::time::Instant;

use aura_reasoner::{ModelProvider, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument};

use crate::events::AgentLoopEvent;
use crate::types::AgentToolExecutor;

use super::tool_pipeline::{self, ToolBatch, ToolEffectCtx};
use super::transport::{self, SamplingCtx, TransportOutcome};
use super::{context, is_cancelled, iteration, streaming, AgentLoop, LoopState};

/// Outcome of a single sampling request inside a turn.
///
/// The fields mirror codex's `SamplingRequestResult` shape but are
/// `pub(crate)` per Rule 3.1 — they never cross the crate boundary.
pub(crate) struct SamplingRequestResult {
    /// Whether the turn loop should continue to another sampling
    /// request based purely on the model's signal (`ToolUse` or
    /// `MaxTokens` with pending tool calls). Combined with the
    /// `injected_continuation` outcome from
    /// [`super::turn::run_turn_stop_hooks`] to produce the final
    /// `needs_follow_up` decision in the turn loop.
    pub(crate) needs_follow_up: bool,
    /// `true` when the sampling failed in a way that the turn loop
    /// must observe (fatal model error, cancellation). In this case
    /// the loop must break and not run stop hooks — the result has
    /// already been mutated with `llm_error` / `cancelled`.
    pub(crate) broke_for_error: bool,
}

/// Drive one sampling request to completion (Phase 4 unified body).
///
/// Mirrors the per-iteration body of the pre-Phase-3 `run_inner`
/// loop with the dual buffered/pump split collapsed behind
/// [`super::transport::ModelTransport`]. Returns a
/// [`SamplingRequestResult`] that lets the enclosing turn loop decide
/// whether to continue with another sampling request.
///
/// The argument list intentionally holds every dependency the body
/// touches (provider, executor, tools, event sink, cancellation
/// token, mutable `LoopState`, iteration counter) so the function
/// stays free-standing and trivially callable from
/// [`super::turn::run_turn`]. Phase 8 will collapse these into a
/// `TurnCtx` struct; until then we suppress
/// `clippy::too_many_arguments` rather than introduce a one-shot
/// builder type that would be thrown away almost immediately.
#[allow(clippy::too_many_arguments)] // Phase 8 collapses into TurnCtx.
#[instrument(
    name = "sampling",
    skip_all,
    fields(iter = iteration),
)]
pub(crate) async fn run_sampling_request(
    agent: &AgentLoop,
    provider: &dyn ModelProvider,
    executor: &dyn AgentToolExecutor,
    tools: &[ToolDefinition],
    event_tx: Option<&Sender<AgentLoopEvent>>,
    cancellation_token: Option<&CancellationToken>,
    state: &mut LoopState,
    iteration: usize,
) -> SamplingRequestResult {
    if is_cancelled(cancellation_token) {
        debug!("Cancellation requested before sampling, stopping loop");
        return SamplingRequestResult {
            needs_follow_up: false,
            broke_for_error: true,
        };
    }

    state.begin_iteration(&agent.config, iteration);
    let iteration_started_at = Instant::now();

    match context::compact_if_needed(&agent.config, state, tools, iteration) {
        context::CompactionOutcome::NeedsSummary(input) => {
            agent
                .apply_summary_compaction(
                    provider,
                    tools,
                    event_tx,
                    cancellation_token,
                    state,
                    input,
                )
                .await;
        }
        context::CompactionOutcome::Applied(tier) => {
            debug!(?tier, "local compaction applied before model call");
        }
        context::CompactionOutcome::None => {}
    }

    let request = match state.build_request(&agent.config, tools, iteration) {
        Ok(r) => r,
        Err(e) => {
            iteration::LlmCallError::Fatal(e.to_string()).apply(&mut state.result, event_tx);
            return SamplingRequestResult {
                needs_follow_up: false,
                broke_for_error: true,
            };
        }
    };

    // Phase 4 + 7: route the model call through the pump transport
    // returned by `active_transport`. The transport produces a
    // [`TransportOutcome`] that flattens to a
    // `(ModelResponse, ToolBatch)` pair so the post-sample tail
    // stays uniform.
    let transport_impl = transport::active_transport();
    let sampling_ctx = SamplingCtx {
        agent,
        provider,
        executor,
        tools,
        event_tx,
        cancellation_token,
        input_queue: None,
        state,
        request,
        iteration,
    };
    let outcome = match transport_impl.sample(sampling_ctx).await {
        Ok(o) => o,
        Err(e) => {
            e.apply(&mut state.result, event_tx);
            return SamplingRequestResult {
                needs_follow_up: false,
                broke_for_error: true,
            };
        }
    };

    let (response, batch) = match outcome {
        TransportOutcome::Streamed {
            response,
            pre_executed,
        } => (response, ToolBatch::PreExecuted(pre_executed)),
        TransportOutcome::Cancelled => {
            debug!("transport observed cancellation; bailing the sampling request");
            return SamplingRequestResult {
                needs_follow_up: false,
                broke_for_error: true,
            };
        }
    };

    iteration::accumulate_response(&agent.config, state, &response, iteration);
    state.result.iterations = iteration + 1;
    streaming::emit_iteration_complete(event_tx, iteration, &response, iteration_started_at);

    // Stop fired during or right after sampling: bail before
    // dispatching a fresh batch UNLESS the pump pre-executed a
    // batch carrying `[CANCELLED]` synthetic tool_results — those
    // must still flow through `dispatch` so the Anthropic
    // `tool_use ↔ tool_result` adjacency contract stays intact.
    // When the batch is empty (no in-flight tools, e.g.
    // cancellation arrived before any `OutputItemDone(tool_use)`),
    // we keep the fast bail-out path.
    if is_cancelled(cancellation_token) && batch.is_empty() {
        debug!("Cancellation observed after model call; skipping tool dispatch");
        return SamplingRequestResult {
            needs_follow_up: false,
            broke_for_error: true,
        };
    }

    let tool_ctx = ToolEffectCtx {
        executor,
        event_tx,
        cancellation_token,
    };
    let dispatch_says_break =
        tool_pipeline::dispatch(agent, state, &response, batch, tool_ctx).await;

    SamplingRequestResult {
        needs_follow_up: !dispatch_says_break,
        broke_for_error: false,
    }
}
