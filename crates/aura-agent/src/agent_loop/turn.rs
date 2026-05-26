//! Turn driver (Layer E.1).
//!
//! A *turn* is the unit of agent work between the model "starting to
//! talk" and "going quiet without a follow-up signal". Codex's turn
//! loop at [codex-rs/core/src/session/turn.rs:131-355](
//! https://github.com/.../codex-rs/core/src/session/turn.rs) runs a
//! sequence of sampling requests until
//!
//! ```text
//! needs_follow_up = model_says_continue
//!     || has_pending_input        // E.2 hook
//!     || stop_hook_injected_more  // Phase 1.B migration target
//! ```
//!
//! evaluates to `false`. Aura's E.1 cut wires the same predicate but
//! keeps the `has_pending_input` path as a const-`false` stub: E.2
//! will land the `input_queue` and replace the stub.
//!
//! Phase 1.B's continuation runtime was previously called as
//! `post_iteration_checks` inside the linear `for iteration` body
//! ([crates/aura-agent/src/agent_loop/mod.rs](
//! crates/aura-agent/src/agent_loop/mod.rs)). E.1 lifts it into
//! [`run_turn_stop_hooks`], invoked after every sampling request that
//! did *not* terminate the loop for an unrelated reason (model fatal
//! error, cancellation, task_done stop_loop). This preserves the
//! pre-E.1 invariant that continuation only injects when the loop
//! would otherwise continue — Phase 1.B's `dev_loop_endturn_*` tests
//! still hold end-to-end after the restructure.
//!
//! Invariants:
//!
//! - The turn loop terminates as soon as `needs_follow_up == false`.
//! - `task_blocked` (max_continuation_turns exhausted) sets
//!   `StopHookOutcome::should_break = true` and the turn loop unwinds.
//! - Cancellation / fatal model errors short-circuit the loop without
//!   running stop hooks (the result is already finalised).

use aura_reasoner::{ModelProvider, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;
use crate::types::AgentToolExecutor;
use crate::{helpers, AgentError};

use super::sampling::{run_sampling_request, SamplingRequestResult};
use super::{context, continuation, streaming, AgentLoop, LoopState, TaskId};

/// Result of a single turn.
///
/// Fields capture just enough context to let the outer task shell
/// (`task::run_task`) decide whether to keep running turns. E.2 will
/// extend this with `last_message: Option<Message>` and similar
/// values once `input_queue` lands.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnOutcome {
    /// `true` when the turn loop broke because the model signalled
    /// stop *and* no stop-hook injection requested a follow-up. E.2's
    /// `input_queue` drain will widen the "should we restart the
    /// task?" decision; for E.1 the task shell exits as soon as a
    /// turn completes cleanly.
    pub(crate) terminated_cleanly: bool,
    /// `true` when the turn loop broke because a stop hook signalled
    /// `should_break` (currently only the `task_blocked` path) or a
    /// fatal model error / cancellation was observed. The task shell
    /// reads this to skip any "restart on pending input" behavior.
    pub(crate) broke_for_error: bool,
    /// Number of sampling requests completed inside this turn. Useful
    /// for debug logging and for the outer task shell's
    /// `max_iterations_per_task` accounting.
    pub(crate) sampling_count: u32,
}

/// Outcome of [`run_turn_stop_hooks`].
///
/// Encodes the three orthogonal post-sampling signals — checkpoint
/// emission is a side-effect handled inside the function, so it does
/// not need its own bit here.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StopHookOutcome {
    /// `true` when a continuation steering message was appended to
    /// `state.messages` this iteration. The turn loop folds this into
    /// `needs_follow_up` so the next sampling sees the steering
    /// message.
    pub(crate) injected_continuation: bool,
    /// `true` when the loop must terminate (task_blocked path,
    /// budget exhausted). The turn loop breaks and the task shell
    /// observes `TurnOutcome::broke_for_error`.
    pub(crate) should_break: bool,
}

/// Drive one turn to completion.
///
/// The loop body is the codex-shaped polarity flip: each iteration
/// runs one sampling request, then asks `needs_follow_up?` (model
/// signal OR stop-hook injection). When the answer is `false` the
/// turn terminates; otherwise the loop continues.
///
/// `iteration_offset` is the running sampling-request counter shared
/// with the task shell so that `state.result.iterations` keeps a
/// monotonically-increasing total across turns. E.1's task shell runs
/// exactly one turn per task (no `input_queue` yet), but threading the
/// offset here keeps the contract correct for E.2.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_turn(
    agent: &AgentLoop,
    provider: &dyn ModelProvider,
    executor: &dyn AgentToolExecutor,
    tools: &[ToolDefinition],
    event_tx: Option<&Sender<AgentLoopEvent>>,
    cancellation_token: Option<&CancellationToken>,
    state: &mut LoopState,
    task_id: TaskId,
    turn_index: u32,
    iteration_offset: u32,
) -> Result<TurnOutcome, AgentError> {
    let mut sampling_count: u32 = 0;
    let mut terminated_cleanly = false;
    let mut broke_for_error = false;

    loop {
        let iteration =
            usize::try_from(iteration_offset.saturating_add(sampling_count)).unwrap_or(usize::MAX);

        // Hard ceiling: max_iterations is the pre-E.1 global cap
        // (default `usize::MAX`). Trip it BEFORE the next sampling so
        // we never pay for one more model call past the budget.
        if agent.config.max_iterations != usize::MAX && iteration >= agent.config.max_iterations {
            return Err(AgentError::TurnBudgetExceeded {
                task_id,
                turn_index,
            });
        }

        let sampling_result: SamplingRequestResult = run_sampling_request(
            agent,
            provider,
            executor,
            tools,
            event_tx,
            cancellation_token,
            state,
            iteration,
        )
        .await;

        sampling_count = sampling_count.saturating_add(1);

        if sampling_result.broke_for_error {
            broke_for_error = true;
            break;
        }

        // Codex shape: `needs_follow_up` defaults to "continue".
        // When the model signals follow-up (ToolUse / MaxTokens with
        // pending), the post-sampling stop hooks run (preserving
        // Phase 1.B's "checkpoint + continuation + budget warnings"
        // semantics from pre-E1 `post_iteration_checks`). When the
        // model signals stop, the turn ends — E.4 will hand the
        // continuation decision off to `GoalRuntime` and re-enable
        // injection on the "stop" path; for E.1 we keep the pre-E.1
        // semantic that stop ends the turn.
        if sampling_result.needs_follow_up {
            let stop_outcome =
                run_turn_stop_hooks(&agent.config, event_tx, state, iteration).await?;
            if stop_outcome.should_break {
                broke_for_error = true;
                break;
            }
            // `injected_continuation` is informational here — the
            // loop continues either way (a steering message has been
            // appended, the next sampling will pick it up).
            continue;
        }

        terminated_cleanly = true;
        break;
    }

    Ok(TurnOutcome {
        terminated_cleanly,
        broke_for_error,
        sampling_count,
    })
}

