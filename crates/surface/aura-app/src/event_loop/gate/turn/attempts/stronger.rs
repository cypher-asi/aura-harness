//! An attempt that failed: its outcome is recorded, Gate's judge says
//! whether its model was at fault, and unless the judge excuses it the
//! next stronger model takes the turn over in the same conversation.

use super::{Attempting, Current};
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::gate::followup::{
    failure_text, joined, record_message, stronger, Judging, Takeover,
};
use crate::event_loop::gate::turn::elapsed_ms;
use crate::event_loop::gate::turn::route::route_line;
use crate::event_loop::handlers::system_message;
use crate::model_selection::routed_effort;
use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::{OutcomePayload, OutcomeSignal};
use aura_gate::judge::turn::Failed;
use aura_gate::live::{self, Judged};
use aura_gate::route::Decision;
use aura_model::Message;
use std::time::Instant;
use tracing::{info, warn};

/// An attempt that ended badly enough to call for a stronger model.
pub(super) struct Failure<'a> {
    pub result: &'a Result<AgentLoopResult, AgentError>,
    pub forwarded: &'a ForwarderState,
    /// The model it ran on
    pub model: &'a str,
    pub signal: OutcomeSignal,
    /// What the harness's check printed when it failed
    pub harness: Option<String>,
    /// What the judge or the reviewer found wrong with work no check
    /// vouched for
    pub unchecked: Option<String>,
}

impl Attempting<'_, '_> {
    /// Hand the turn `failure` ended on to a stronger model when one should
    /// take it over (see [`super::run`]). Returns whether the turn goes
    /// on.
    pub(super) async fn escalate(&mut self, mut failure: Failure<'_>) -> bool {
        let failed = self.outcome_of(&failure);
        let judge = live::judge_for(&self.gate.live, &self.gate.candidates, &failed);
        let mut next = None;
        if judge.is_none() {
            next = self.stronger_model(&failure);
            if next.is_none() {
                return false;
            }
        }
        self.settle(&failure, &failed).await;
        let mut review = None;
        if let Some(judge) = judge {
            match self.judge(&judge, &failed, &failure).await {
                Some(judged) if !judged.verdict.at_fault => return false,
                Some(judged) => review = Some(judged.verdict.reasons),
                None => {}
            }
            next = self.stronger_model(&failure);
        }
        let review = review.or(failure.unchecked.take());
        let (Some(next), Ok(partial)) = (next, failure.result) else {
            return false;
        };
        let found = joined(failure.harness.take(), review);
        let takeover = Takeover::new(failure.model, next, failure.signal, found.as_deref());
        self.take_over(partial, &failure, takeover).await
    }

    /// The outcome of the attempt `failure` ended.
    fn outcome_of(&self, failure: &Failure<'_>) -> OutcomePayload {
        let agent = self.session.agent_id();
        OutcomePayload {
            prompt_tx: self.current.prompt_tx,
            category: self.asked.category,
            model: failure.model.to_string(),
            success: failure.signal.success(),
            signal: failure.signal,
            cost_usd: self.attempts.cost_usd + failure.forwarded.usage.cost_usd,
            subtask_cost_usd: self.gate.hub.subtask_cost_usd(agent, self.asked.started_ms),
            latency_ms: elapsed_ms(self.current.started),
        }
    }

    /// The stronger model that takes the turn over after `failure`; only
    /// a turn whose conversation survived can be carried on.
    fn stronger_model(&self, failure: &Failure<'_>) -> Option<Decision> {
        let ran = (failure.model, routed_effort(self.turn.agent_loop.config()));
        let (live, candidates) = (&self.gate.live, &self.gate.candidates);
        let category = self.asked.category;
        (failure.result.is_ok())
            .then(|| stronger(live, candidates, category, ran, failure.signal))
            .flatten()
    }

    /// Log and record `failed`, the outcome of the attempt `failure`
    /// ended, as the turn's outcome so far.
    async fn settle(&mut self, failure: &Failure<'_>, failed: &OutcomePayload) {
        let turn_start = self.current.turn_start;
        let left = (failure.result.as_ref().ok())
            .map(|result| self.follow.steps.evidence(result, turn_start));
        let agent = self.session.agent_id();
        self.gate
            .live
            .log_attempt(agent, failed, self.asked.terms(), left);
        live::record(&self.session.kernel, agent, failed).await;
        self.attempts.settled = true;
    }

    /// Ask `judge` whether the model was at fault in the attempt `failure`
    /// ended, whose outcome is `failed`; returns its finding, shown in the
    /// chat.
    async fn judge(
        &mut self,
        judge: &str,
        failed: &OutcomePayload,
        failure: &Failure<'_>,
    ) -> Option<Judged> {
        let result = failure.result;
        let diff = self
            .follow
            .steps
            .diff(self.session, result.as_ref().ok())
            .await;
        let reason = (failure.harness.clone())
            .or_else(|| failure_text(result, self.current.turn_start))
            .unwrap_or_default();
        let judging = Judging {
            session: self.session,
            judge,
            ended: failed,
            shown: Failed {
                task: self.asked.task,
                diff: &diff,
                reply: result
                    .as_ref()
                    .map_or("", |result| result.total_text.as_str()),
                failure: &reason,
            },
            auth_token: self.turn.agent_loop.config().auth_token.clone(),
        };
        let (_, judged) = judging.ask(self.turn.ui).await?;
        Some(judged)
    }

    /// Hand the turn to the stronger model `takeover` names, carrying on
    /// in the same conversation from `partial`, what `failure` left.
    /// Returns whether it goes on: not when its prompt could not be
    /// recorded.
    async fn take_over(
        &mut self,
        partial: &AgentLoopResult,
        failure: &Failure<'_>,
        takeover: Takeover,
    ) -> bool {
        let category = self.asked.category;
        let prompt = takeover.prompt(category, String::new());
        self.commit(partial).await;
        let recorder = self.turn.recorder.as_deref();
        let prompt_tx = match record_message(self.session, &prompt, recorder).await {
            Ok(prompt_tx) => prompt_tx,
            Err(problem) => {
                warn!(%problem, "Could not record the stronger model's prompt");
                return false;
            }
        };
        self.session.usage.add(&failure.forwarded.usage);
        self.attempts.cost_usd = 0.0;
        self.attempts.settled = false;
        self.attempts.signal = None;
        let turn_start = self.session.messages.len();
        self.current = Current {
            prompt_tx,
            turn_start,
            started: Instant::now(),
        };
        self.session.messages.push(Message::user(prompt.text));
        let next = takeover.next;
        info!(
            from = %failure.model,
            to = %next.model.id,
            signal = %failure.signal,
            "Gate moves the turn to a stronger model"
        );
        self.turn
            .ui
            .send(system_message(route_line(&next, category)))
            .await;
        self.renew_loop();
        let agent = self.session.agent_id();
        self.gate.point(agent, &mut self.turn.agent_loop, &next);
        self.gate.show(self.turn.ui, &next).await;
        true
    }
}
