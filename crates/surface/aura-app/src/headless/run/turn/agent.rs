//! A turn's agent loop, run until it ends, a run limit stops it or the
//! run's time runs out, its events streamed and counted against the
//! run's limits, while the notices of the agent's background work are
//! steered into it.

use crate::event_loop::handlers::drive_turn;
use crate::event_loop::steering::Steering;
use crate::headless::output::ExitReason;
use crate::headless::run::drive::Driving;
use crate::headless::sink::{CallLimits, HeadlessSink, TurnTally};
use aura_agent::{AgentError, AgentLoop, AgentLoopResult};
use aura_agent_trace::RunRecorder;
use aura_model::ToolDefinition;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// How long a turn may keep running after `--max-time` cancelled it
/// before it is dropped, losing the turn's unsaved steps.
const CANCEL_GRACE: Duration = Duration::from_secs(60);

/// Events a turn's agent loop may emit ahead of the sink reading them.
const EVENT_BUFFER: usize = 1024;

/// What running a turn's agent loop came to.
pub(super) struct Ran {
    /// How the loop ended; `None` when it did not stop in time after
    /// `--max-time`
    pub(super) result: Option<Result<AgentLoopResult, AgentError>>,
    /// What the sink saw of the turn
    pub(super) tally: TurnTally,
    /// What was sent into the turn
    pub(super) steering: Steering,
    /// Notices that came once the turn no longer took input
    pub(super) late: Vec<String>,
    /// The run limit that stopped the turn, if one did
    pub(super) limit: Option<ExitReason>,
}

impl Driving<'_> {
    /// Run `agent_loop`, offering `tools`, on the run's session until it
    /// ends or something stops it.
    pub(super) async fn run_agent(
        &mut self,
        agent_loop: &AgentLoop,
        tools: Vec<ToolDefinition>,
    ) -> Ran {
        let cancel = self.run.interrupt.child_token();
        let steering = Steering::new(&cancel);
        let (events, received) = mpsc::channel(EVENT_BUFFER);
        let context_limit = agent_loop.config().max_context_tokens;
        let forwarder = tokio::spawn(self.sink(context_limit, &cancel).forward(received));
        // Cancelling at the deadline lets the loop stop between steps and
        // keep what it did; the timeout below is only a backstop.
        let out_of_time = Arc::new(AtomicBool::new(false));
        let watcher = (self.run.deadline).map(|deadline| watch(deadline, &out_of_time, &cancel));
        let backstop = (self.run.deadline)
            .map(|deadline| deadline.saturating_duration_since(Instant::now()) + CANCEL_GRACE);
        let kernel = Arc::clone(&self.session.kernel);
        // Listed as running so the subtasks the turn dispatches are told
        // apart from those of the next turn.
        let hub = &self.headless.hub;
        hub.start_running(kernel.agent_id, Arc::clone(&kernel), cancel.clone(), None);
        let turn = drive_turn(
            self.session,
            agent_loop,
            tools,
            events,
            &cancel,
            &steering,
            backstop,
        );
        let (result, late) = self.headless.steer_into(turn, &kernel, &steering).await;
        self.headless.hub.stop_running(kernel.agent_id);
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        let tally = forwarder.await.unwrap_or_else(|e| {
            warn!(error = %e, "Headless event sink panicked");
            TurnTally::default()
        });
        self.session.usage.add(&tally.usage);
        if let Some(trace) = &self.headless.trace {
            end_trace(trace, result.as_ref());
        }
        let limit = if self.run.interrupt.is_cancelled() {
            Some(ExitReason::Interrupted)
        } else if out_of_time.load(Ordering::Acquire) {
            Some(ExitReason::MaxTime)
        } else {
            tally.limit_hit
        };
        Ran {
            result,
            tally,
            steering,
            late,
            limit,
        }
    }

    /// The sink a turn's events go through: it streams them, counts the
    /// turn's calls against the run's limits on top of those of its
    /// earlier turns, cancelling `cancel` when one is reached, and traces
    /// them when the run is traced. `context_limit` is the turn's context
    /// window.
    fn sink(&self, context_limit: Option<u64>, cancel: &CancellationToken) -> HeadlessSink {
        let limits = CallLimits {
            max_calls: self.run.limits.max_turns,
            max_cost_usd: self.run.limits.max_cost_usd,
        };
        let before = self.totals.usage.clone();
        let output = self.run.output.clone();
        HeadlessSink::new(output, before, limits, cancel.clone(), context_limit)
            .with_trace(self.headless.trace.clone())
    }
}

/// Cancel a turn through `cancel` at `deadline`, noting in `out_of_time`
/// that the run's time ran out.
fn watch(
    deadline: Instant,
    out_of_time: &Arc<AtomicBool>,
    cancel: &CancellationToken,
) -> JoinHandle<()> {
    let out_of_time = Arc::clone(out_of_time);
    let cancel = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep_until(deadline.into()).await;
        out_of_time.store(true, Ordering::Release);
        cancel.cancel();
    })
}

/// End `trace`'s turn with how its agent loop ended: `result`, or the
/// error it ended in; `None` when it did not stop in time.
fn end_trace(trace: &RunRecorder, result: Option<&Result<AgentLoopResult, AgentError>>) {
    match result {
        Some(Ok(result)) => trace.end_turn(result),
        failed => trace.end_turn(&AgentLoopResult {
            llm_error: Some(failed.and_then(|r| r.as_ref().err()).map_or_else(
                || "the turn did not stop in time".to_string(),
                ToString::to_string,
            )),
            ..AgentLoopResult::default()
        }),
    }
}
