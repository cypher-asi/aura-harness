//! How a turn ended: its `Outcome` when Gate routed it, what the session
//! keeps of it, and why the run stops after it, if it does.

use super::agent::Ran;
use super::start::Started;
use super::TurnOutcome;
use crate::event_loop::gate::followup::{ended_signal, failure_text};
use crate::event_loop::handlers::record_response;
use crate::event_loop::turns::record_prompt;
use crate::event_loop::AgentSession;
use crate::headless::output::ExitReason;
use crate::headless::run::drive::{Driving, Stop};
use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::{OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload};
use aura_model::Role;
use std::time::Instant;
use tracing::warn;

/// How the agent loop of a turn ended: `None` when it did not stop in
/// time after `--max-time`.
type Ended = Option<Result<AgentLoopResult, AgentError>>;

impl Driving<'_> {
    /// How the turn `started` set up, which began at `at`, ended once its
    /// agent loop came to `ran`.
    pub(super) async fn end_turn(
        &mut self,
        started: Started,
        ran: Ran,
        at: Instant,
    ) -> TurnOutcome {
        let Ran {
            result,
            tally,
            steering,
            mut late,
            limit,
        } = ran;
        let messages = (result.as_ref().and_then(|r| r.as_ref().ok()))
            .map_or(&[][..], |r| r.messages.as_slice());
        let (seen, unseen) = steering.finish(messages);
        let mut unseen: Vec<String> = unseen.into_iter().map(|(_, text)| text).collect();
        unseen.append(&mut late);
        let mut outcome = TurnOutcome {
            unseen,
            turn_start: started.turn_start,
            terms: started.terms,
            ..TurnOutcome::default()
        };
        if let Some(category) = started.category {
            let (own, failure) = own_signal(&result, limit, started.turn_start);
            let signal = own.unwrap_or(OutcomeSignal::Unavailable);
            let cost_usd = tally.usage.cost_usd;
            outcome.ended = Some(self.outcome_of(&started, (category, signal), cost_usd, at));
            outcome.failure = failure;
            outcome.unchecked = own.is_none();
            outcome
                .auth_token
                .clone_from(&started.agent_loop.config().auth_token);
        }
        outcome.usage = tally.usage;
        self.keep(&mut outcome, result, limit, &seen).await;
        outcome
    }

    /// The `Outcome` of the turn `started` set up, whose work asked for
    /// a category and ended with a signal, which cost `cost_usd` and
    /// began at `at`. A turn whose model was asked to check its change
    /// speaks for the turn before it, that turn's cost and time included.
    fn outcome_of(
        &self,
        started: &Started,
        (category, signal): (PromptCategory, OutcomeSignal),
        cost_usd: f64,
        at: Instant,
    ) -> OutcomePayload {
        let agent = self.session.agent_id();
        let mut outcome = OutcomePayload {
            prompt_tx: started.prompt_tx,
            category,
            model: started.agent_loop.config().model.clone(),
            success: signal.success(),
            signal,
            cost_usd,
            subtask_cost_usd: self
                .headless
                .hub
                .subtask_cost_usd(agent, started.started_ms),
            latency_ms: u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        if let Some(before) = &started.nudged {
            outcome.prompt_tx = before.prompt_tx;
            outcome.cost_usd += before.cost_usd;
            outcome.subtask_cost_usd += before.subtask_cost_usd;
            outcome.latency_ms = outcome.latency_ms.saturating_add(before.latency_ms);
        }
        outcome
    }

    /// Keep in `outcome` what its turn left in `result`, the turn having
    /// been stopped by a run `limit` when one did, and in the session its
    /// conversation, with the notices `seen` mid-turn; and say why the
    /// run stops after it, if it does.
    async fn keep(
        &mut self,
        outcome: &mut TurnOutcome,
        result: Ended,
        limit: Option<ExitReason>,
        seen: &[String],
    ) {
        match result {
            Some(Ok(result)) => {
                commit(self.session, &result, seen).await;
                outcome.stop = turn_stop(limit, &result);
                // The model's call failed, not the run: its conversation
                // is kept, so another model can go on from it.
                outcome.can_go_on =
                    limit.is_none() && !result.insufficient_credits && result.llm_error.is_some();
                outcome.result = Some(result);
            }
            Some(Err(e)) => {
                let message = e.to_string();
                outcome.stop = Some((limit.unwrap_or(ExitReason::Error), Some(message)));
            }
            None => {
                outcome.stop = Some((
                    ExitReason::MaxTime,
                    Some("the turn did not stop in time after --max-time".to_string()),
                ));
            }
        }
    }
}

/// The signal of a turn that ended with `result`, its own messages
/// starting at `turn_start`, and how it failed in words; `None` for the
/// signal when nothing says yet how it went. A turn a run `limit` stopped
/// is cancelled: the run's own limits say nothing about the model.
fn own_signal(
    result: &Ended,
    limit: Option<ExitReason>,
    turn_start: usize,
) -> (Option<OutcomeSignal>, Option<String>) {
    match (result, limit) {
        (Some(result), None) => (
            ended_signal(result, turn_start),
            failure_text(result, turn_start),
        ),
        _ => (Some(OutcomeSignal::Cancelled), None),
    }
}

/// Whether the agent ended `result` on its own: a final reply with no
/// call left unanswered and no model error.
fn finished_naturally(result: &AgentLoopResult) -> bool {
    result.llm_error.is_none()
        && result
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Assistant && !m.has_tool_use())
}

/// Why the run stops after a turn that returned `result`, if it does.
fn turn_stop(limit: Option<ExitReason>, result: &AgentLoopResult) -> Option<Stop> {
    match limit {
        // A limit that hit as the agent wrote its final reply stopped
        // nothing.
        Some(ExitReason::Interrupted) => Some((ExitReason::Interrupted, None)),
        Some(reason) if !finished_naturally(result) => Some((reason, None)),
        _ if result.insufficient_credits => {
            Some((ExitReason::Error, Some("insufficient credits".to_string())))
        }
        _ => match &result.llm_error {
            Some(err) if result.total_text.trim().is_empty() => {
                Some((ExitReason::Error, Some(err.clone())))
            }
            _ => None,
        },
    }
}

/// Keep what a turn produced: its conversation, todo list, the notices
/// the model got mid-turn, and its reply, then checkpoint the session.
async fn commit(session: &mut AgentSession, result: &AgentLoopResult, seen: &[String]) {
    session.messages.clone_from(&result.messages);
    session.todos.clone_from(&result.todos);
    for text in seen {
        if let Err(problem) = record_prompt(session, &PromptPayload::new(text.as_str())).await {
            warn!(%problem, "Failed to record a mid-turn notice");
        }
    }
    record_response(session, &result.total_text).await;
    session.save_checkpoint().await;
}
