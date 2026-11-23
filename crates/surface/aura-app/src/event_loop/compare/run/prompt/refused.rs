//! The move off a route that refused the user's sign-in on a Gate run:
//! the turn goes on from where it stopped on Gate's next choice, and the
//! refused model is left out of the rest of the run.

use super::attempts::{AttemptRun, Attempting};
use crate::event_loop::gate::followup::{self, refused_notice};
use crate::event_loop::handlers::system_message;
use aura_gate::route::Work;
use tracing::info;

impl Attempting<'_, '_> {
    /// Go on with a Gate run's turn on Gate's next choice when the route of
    /// the attempt that ended as `ran` refused the user's sign-in, keeping
    /// the conversation the attempt left. Returns whether it goes on: not
    /// on other arms, when the user stopped the run, or when no candidate
    /// is left.
    pub(super) async fn reroute(&mut self, (result, forwarded): &AttemptRun) -> bool {
        let parts = self.parts;
        if !forwarded.credentials_rejected || !parts.gate_arm() || parts.cancel.is_cancelled() {
            return false;
        }
        let model = self.current.model.clone();
        self.asked.refused.push(model.clone());
        let (category, difficulty) = (self.asked.category, self.asked.difficulty);
        let work = Work::turn(category).of(difficulty);
        let candidates = &parts.ctx.plan.candidates;
        let next = followup::reroute(&parts.ctx.hub.gate, &work, candidates, self.asked.refused);
        let Some(next) = next else {
            return false;
        };
        self.current.cost_usd += forwarded.usage.cost_usd;
        self.session.usage.add(&forwarded.usage);
        if let Ok(partial) = result {
            self.session.messages.clone_from(&partial.messages);
        }
        let agent_id = self.session.agent_id();
        info!(%agent_id, from = %model, to = %next.model.id, "Gate re-routed the run's turn");
        let notice = refused_notice(&model, &parts.gate_line(&next, category));
        parts.ui.send(system_message(notice)).await;
        self.point(&next);
        true
    }
}
