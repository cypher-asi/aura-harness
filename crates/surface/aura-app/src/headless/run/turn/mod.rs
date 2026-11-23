//! One agent turn of a run: its prompt recorded and given to the model,
//! the agent loop run under the run's limits while the notices of the
//! agent's background work are steered into it, and what the turn left.
//!
//! - `start`: the model a turn runs on, whether it leads, and its prompt
//! - `agent`: the agent loop, run until it ends or something stops it
//! - `end`: how a turn ended, its `Outcome`, and what is kept of it

mod agent;
mod end;
mod start;

use super::drive::{Driving, Stop};
use crate::event_loop::turns::drain_stale_inbox;
use crate::headless::output::ExitReason;
use crate::session_checkpoint::SessionUsage;
use aura_agent::AgentLoopResult;
use aura_core_types::{OutcomePayload, PromptCategory};
use aura_gate::attempts::Terms;
use aura_gate::route::Decision;
use std::time::Instant;

/// How one agent turn ended.
#[derive(Default)]
pub(super) struct TurnOutcome {
    /// How the turn went, when Gate routed it: the `Outcome` the run
    /// records once Gate's follow-up has had its say
    pub(super) ended: Option<OutcomePayload>,
    pub(super) result: Option<AgentLoopResult>,
    /// Model calls and tokens of the turn
    pub(super) usage: SessionUsage,
    /// Set when the turn ended the run early
    pub(super) stop: Option<Stop>,
    /// Notices the model never got, for the next turn
    pub(super) unseen: Vec<String>,
    /// How the turn failed, in words, when Gate routed it and it did
    pub(super) failure: Option<String>,
    /// Whether `stop` is a model error the turn's conversation survived,
    /// so a stronger model can still take the turn over
    pub(super) can_go_on: bool,
    /// Where the turn's own messages start in `result`
    pub(super) turn_start: usize,
    /// What the turn ran under, for Gate's attempt log
    pub(super) terms: Terms,
    /// Whether nothing has said yet how the turn went, which leaves its
    /// `Outcome` for Gate's follow-up to settle
    pub(super) unchecked: bool,
    /// The token the turn's requests were sent with, for Gate's calls
    /// about it when Gate routed it
    pub(super) auth_token: Option<String>,
}

impl TurnOutcome {
    /// A turn that never ran: `problem` ends the run, with the session's
    /// messages ending at `turn_start`.
    fn stopped(problem: String, turn_start: usize) -> Self {
        Self {
            stop: Some((ExitReason::Error, Some(problem))),
            turn_start,
            ..Self::default()
        }
    }
}

/// What a turn carries on from the one before it, which ran under the
/// `Terms` each carries.
pub(super) enum Carry {
    /// Work a weaker model failed at, on this model, with the work's
    /// category; it keeps the work's difficulty.
    Escalate(Decision, PromptCategory, Terms),
    /// The same model, asked to check the change of the turn that ended
    /// as this; the new turn's `Outcome` takes its place, and it is done
    /// the same way, of the same difficulty.
    Nudge(OutcomePayload, Terms),
}

impl Carry {
    /// What the turn carried on from ran under.
    const fn earlier(&self) -> Terms {
        match self {
            Self::Escalate(.., terms) | Self::Nudge(_, terms) => *terms,
        }
    }
}

impl Driving<'_> {
    /// Record `text` as the prompt and run one agent turn on the run's
    /// session. When Gate routes the turn, it ends with an `Outcome`
    /// saying how it went. `carry` is set for a turn that goes on from
    /// the one before: with work a weaker model failed at, or on the same
    /// model asked to check its change, which the `Outcome` then speaks
    /// for, its cost included. Either keeps the difficulty the classifier
    /// gave the work.
    pub(super) async fn run_turn(&mut self, text: String, carry: Option<Carry>) -> TurnOutcome {
        drain_stale_inbox(self.session).await;
        let started = Instant::now();
        let (turn, tools) = match self.start_turn(text, carry).await {
            Ok(ready) => ready,
            Err(problem) => return TurnOutcome::stopped(problem, self.session.messages.len()),
        };
        let ran = self.run_agent(&turn.agent_loop, tools).await;
        self.end_turn(turn, ran, started).await
    }
}
