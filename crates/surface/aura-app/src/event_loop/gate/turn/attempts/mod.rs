//! The attempts at a turn Gate labelled. The first runs on Gate's
//! choice; one whose route refused the user's sign-in goes on on Gate's
//! next choice; one that checked nothing gets Gate's follow-up and may
//! carry on on its model; and one that failed goes to Gate's judge and,
//! unless the judge excuses it, on to the next stronger model.
//!
//! - `refused`: the move off a route that refused the user's sign-in
//! - `carry`: Gate's follow-up, and carrying on on the same model
//! - `stronger`: the judge of a failure, and the move to a stronger model

mod carry;
mod refused;
mod stronger;

use super::ending::Ending;
use super::route::GateTurn;
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::gate::followup::{ended_signal, may_review, Before, Following};
use crate::event_loop::turns::{Ran, TurnParts};
use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::{Difficulty, Hash, OutcomeSignal, PromptCategory};
use aura_gate::attempts::Terms;
use aura_gate::evidence::outcome::calls_for_stronger;
use aura_gate::evidence::verifiable;
use aura_gate::judge::criteria::Criteria;
use std::sync::Arc;
use std::time::Instant;
use stronger::Failure;

/// How an attempt's run ended: its result, and what its events told.
type AttemptRun = (Result<AgentLoopResult, AgentError>, ForwarderState);

/// The prompt a turn answers, for the attempts at it.
pub(super) struct Asked<'a> {
    /// What the user asked
    pub task: &'a str,
    pub category: PromptCategory,
    /// How hard the classifier judged it
    pub difficulty: Option<Difficulty>,
    /// Whether Gate had the turn led
    pub led: Option<bool>,
    /// What its work is checked against, written down before it started
    pub criteria: Option<Arc<Criteria>>,
    /// When the turn started, in Unix milliseconds
    pub started_ms: u64,
    /// The first attempt
    pub first: Current,
}

impl Asked<'_> {
    /// What the turn's attempts run under, for Gate's attempt log.
    pub(super) fn terms(&self) -> Terms {
        Terms {
            difficulty: self.difficulty,
            led: self.led,
            routed: true,
        }
    }
}

/// An attempt at a turn: the recorded prompt it answers, where its
/// messages start in the session, and when it started.
#[derive(Debug, Clone, Copy)]
pub(super) struct Current {
    pub prompt_tx: Hash,
    pub turn_start: usize,
    pub started: Instant,
}

/// What a turn's attempts leave for its outcome, besides the last one's
/// result.
#[derive(Default)]
struct Attempts {
    /// Models whose routes refused the user's sign-in
    refused: Vec<String>,
    /// What the last attempt's runs before its final one cost
    cost_usd: f64,
    /// Whether the last attempt's outcome is already recorded
    settled: bool,
    /// What the last attempt ended with once Gate's follow-up had its say:
    /// a harness check's result, or the judge's verdict on unchecked work.
    /// `None` when Gate did not follow it up
    signal: Option<OutcomeSignal>,
}

/// Run the attempts at the turn of the prompt `asked` on `session`. When
/// Gate routes it (`gate`), Gate is asked again while a route refuses the
/// user's sign-in. An attempt that ran no check of its own gets Gate's
/// follow-up first (see [`followup`](crate::event_loop::gate::followup)):
/// when it changed files it is asked once to check them, then the
/// harness runs the project's tests, and work still unchecked gets the
/// judge's verdict. Work that is hard or shows a sign of failure is
/// reviewed against the request before it stands; what the review finds
/// unfinished goes back to the same model once. An attempt that fails (a
/// model error, its last check or the harness's failing, or the judge
/// finding its work unfinished) is handled here:
///
/// 1. its `Outcome` is recorded and, for a failed check or an error,
///    Gate's judge says whether the model was at fault
///    ([`live::judge_failed`](aura_gate::live::judge_failed));
/// 2. a failure the judge excuses ends the turn where it is;
/// 3. otherwise the turn carries on in the same conversation on the
///    next stronger model, told what the judge found, under a prompt of
///    its own recorded with route reason `escalate`.
///
/// A failure with no judge and no stronger model is left for the turn's
/// [`Ending`] to record.
pub(super) async fn run(
    session: &mut AgentSession,
    turn: &mut TurnParts<'_>,
    gate: Option<Box<GateTurn>>,
    asked: Asked<'_>,
) -> Ran {
    let Some(gate) = gate else {
        let (result, forwarded) = turn.attempt(session).await;
        let ending = Ending::new(&asked, asked.first, None);
        return Ran {
            result,
            forwarded,
            ending: Some(ending),
            refused: Vec::new(),
        };
    };
    let mut attempting = Attempting {
        session,
        turn,
        gate,
        current: asked.first,
        asked,
        before: None,
        follow: Following::default(),
        attempts: Attempts::default(),
    };
    attempting.watch().await;
    let ran = attempting.run().await;
    attempting.end(ran)
}

