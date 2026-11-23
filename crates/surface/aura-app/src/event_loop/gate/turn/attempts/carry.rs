//! Gate's follow-up of an attempt that ended, and the same model carrying
//! on in the same conversation when a step hands the work back to it.

use super::Attempting;
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::gate::followup::{record_message, At, Attempt, CarryOn, Followed};
use crate::event_loop::handlers::{record_response, system_message};
use crate::model_selection::routed_effort;
use aura_agent::{AgentLoop, AgentLoopResult};
use aura_core_types::{OutcomeSignal, PromptPayload};
use aura_model::Message;
use tracing::{info, warn};

impl Attempting<'_, '_> {
    /// Gate's follow-up of the attempt on `model` that ended with
    /// `partial` and, so far, `signal` (see
    /// [`Following::run`](crate::event_loop::gate::followup::Following::run)).
    pub(super) async fn follow_up(
        &mut self,
        partial: &AgentLoopResult,
        model: &str,
        signal: Option<OutcomeSignal>,
    ) -> Followed {
        let workspace = self.session.kernel.agent_workspace();
        let config = self.turn.agent_loop.config();
        let attempt = Attempt {
            session: self.session,
            live: &self.gate.live,
            candidates: &self.gate.candidates,
            task: self.asked.task,
            ran: (model, routed_effort(config)),
            result: partial,
            at: At {
                category: self.asked.category,
                difficulty: self.asked.difficulty,
                signal,
                turn_start: self.current.turn_start,
            },
            prompt_tx: Some(self.current.prompt_tx),
            workspace: &workspace,
            before: self.before.as_ref(),
            criteria: self.asked.criteria.as_ref(),
            auth_token: config.auth_token.clone(),
        };
        self.follow.run(&attempt, self.turn.ui).await
    }

    /// Have the same model carry on in the same conversation from
    /// `partial`, told what `carry_on` says, the run that left it costing
    /// what `forwarded` counted.
    pub(super) async fn carry_on(
        &mut self,
        partial: &AgentLoopResult,
        forwarded: &ForwarderState,
        carry_on: CarryOn,
    ) {
        let CarryOn {
            message, notice, ..
        } = carry_on;
        self.commit(partial).await;
        let prompt = PromptPayload::new(message);
        let recorder = self.turn.recorder.as_deref();
        if let Err(problem) = record_message(self.session, &prompt, recorder).await {
            warn!(%problem, "Could not record Gate's message to the model");
        }
        self.session.usage.add(&forwarded.usage);
        self.attempts.cost_usd += forwarded.usage.cost_usd;
        self.session.messages.push(Message::user(prompt.text));
        let model = &self.turn.agent_loop.config().model;
        info!(%model, %notice, "Gate has the turn carry on");
        self.turn.ui.send(system_message(notice)).await;
        self.renew_loop();
    }

    /// Keep what the attempt that ended in `partial` left: its
    /// conversation, its todo list and its reply.
    pub(super) async fn commit(&mut self, partial: &AgentLoopResult) {
        self.session.messages.clone_from(&partial.messages);
        self.session.todos.clone_from(&partial.todos);
        record_response(self.session, &partial.total_text).await;
    }

    /// A fresh agent loop for the next run, starting from the session's
    /// todo list at the effort the last run's requests were held at.
    pub(super) fn renew_loop(&mut self) {
        let mut config = self.turn.agent_loop.config().clone();
        config.initial_todos.clone_from(&self.session.todos);
        config.initial_thinking_effort = config.held_thinking_effort();
        *self.turn.agent_loop = AgentLoop::new(config);
    }
}