/// Run the post-sampling stop hooks for a single turn iteration.
///
/// Successor to the pre-E.1 `post_iteration_checks` free function in
/// [`super`]. Owns three responsibilities, preserved from Phase 1.B:
///
/// 1. Emit the first-write checkpoint warning at most once per run.
/// 2. Invoke the Phase 1.B continuation runtime
///    ([`continuation::ContinuationState::on_iteration_end`]) and
///    inject the rendered nudge / blocked envelope when the streak
///    increments without a write. After `max_continuation_turns`
///    injections without progress, fail the task with
///    `task_blocked` (sets `should_break = true`).
/// 3. Emit budget warnings and trip the credit-budget stop.
///
/// E.4 will rehome the continuation decision into `GoalRuntime`; this
/// function is the temporary host for the logic until then.
pub(crate) async fn run_turn_stop_hooks(
    config: &super::AgentLoopConfig,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
    iteration: usize,
) -> Result<StopHookOutcome, AgentError> {
    let mut outcome = StopHookOutcome::default();

    context::emit_checkpoint_if_needed(event_tx, state);

    if maybe_inject_continuation(config, event_tx, state, iteration, &mut outcome) {
        outcome.should_break = true;
        return Ok(outcome);
    }

    context::check_budget_warnings(config, event_tx, state, iteration);
    if context::should_stop_for_budget(config, state, iteration) {
        state.result.timed_out = true;
        outcome.should_break = true;
    }

    Ok(outcome)
}

/// Phase 1.B continuation injection, lifted verbatim from the pre-E.1
/// `agent_loop::maybe_inject_continuation` free function.
///
/// Returns `true` when the loop must terminate (task_blocked path).
/// On a successful injection, sets
/// [`StopHookOutcome::injected_continuation`] and returns `false` so
/// the turn loop continues for at least one more sampling request.
fn maybe_inject_continuation(
    config: &super::AgentLoopConfig,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
    iteration: usize,
    outcome: &mut StopHookOutcome,
) -> bool {
    if !config.dev_loop_completion_required {
        return false;
    }
    // A clean `task_done` already terminates the loop in the sampling
    // request's `dispatch_stop_reason` path; the continuation runtime
    // only fires on iterations that the loop intends to continue.
    if state.task_done_completed {
        return false;
    }
    // Placeholder until the iteration_read_paths plumbing lands;
    // ContinuationState's nudge/blocked decision is on the diff alone,
    // so the empty set keeps the streak counter correct. The
    // blocker_signature follow-up will replace this with the real
    // set of read paths from this iteration.
    let read_paths = std::collections::HashSet::new();
    let Some(kind) = state
        .continuation
        .on_iteration_end(&state.turn_diff, read_paths)
    else {
        return false;
    };

    if state.total_continuation_turns >= config.max_continuation_turns {
        // task_blocked: there is no dedicated `AgentLoopResult` variant
        // for "blocked-after-N-continuations" today, so we co-opt the
        // existing failure shape — `stalled = true` + an `llm_error`
        // string with the canonical `task_blocked:` prefix so the
        // harness / dashboards can grep for it. Follow-up should
        // introduce a dedicated bool / enum variant.
        let reason = format!(
            "task_blocked: max_continuation_turns ({}) exceeded without a write at iteration {}",
            config.max_continuation_turns, iteration
        );
        tracing::warn!(
            iteration,
            consecutive_no_write = state.continuation.consecutive_no_write,
            total_continuation_turns = state.total_continuation_turns,
            "{reason}"
        );
        state.result.stalled = true;
        if state.result.llm_error.is_none() {
            state.result.llm_error = Some(reason.clone());
        }
        streaming::emit(event_tx, AgentLoopEvent::Warning(reason));
        return true;
    }

    let body = continuation::render(kind, iteration, state.continuation.consecutive_no_write);
    helpers::append_warning(&mut state.messages, &body);
    streaming::emit(event_tx, AgentLoopEvent::Warning(body));
    state.total_continuation_turns = state.total_continuation_turns.saturating_add(1);
    outcome.injected_continuation = true;
    false
}
