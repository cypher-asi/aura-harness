//! What Gate does once a turn answering the run's prompt ends (see
//! [`gate::followup`](crate::event_loop::gate::followup)): it follows the
//! turn up, records how it went unless its model carries on with the
//! work, has its judge say whether the model of a failed turn was at
//! fault, and hands the work on: back to the same model when the
//! follow-up asks it to carry on, or to the next stronger model when the
//! turn ended badly. Each hand-off is streamed as a notice.

use super::drive::{Driving, Next};
use super::turn::{Carry, TurnOutcome};
use crate::event_loop::gate::followup::{
    self, from_harness, joined, At, Attempt, CarryOn, Followed, Judging, Takeover,
};
use crate::headless::output::StreamEvent;
use aura_core_types::{OutcomePayload, OutcomeSignal};
use aura_gate::evidence::outcome::calls_for_stronger;
use aura_gate::judge::turn::Failed;
use aura_gate::live;
use aura_gate::route::Decision;
use tracing::info;

impl Driving<'_> {
    /// What Gate does once a turn answering the run's prompt ended as
    /// `outcome`: its follow-up, the record of how it went and its judge.
    /// Returns the turn that carries the work on, if one does: the same
    /// model's when the follow-up hands the work back to it, else the
    /// next stronger model's when the turn ended badly.
    pub(super) async fn follow(&mut self, outcome: &mut TurnOutcome) -> Option<Next> {
        let mut found = Followed::default();
        if outcome.stop.is_none() {
            found = self.follow_up(outcome).await;
        }
        if let Some(carry_on) = found.carry_on.take() {
            self.answered.clone_from(&outcome.ended);
            return self.carry_on(carry_on, outcome);
        }
        self.settle(outcome).await;
        // The stronger model is told how the turn ended before the judge's
        // verdict replaced its signal.
        let raw = outcome.ended.as_ref().map(|ended| ended.signal);
        let review = match found.wrong.take() {
            Some(wrong) => Some(wrong),
            None => self.judge(outcome).await,
        };
        self.answered.clone_from(&outcome.ended);
        if outcome.stop.is_some() && !outcome.can_go_on {
            return None;
        }
        self.take_over(outcome, raw, joined(found.harness.take(), review))
    }

    /// Follow up the turn that ended as `outcome` as Gate's settings
    /// allow (see [`followup`]): one that ran no check of its own is asked
    /// once to check its change, else the harness runs the project's
    /// tests, else the judge looks at work that seems unfinished; and work
    /// that is hard or shows a sign of failure is reviewed before it
    /// stands, going back to its model once with what the review finds.
    /// What the harness's check, the judge or the review finds becomes
    /// the turn's `Outcome`, and a failing check of the harness's its
    /// failure.
    async fn follow_up(&mut self, outcome: &mut TurnOutcome) -> Followed {
        let (Some(ended), Some(result)) = (outcome.ended.as_mut(), outcome.result.as_ref()) else {
            return Followed::default();
        };
        let Some(candidates) = self.candidates.as_deref() else {
            return Followed::default();
        };
        let workspace = self.session.kernel.agent_workspace();
        let signal = (!outcome.unchecked).then_some(ended.signal);
        let attempt = Attempt {
            session: self.session,
            live: &self.headless.hub.gate,
            candidates,
            task: &self.task,
            ran: (ended.model.as_str(), self.headless.effort),
            result,
            at: At {
                category: ended.category,
                difficulty: outcome.terms.difficulty,
                signal,
                turn_start: self.attempt_start,
            },
            prompt_tx: Some(ended.prompt_tx),
            workspace: &workspace,
            before: self.before.as_ref(),
            criteria: self.headless.criteria.as_ref(),
            auth_token: outcome.auth_token.clone(),
        };
        let mut followed = self.follow.run(&attempt, self.run.output).await;
        if let Some(found) = followed.signal.filter(|found| Some(*found) != signal) {
            *ended = ended.with_signal(found);
            outcome.unchecked = false;
        }
        self.headless.judged.append(&mut followed.judged);
        if followed.harness.is_some() {
            outcome.failure.clone_from(&followed.harness);
        }
        followed
    }

    /// Write the turn that ended as `outcome`, when Gate routed it, to
    /// Gate's attempt log as part of the attempt so far, and record its
    /// `Outcome`.
    pub(super) async fn settle(&self, outcome: &TurnOutcome) {
        let Some(ended) = &outcome.ended else {
            return;
        };
        let left = (outcome.result.as_ref())
            .map(|result| self.follow.steps.evidence(result, self.attempt_start));
        let agent = self.session.agent_id();
        let gate = &self.headless.hub.gate;
        gate.log_attempt(agent, ended, outcome.terms, left);
        live::record(&self.session.kernel, agent, ended).await;
    }

    /// Ask Gate's judge about the turn that ended as `outcome`, when Gate
    /// routed it and it failed on a model weaker than the judge: was the
    /// model at fault? The finding is streamed and recorded as a later
    /// `Outcome` of the turn's prompt, which becomes the turn's:
    /// `judged_fail` counts against the model and sends the turn on to a
    /// stronger one, `judged_excused` counts for nothing and leaves the
    /// turn where it is. Returns what the judge found; `None` when the
    /// turn is not judged or the judge gave no verdict, and the turn's
    /// own signal stands.
    ///
    /// The judge is only asked about failures, so what it costs is paid
    /// when there is something to learn; the cost is reported on its own
    /// ([`live::Judged::cost_usd`]), not as part of the work.
    async fn judge(&mut self, outcome: &mut TurnOutcome) -> Option<String> {
        let ended = outcome.ended.as_ref()?;
        let session = &*self.session;
        let judge = self
            .headless
            .hub
            .judge_of(session.definition.as_deref(), ended)?;
        let diff = self
            .follow
            .steps
            .diff(session, outcome.result.as_ref())
            .await;
        let judging = Judging {
            session,
            judge: &judge,
            ended,
            shown: Failed {
                task: &self.task,
                diff: &diff,
                reply: (outcome.result.as_ref()).map_or("", |r| r.total_text.as_str()),
                failure: outcome.failure.as_deref().unwrap_or_default(),
            },
            auth_token: outcome.auth_token.clone(),
        };
        let (judged_as, judged) = judging.ask(self.run.output).await?;
        outcome.ended = Some(judged_as);
        let reasons = judged.verdict.reasons.clone();
        self.headless.judged.push(judged);
        Some(reasons)
    }

    /// The turn in which the same model carries on with the work of the
    /// turn that ended as `outcome`, told what `carry_on` says; the user
    /// is told of it in a notice.
    fn carry_on(&self, carry_on: CarryOn, outcome: &mut TurnOutcome) -> Option<Next> {
        let ended = outcome.ended.take()?;
        let notice = StreamEvent::Notice {
            text: &carry_on.notice,
        };
        self.run.output.event(&notice);
        let message = with_unseen(carry_on.message, &outcome.unseen);
        Some((message, Some(Carry::Nudge(ended, outcome.terms))))
    }

    /// The turn in which the next stronger model takes over the work of
    /// the turn that ended badly as `outcome`, told how it ended (`raw`,
    /// its signal before the judge's verdict) and what was `found` wrong
    /// with it. `None` when it ended well enough or nothing is stronger.
    fn take_over(
        &self,
        outcome: &TurnOutcome,
        raw: Option<OutcomeSignal>,
        found: Option<String>,
    ) -> Option<Next> {
        let (decision, ended) = self.stronger(outcome.ended.as_ref())?;
        let signal = raw.unwrap_or(ended.signal);
        let takeover = Takeover::new(&ended.model, decision, signal, found.as_deref());
        let text = with_unseen(takeover.text, &outcome.unseen);
        self.run.output.event(&StreamEvent::Notice { text: &text });
        let carry = Carry::Escalate(takeover.next, ended.category, outcome.terms);
        Some((from_harness(&text), Some(carry)))
    }

    /// Where a turn that Gate routed and that `ended` badly goes on: the
    /// next stronger model, with how the turn ended. `None` when it ended
    /// well enough or nothing is stronger.
    fn stronger<'o>(
        &self,
        ended: Option<&'o OutcomePayload>,
    ) -> Option<(Decision, &'o OutcomePayload)> {
        let ended = ended.filter(|ended| calls_for_stronger(ended.signal))?;
        let candidates = self.candidates.as_deref()?;
        let ran = (ended.model.as_str(), self.headless.effort);
        let gate = &self.headless.hub.gate;
        let decision = followup::stronger(gate, candidates, ended.category, ran, ended.signal)?;
        info!(
            from = %ended.model,
            to = %decision.model.id,
            signal = %ended.signal,
            "Gate moves the turn to a stronger model"
        );
        Some((decision, ended))
    }
}

/// `text`, then each of the notices a turn's model never got, a blank
/// line apart.
fn with_unseen(mut text: String, unseen: &[String]) -> String {
    for notice in unseen {
        text.push_str("\n\n");
        text.push_str(notice);
    }
    text
}
