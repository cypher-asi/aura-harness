//! Task shell loop (Layer E.1).
//!
//! A *task* is the outermost unit of agent work: it owns the
//! conversation state, the build baseline, and (once E.2 lands) the
//! `input_queue` that lets the user steer the agent mid-task. Codex's
//! task shell at [codex-rs/core/src/tasks/regular.rs:73 analog](
//! https://github.com/.../codex-rs/core/src/tasks/regular.rs) drives
//! the pattern:
//!
//! ```text
//! loop {
//!     run_turn(...);
//!     if !input_queue.has_pending() { return; }
//! }
//! ```
//!
//! E.1 wires the nesting (`run_task` → [`super::turn::run_turn`] →
//! [`super::sampling::run_sampling_request`]) with the
//! `input_queue.has_pending()` probe stubbed out to `false`. Until
//! E.2 lands the queue, every task runs exactly one turn — preserving
//! pre-E.1 behavior where one `AgentLoop::run_inner` call drove the
//! whole conversation.
//!
//! The task shell owns two safety nets per Rule 4.3:
//!
//! - [`AgentLoopConfig::max_turns_per_task`]: hard cap on how many
//!   turns one task can run. Default `50` matches the codex pattern.
//! - [`AgentLoopConfig::max_iterations_per_task`]: hard cap on the
//!   total number of sampling requests across all turns of one task.
//!   Default `500` keeps the existing long-batch workflows
//!   (e.g. multi-`create_task` extraction) inside the envelope
//!   without the silent-cancel regression that the 25-iteration cap
//!   used to cause.
//!
//! Both ceilings surface an
//! [`AgentError::TurnBudgetExceeded`] with structured context so the
//! UI / dashboards can correlate the failure with the task that
//! produced it.

use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::events::AgentLoopEvent;
use crate::types::{AgentLoopResult, AgentToolExecutor};
use crate::AgentError;

use super::turn::{run_turn, TurnOutcome};
use super::{AgentLoop, LoopState};

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

/// Drive one task to completion.
///
/// E.1 wires the codex-shaped nesting: outer task shell calls
/// [`run_turn`] in a loop that terminates when no input is pending
/// (stubbed to `false` until E.2). Each turn drives sampling requests
/// until `needs_follow_up` evaluates to `false`.
///
/// Returns the populated [`AgentLoopResult`] regardless of whether the
/// task terminated cleanly or short-circuited on a fatal model error.
/// Only the per-task hard ceilings (`max_turns_per_task`,
/// `max_iterations_per_task`) surface as `Err(AgentError::…)`; every
/// other failure mode is materialised on `state.result` (so the
/// pre-E.1 caller contract — "`run` always returns `Ok` with errors
/// folded into the result" — survives).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_task(
    agent: &AgentLoop,
    provider: &dyn ModelProvider,
    executor: &dyn AgentToolExecutor,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    event_tx: Option<Sender<AgentLoopEvent>>,
    cancellation_token: Option<CancellationToken>,
) -> Result<AgentLoopResult, AgentError> {
    let task_id = TaskId::new_v4();
    let mut state = LoopState::new(&agent.config, messages);
    state.build_baseline = executor.capture_build_baseline().await;
    info!(
        task_id = %task_id,
        max_iterations = agent.config.max_iterations,
        max_turns_per_task = agent.config.max_turns_per_task,
        max_iterations_per_task = agent.config.max_iterations_per_task,
        "Starting agent task"
    );

    let event_tx_ref = event_tx.as_ref();
    let cancellation_ref = cancellation_token.as_ref();
    // E.1 runs at most one turn per task (no `input_queue` yet). The
    // counters start at zero and the post-turn increments are deferred
    // to E.2, which will lift the unconditional `break` below into a
    // queue-driven loop. Per-task budget ceilings stay enforced via
    // the pre-turn checks; in E.1 they only meaningfully guard
    // misconfigured callers that set `max_turns_per_task = 0`.
    let turn_index: u32 = 0;
    let iteration_offset: u32 = 0;

    // E.1 deliberately runs at most one iteration of this outer `loop`
    // — the body always falls through to the unconditional `break` at
    // the bottom because there is no `input_queue` yet. Clippy's
    // `never_loop` lint flags that, but the loop *form* is the
    // structural shape E.2 expands into a real queue-driven loop;
    // collapsing it to a single straight-line block now would force
    // a re-introduction in E.2 and obscure the topology that the rest
    // of the codex-shape plan is built on. Documented per Rule 1.4.
    #[allow(clippy::never_loop)]
    loop {
        // Hard ceiling: surface a typed error per Rule 4.3 instead of
        // silently terminating. Trip before the next `run_turn` call
        // so we never half-execute another turn past the cap.
        if turn_index >= agent.config.max_turns_per_task {
            return Err(AgentError::TurnBudgetExceeded {
                task_id,
                turn_index,
            });
        }
        if iteration_offset >= agent.config.max_iterations_per_task {
            return Err(AgentError::TurnBudgetExceeded {
                task_id,
                turn_index,
            });
        }

        let turn_outcome: TurnOutcome = run_turn(
            agent,
            provider,
            executor,
            &tools,
            event_tx_ref,
            cancellation_ref,
            &mut state,
            task_id,
            turn_index,
            iteration_offset,
        )
        .await?;

        if turn_outcome.broke_for_error {
            break;
        }
        if !turn_outcome.terminated_cleanly {
            break;
        }

        // E.1: queue is conceptually empty after every turn, so one
        // turn per task is the steady-state behaviour. E.2 will
        // replace this with
        // `if !ctx.session.input_queue.has_pending() { break; }`,
        // re-introducing the `turn_index` / `iteration_offset`
        // accumulators that the per-task ceilings above are sized
        // against.
        let _ = turn_outcome.sampling_count;
        break;
    }

    state.result.messages = state.messages;

    for observer in &agent.config.observers {
        observer.on_turn_complete(&state.result).await;
    }

    Ok(state.result)
}
