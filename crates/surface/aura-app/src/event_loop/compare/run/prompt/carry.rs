//! Gate's follow-up of an attempt that ended on a Gate run, and the same
//! model carrying on in the same conversation when a step hands the work
//! back to it.

use super::attempts::Attempting;
use crate::event_loop::agent_events::ForwarderState;
use crate::event_loop::gate::followup::{record_message, At, Attempt, CarryOn, Followed};
use crate::event_loop::handlers::system_message;
use crate::model_selection::routed_effort;
use aura_agent::AgentLoopResult;
use aura_core_types::{OutcomeSignal, PromptPayload};
use aura_model::Message;
use tracing::warn;

impl Attempting<'_, '_> {
    /// Gate's follow-up of the attempt that ended with `partial` and, so
    /// far, `signal` (see
    /// [`Following::run`](crate::event_loop::gate::followup::Following::run)).
    pub(super) async fn follow_up(
        &mut self,
        partial: &AgentLoopResult,
        signal: Option<OutcomeSignal>,
    ) -> Followed {
        let (parts, asked) = (self.parts, &self.asked);
        let attempt = Attempt {
            session: self.session,
            live: &parts.ctx.hub.gate,
            candidates: &parts.ctx.plan.candidates,
            task: asked.text,
            ran: (self.current.model.as_str(), routed_effort(&self.config)),
            result: partial,
            at: At {
                category: asked.category,
                difficulty: asked.difficulty,
                signal,
                turn_start: self.current.turn_start,
            },
            prompt_tx: self.current.prompt_tx,
            workspace: &self.workdir,
            before: asked.before.as_ref(),
            criteria: asked.criteria.as_ref(),
            auth_token: self.config.auth_token.clone(),
        };
        self.follow.run(&attempt, &parts.ui).await
    }

    /// Have the same model carry on in the same conversation, told what
    /// `carry_on` says, the run that handed the work back costing what
    /// `forwarded` counted.
    pub(super) async fn carry_on(&mut self, carry_on: CarryOn, forwarded: &ForwarderState) {
        let CarryOn {
            message,
            notice,
            nudge,
        } = carry_on;
        let prompt = PromptPayload::new(message);
        let recorder = self.parts.recorder.as_deref();
        if let Err(problem) = record_message(self.session, &prompt, recorder).await {
            let agent_id = self.session.agent_id();
            warn!(%agent_id, %problem, "Failed to record Gate's message to the model");
        }
        self.parts.ui.send(system_message(notice)).await;
        self.current.cost_usd += forwarded.usage.cost_usd;
        if nudge {
            self.nudges += 1;
        } else {
            self.sent_back += 1;
        }
        self.config.initial_todos.clone_from(&self.session.todos);
        self.config.initial_thinking_effort = self.config.held_thinking_effort();
        self.session.messages.push(Message::user(prompt.text));
    }
}
