//! Task shell loop.
//!
//! A *task* is the outermost unit of agent work: it owns the
//! conversation state and (via the enclosing [`crate::session::Session`])
//! the `input_queue` that lets the user steer the agent mid-task.
//! The shell mirrors codex's `tasks::regular::run` shape:
//!
//! ```text
//! loop {
//!     run_turn(...);
//!     if !input_queue.has_pending() { return; }
//! }
//! ```
//!
//! [`run_task`] threads the nesting `run_task → [`super::turn::run_turn`] →
//! [`super::sampling::run_sampling_request`]` and trusts the model's
//! `EndTurn` stop reason as the authoritative end-of-task signal.
//! The `input_queue` is always present today (session-scoped, allocated
//! by [`crate::session::Session`]); the `has_pending()` probe at the
//! end of every turn decides whether to spin another turn or return
//! to the caller. The pre-codex-parity continuation runtime is gone
//! (Phase 7 deleted the placeholder `GoalRuntime`); the only
//! persistent cross-turn state is the conversation history on
//! [`super::LoopState`].
//!
//! The task shell owns two safety nets per Rule 4.3:
//!
//! - [`super::AgentLoopConfig::max_turns_per_task`]: hard cap on how
//!   many turns one task can run. Default `50` matches the codex
//!   pattern.
//! - [`super::AgentLoopConfig::max_iterations_per_task`]: hard cap on
//!   the total number of sampling requests across all turns of one
//!   task. Default `500` keeps the existing long-batch workflows
//!   (e.g. multi-`create_task` extraction) inside the envelope
//!   without the silent-cancel regression that the 25-iteration cap
//!   used to cause.
//!
//! Phase 8 split the per-task budget surface into two sibling
//! [`AgentError`] variants — one carries the `max_turns_per_task`
//! limit (as [`AgentError::TurnBudgetExceeded`]) and one the
//! `max_iterations_per_task` limit (as
//! [`AgentError::IterationBudgetExceeded`]) — so callers no longer
//! need to re-derive which ceiling fired when surfacing the failure.

use aura_model_reasoner::{Message, ToolDefinition};
use tracing::{field, instrument, Span};
use uuid::Uuid;

use crate::console;
use crate::session::input_queue::InputQueue;
use crate::types::AgentLoopResult;
use crate::AgentError;

use super::cx::{RunCtx, TurnCtx};
use super::turn::{run_turn, TurnOutcome};
use super::LoopState;

/// Newtype wrapper around a `Uuid` identifying one in-flight task.
///
/// Generated at task start by [`run_task`] and threaded through the
/// turn loop so that
/// [`AgentError::TurnBudgetExceeded`](crate::AgentError::TurnBudgetExceeded)
/// can attribute the budget overrun to a specific task (Rule 4.3,
/// Rule 5.1). E.2 will extend this into a `SessionId` / `TaskId`
/// pair when the session struct lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub Uuid);

