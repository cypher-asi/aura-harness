//! Phase 8 context structs that collapse the multi-arg `async fn`
//! signatures threading through the agent loop into reusable bundles.
//!
//! The pre-Phase-8 shape carried 8–12 parameters across
//! [`super::run::AgentLoop::run_with_session`] →
//! [`super::task::run_task`] → [`super::turn::run_turn`] →
//! [`super::sampling::run_sampling_request`] with each call site re-listing
//! the same `provider` / `executor` / `event_tx` / `cancellation_token` /
//! `session` borrows. Every layer carried a `too_many_arguments`
//! to silence the resulting noise.
//!
//! The two structs in this module replace that scaffolding:
//!
//! - [`RunCtx`] holds the per-run borrowed services that stay constant
//!   from the public entry point through every nested layer (the
//!   model provider, the tool executor, the optional event sink, the
//!   optional cancellation token, and the shared
//!   [`crate::session::Session`]). Built once inside
//!   `AgentLoop::run_with_session` and threaded by reference into
//!   `run_task`.
//! - [`TurnCtx`] carries the turn-scoped identity that travels with a
//!   single turn (`task_id`, `turn_index`, `iteration_offset`, the
//!   optional [`super::InputQueue`] handle) plus a borrow of the
//!   stable `RunCtx`. Materialised inside `run_task` for each
//!   sampling iteration and passed by reference down to
//!   `run_sampling_request`.
//!
//! Lifetimes: every field is `&'a`, so the structs are zero-cost
//! aggregations — building a `RunCtx` or `TurnCtx` introduces no
//! clones at the call site (the borrow lifetimes line up with the
//! enclosing `async fn`'s body where the source values live).

use aura_model_reasoner::{ModelProvider, ToolDefinition};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::events::AgentLoopEvent;
use crate::session::input_queue::InputQueue;
use crate::session::Session;
use crate::types::AgentToolExecutor;
use crate::AgentRunnerHandle;

use super::task::TaskId;
use super::AgentLoop;

/// Bundle of the three optional coordination handles a caller can
/// supply to [`AgentLoop::run_with_session`] (and the per-runner
/// [`crate::AgentRunner::execute_chat`] passthrough).
///
/// Phase 8 introduced this struct to collapse the previous trailing
/// `event_tx, cancellation_token, handle` triple — three of which
/// pushed the public entry points past the
/// `clippy::too_many_arguments` ceiling — into a single value with
/// a documented field layout. All three fields stay [`Option`] so
/// callers can opt out of any subset (the headless / non-cancellable
/// / single-turn-per-task code paths simply construct a default
/// bundle).
#[derive(Default)]
pub struct RunOptions<'a> {
    /// Channel for streaming [`AgentLoopEvent`]s back to the caller.
    /// When `None`, the loop uses non-streaming `provider.complete()`
    /// and emits no per-iteration events.
    pub event_tx: Option<Sender<AgentLoopEvent>>,
    /// External cancellation signal honoured by the per-iteration
    /// cancel probes and by the streaming pump's
    /// `tokio::select!` shape.
    pub cancellation_token: Option<CancellationToken>,
    /// Optional session-scoped [`AgentRunnerHandle`] for mid-task
    /// user steering. When `Some`, the task shell loops on the
    /// handle's `InputQueue::has_pending()` flag at the end of
    /// every turn so queued user inputs keep the agent responsive
    /// without aborting the conversation.
    pub handle: Option<&'a AgentRunnerHandle>,
}

/// Borrowed bundle of per-run dependencies that stay constant from the
/// public `AgentLoop::run_with_session` entry point through every
/// nested layer. Built once per run; threaded by reference.
///
/// The bundle deliberately does NOT carry the conversation `messages`
/// or the request-shaping `tools` — those are inputs to a single
/// `run` and travel as separate arguments so the public entry points
/// can keep their existing shape (the tests pin a specific signature
/// for `run_with_events` and friends, which we keep intact).
pub(crate) struct RunCtx<'a> {
    pub(crate) agent: &'a AgentLoop,
    pub(crate) provider: &'a dyn ModelProvider,
    pub(crate) executor: &'a dyn AgentToolExecutor,
    pub(crate) event_tx: Option<&'a Sender<AgentLoopEvent>>,
    pub(crate) cancellation_token: Option<&'a CancellationToken>,
    /// Session-scoped handle to the shared
    /// [`crate::session::input_queue::InputQueue`] and the per-session
    /// id. Always present (allocated by the public entry point) so
    /// the inner loop never has to branch on `Option<&Session>`.
    pub(crate) session: &'a Session,
}

/// Turn-scoped bundle layered on top of [`RunCtx`].
///
/// Threads the stable `run` context plus the turn identity
/// (`task_id`, `turn_index`, `iteration_offset`) plus the active tool
/// catalog and the optional [`InputQueue`] used for mid-turn user
/// steering. Materialised inside `run_task` once per turn iteration
/// and passed by reference down to `run_turn` and the per-sample
/// helpers.
pub(crate) struct TurnCtx<'a> {
    pub(crate) run: &'a RunCtx<'a>,
    pub(crate) tools: &'a [ToolDefinition],
    pub(crate) task_id: TaskId,
    pub(crate) turn_index: u32,
    pub(crate) iteration_offset: u32,
    pub(crate) input_queue: Option<&'a InputQueue>,
}
