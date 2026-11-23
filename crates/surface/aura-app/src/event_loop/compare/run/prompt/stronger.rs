//! A Gate run's attempt that failed: its outcome is logged and recorded,
//! Gate's judge says whether its model was at fault, and unless the judge
//! excuses it the next stronger model takes the turn over in the same
//! conversation, under a prompt of its own recorded with route reason
//! `escalate`.

use super::attempts::{Attempting, Ended};
use crate::event_loop::gate::followup::{joined, record_message, stronger, Judging, Takeover};
use crate::event_loop::handlers::system_message;
use crate::event_loop::processes::now_ms;
use crate::model_selection::routed_effort;
use aura_core_types::{OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate::judge::turn::Failed;
use aura_gate::live;
use aura_model::{Message, ThinkingEffort};
use std::ops::ControlFlow;
use std::time::Instant;
use tracing::{info, warn};

/// An attempt that ended badly enough to call for a stronger model.
pub(super) struct Failure {
    /// What its prompt asks for, which the stronger model is chosen for
    pub category: PromptCategory,
    pub signal: OutcomeSignal,
    /// Its outcome; `None` when the prompt it answers could not be recorded
    pub outcome: Option<OutcomePayload>,
    pub reply: String,
    pub error: Option<String>,
    /// What its judge is shown of its change, and of how it failed
    pub diff: String,
    pub reason: String,
    /// What the harness's check printed when it failed
    pub harness: Option<String>,
    /// What the judge or the reviewer found wrong with work no check
    /// vouched for
    pub unchecked: Option<String>,
}

impl Attempting<'_, '_> {
    /// Hand the turn `failure` ended on to the next stronger model, unless
    /// Gate's judge excuses its model or nothing is stronger. Returns how
    /// the turn ended when it does not go on.
    pub(super) async fn escalate(&mut self, failure: Failure) -> ControlFlow<Ended> {
        let judged = self.judged(&failure).await;
        let Failure {
            category,
            signal,
            outcome,
            reply,
            error,
            harness,
            unchecked,
            ..
        } = failure;
        let review = match judged {
            ControlFlow::Continue(review) => review.or(unchecked),
            ControlFlow::Break(excused) => {
                return ControlFlow::Break(Ended {
                    signal: excused.signal,
                    reply,
                    error,
                    outcome: Some(excused),
                    recorded: true,
                });
            }
        };
        let ran_at = self.config.held_thinking_effort();
        let ran = (self.current.model.as_str(), routed_effort(&self.config));
        let (gate, candidates) = (&self.parts.ctx.hub.gate, &self.parts.ctx.plan.candidates);
        let Some(next) = stronger(gate, candidates, category, ran, signal) else {
            return ControlFlow::Break(Ended {
                signal,
                reply,
                error,
                outcome,
                recorded: true,
            });
        };
        let found = joined(harness, review);
        let takeover = Takeover::new(&self.current.model, next, signal, found.as_deref());
        self.take_over(takeover, category, signal, ran_at).await;
        ControlFlow::Continue(())
    }

    /// Log and record the outcome of the attempt `failure` ended, and have
    /// Gate's judge say whether its model was at fault, which is recorded
    /// as a later outcome of its prompt. Breaks with that outcome when the
    /// judge excuses the model; else goes on with what the judge found
    /// wrong, when it was asked and gave a verdict.
    async fn judged(&mut self, failure: &Failure) -> ControlFlow<OutcomePayload, Option<String>> {
        let Some(failed) = &failure.outcome else {
            return ControlFlow::Continue(None);
        };
        self.settle(failed).await;
        let (hub, plan) = (&self.parts.ctx.hub, &self.parts.ctx.plan);
        let Some(judge) = live::judge_for(&hub.gate, &plan.candidates, failed) else {
            return ControlFlow::Continue(None);
        };
        let shown = Failed {
            task: self.asked.text,
            diff: &failure.diff,
            reply: &failure.reply,
            failure: &failure.reason,
        };
        let judging = Judging {
            session: self.session,
            judge: &judge,
            ended: failed,
            shown,
            auth_token: self.config.auth_token.clone(),
        };
        match judging.ask(&self.parts.ui).await {
            Some((excused, judged)) if !judged.verdict.at_fault => ControlFlow::Break(excused),
            Some((_, judged)) => ControlFlow::Continue(Some(judged.verdict.reasons)),
            None => ControlFlow::Continue(None),
        }
    }

    /// Log `failed`, the outcome of the attempt that ended, to Gate's
    /// attempt log with what the attempt left, and record it.
    async fn settle(&mut self, failed: &OutcomePayload) {
        let agent_id = self.session.agent_id();
        let terms = self.terms();
        (self.parts.ctx.hub.gate).log_attempt(agent_id, failed, terms, self.left.take());
        live::record(&self.session.kernel, agent_id, failed).await;
    }

    /// Hand the turn on a prompt of `category` to the stronger model
    /// `takeover` names, in the same conversation, after an attempt that
    /// ended with `signal` and whose requests were held at the effort
    /// `ran_at`.
    async fn take_over(
        &mut self,
        takeover: Takeover,
        category: PromptCategory,
        signal: OutcomeSignal,
        ran_at: Option<ThinkingEffort>,
    ) {
        let (parts, agent_id) = (self.parts, self.session.agent_id());
        let prompt = takeover.prompt(category, parts.ctx.plan.classifier.clone());
        let recorder = parts.recorder.as_deref();
        self.current.prompt_tx = match record_message(self.session, &prompt, recorder).await {
            Ok(hash) => Some(hash),
            Err(problem) => {
                warn!(%agent_id, %problem, "Failed to record a compare run's escalated prompt");
                None
            }
        };
        let next = takeover.next;
        info!(%agent_id, from = %self.current.model, to = %next.model.id, %signal, "Gate moves the run's turn to a stronger model");
        let line = parts.gate_line(&next, category);
        parts.ui.send(system_message(line)).await;
        self.point(&next);
        self.escalations += 1;
        self.current.cost_usd = 0.0;
        (self.current.started, self.current.started_ms) = (Instant::now(), now_ms());
        self.current.turn_start = self.session.messages.len();
        self.config.initial_todos.clone_from(&self.session.todos);
        self.config.initial_thinking_effort = ran_at;
        self.session.messages.push(Message::user(prompt.text));
    }
}