impl TaskId {
    /// Mint a fresh v4 task identifier.
    ///
    /// Used by [`run_task`] when callers do not supply one (the
    /// pre-E.1 entry points do not have access to the wider session
    /// scope where the id would otherwise be allocated).
    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new_v4()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Return the first 8 hex chars of a UUID string (the prefix is
/// universally enough to disambiguate concurrent tasks in a single
/// log file). Used to populate the `task{id=...}` span field without
/// flooding every nested log line with a 36-char UUID.
fn short_id(task_id: &str) -> &str {
    task_id.get(..8).unwrap_or(task_id)
}

/// Drive one task to completion.
///
/// Phase 8 collapsed the previous 8-parameter signature into a
/// single [`RunCtx`] borrow plus the per-run conversation inputs
/// (`messages`, `tools`). The `RunCtx` carries the model provider,
/// tool executor, optional event sink, optional cancellation token,
/// and the shared [`crate::session::Session`] handle; the inner loop
/// re-bundles those into a [`TurnCtx`] for each turn iteration.
///
/// `iteration_offset` accumulates by the per-tool-batch count
/// (`turn_outcome.sampling_count`) across turns, since input-queue
/// restarts always happen at turn boundaries — never mid-sampling.
/// This keeps `state.result.iterations` monotonically increasing and
/// also makes the `max_iterations_per_task` cap count completed
/// sampling requests, which is what the per-task budget semantics
/// document.
///
/// Returns the populated [`AgentLoopResult`] regardless of whether the
/// task terminated cleanly or short-circuited on a fatal model error.
/// Only the per-task hard ceilings (`max_turns_per_task`,
/// `max_iterations_per_task`) surface as `Err(AgentError::…)`; every
/// other failure mode is materialised on `state.result` (so the
/// pre-E.1 caller contract — "`run` always returns `Ok` with errors
/// folded into the result" — survives).
#[instrument(
    name = "task",
    skip_all,
    fields(id = field::Empty),
)]
pub(crate) async fn run_task(
    ctx: &RunCtx<'_>,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
) -> Result<AgentLoopResult, AgentError> {
    let task_id = TaskId::new_v4();
    let task_id_str = task_id.to_string();
    Span::current().record("id", field::display(short_id(&task_id_str)));

    let mut state = LoopState::new(&ctx.agent.config, messages);
    state.build_baseline = ctx.executor.capture_build_baseline().await;

    console::task_start_banner(
        &task_id_str,
        ctx.agent.config.max_turns_per_task,
        ctx.agent.config.max_iterations_per_task,
    );
    tracing::debug!(
        task_id = %task_id,
        session_id = %ctx.session.id,
        max_iterations = ctx.agent.config.max_iterations,
        max_turns_per_task = ctx.agent.config.max_turns_per_task,
        max_iterations_per_task = ctx.agent.config.max_iterations_per_task,
        "Starting agent task"
    );

    let input_queue_ref: &InputQueue = ctx.session.input_queue.as_ref();

    // E.2: turn_index / iteration_offset accumulate across turns so
    // the `max_turns_per_task` / `max_iterations_per_task` caps trip
    // on a genuine runaway. iteration_offset is bumped by the per-
    // turn `sampling_count` (= per-tool-batch count) because every
    // sampling request is one model round-trip and the input-queue
    // restart only happens at turn boundaries — never mid-sampling.
    let mut turn_index: u32 = 0;
    let mut iteration_offset: u32 = 0;

    loop {
        // Hard ceilings: surface a typed error per Rule 4.3 instead of
        // silently terminating. Trip before the next `run_turn` call
        // so we never half-execute another turn past the cap. Phase 8
        // split this into two sibling variants so callers can
        // distinguish which ceiling tripped without re-deriving from
        // `turn_index`.
        if turn_index >= ctx.agent.config.max_turns_per_task {
            return Err(AgentError::TurnBudgetExceeded {
                task_id,
                limit: ctx.agent.config.max_turns_per_task as usize,
            });
        }
        if iteration_offset >= ctx.agent.config.max_iterations_per_task {
            return Err(AgentError::IterationBudgetExceeded {
                task_id,
                limit: ctx.agent.config.max_iterations_per_task as usize,
            });
        }

        let turn_ctx = TurnCtx {
            run: ctx,
            tools: &tools,
            task_id,
            turn_index,
            iteration_offset,
            input_queue: Some(input_queue_ref),
        };
        let turn_outcome: TurnOutcome = run_turn(&turn_ctx, &mut state).await?;

        // Accumulate per-turn sampling count into the per-task
        // counters BEFORE any early-break paths so the post-turn
        // `state.result.iterations` math stays consistent even on
        // error exits.
        iteration_offset = iteration_offset.saturating_add(turn_outcome.sampling_count);
        turn_index = turn_index.saturating_add(1);

        if turn_outcome.broke_for_error {
            break;
        }
        if !turn_outcome.terminated_cleanly {
            break;
        }

        // Only spin another turn when the input queue has pending
        // entries. The queue is session-scoped (always present), so
        // the gate is a single `has_pending()` probe — codex parity
        // with `tasks::regular::run`. The pre-codex-parity
        // continuation accumulator is gone: the only cross-turn
        // state is the conversation history on [`super::LoopState`],
        // so no streak counter needs to be inherited here.
        if input_queue_ref.has_pending() {
            continue;
        }
        break;
    }

    state.result.messages = state.messages;

    for observer in &ctx.agent.config.observers {
        observer.on_turn_complete(&state.result).await;
    }

    Ok(state.result)
}