/// A turn Gate routes, while its attempts run.
struct Attempting<'a, 'p> {
    session: &'a mut AgentSession,
    turn: &'a mut TurnParts<'p>,
    gate: Box<GateTurn>,
    asked: Asked<'a>,
    /// The attempt running, or that last ran
    current: Current,
    /// The workspace as it was when the turn started, when a review may
    /// want to see what the turn changed
    before: Option<Before>,
    follow: Following,
    attempts: Attempts,
}

impl Attempting<'_, '_> {
    /// Start watching the turn's attempts: the files they change, so the
    /// judge can see what a failed one changed, and the workspace as it
    /// is now, so a review sees everything the turn changes, by its
    /// commands as well as its file tools.
    async fn watch(&mut self) {
        let session = &*self.session;
        if let Some(files) = &session.file_checkpoints {
            files.begin_attempt(session.agent_id());
        }
        let (live, candidates) = (&self.gate.live, &self.gate.candidates);
        let model = &self.turn.agent_loop.config().model;
        if verifiable(self.asked.category) && may_review(live, candidates, model) {
            self.before = Before::of_workspace(session.kernel.agent_workspace()).await;
        }
    }

    /// Run attempts until one stands; returns how the last one ended.
    async fn run(&mut self) -> AttemptRun {
        loop {
            let ran = self.turn.attempt(self.session).await;
            if self.turn.cancel.is_cancelled() || !self.goes_on(&ran).await {
                return ran;
            }
        }
    }

    /// Whether another attempt follows the one that ended as `ran`, set
    /// up when one does (see [`run`]).
    async fn goes_on(&mut self, ran: &AttemptRun) -> bool {
        let (result, forwarded) = ran;
        if forwarded.credentials_rejected {
            return self.reroute(ran).await;
        }
        let model = self.turn.agent_loop.config().model.clone();
        let mut signal = ended_signal(result, self.current.turn_start);
        let (mut harness, mut unchecked) = (None, None);
        if let Ok(partial) = result {
            let followed = self.follow_up(partial, &model, signal).await;
            if let Some(carry_on) = followed.carry_on {
                self.carry_on(partial, forwarded, carry_on).await;
                return true;
            }
            (signal, harness, unchecked) = (followed.signal, followed.harness, followed.wrong);
        }
        let signal = signal.unwrap_or(OutcomeSignal::Unavailable);
        self.attempts.signal = Some(signal);
        let failure = Failure {
            result,
            forwarded,
            model: &model,
            signal,
            harness,
            unchecked,
        };
        calls_for_stronger(signal) && self.escalate(failure).await
    }

    /// The turn as its last attempt, which ended as `ran`, left it, with
    /// what Gate records of it once its result is handled.
    fn end(self, (result, forwarded): AttemptRun) -> Ran {
        if let Some(files) = &self.session.file_checkpoints {
            let _ = files.take_attempt(self.session.agent_id());
        }
        let ending = (!self.attempts.settled).then(|| {
            let changed = (result.as_ref().ok())
                .and_then(|result| self.follow.steps.changed_paths(result))
                .map(<[String]>::to_vec);
            Ending {
                cost_usd: self.attempts.cost_usd,
                signal: self.attempts.signal,
                changed,
                ..Ending::new(&self.asked, self.current, Some(Arc::clone(&self.gate.hub)))
            }
        });
        Ran {
            result,
            forwarded,
            ending,
            refused: self.attempts.refused,
        }
    }
}
