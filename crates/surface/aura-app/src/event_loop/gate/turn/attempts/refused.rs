//! The move off a route that refused the user's sign-in: the turn goes on
//! from where it stopped on Gate's next choice, and the refused model is
//! skipped until the user's sign-ins change.

use super::{AttemptRun, Attempting};
use crate::event_loop::gate::followup::refused_notice;
use crate::event_loop::gate::turn::route::route_line;
use crate::event_loop::handlers::system_message;
use tracing::info;

impl Attempting<'_, '_> {
    /// Go on with the turn on Gate's next choice, keeping the
    /// conversation the run that ended as `ran` left, its model's route
    /// having refused the user's sign-in. Returns whether it goes on: not
    /// when no candidate is left.
    pub(super) async fn reroute(&mut self, (result, forwarded): &AttemptRun) -> bool {
        let model = self.turn.agent_loop.config().model.clone();
        self.attempts.refused.push(model.clone());
        let (category, difficulty) = (self.asked.category, self.asked.difficulty);
        let next = (self.gate).reroute(category, difficulty, &self.attempts.refused);
        let Some(next) = next else {
            return false;
        };
        self.attempts.cost_usd += forwarded.usage.cost_usd;
        self.session.usage.add(&forwarded.usage);
        if let Ok(partial) = result {
            self.session.messages.clone_from(&partial.messages);
        }
        info!(from = %model, to = %next.model.id, "Gate re-routed the turn");
        let notice = refused_notice(&model, &route_line(&next, category));
        self.turn.ui.send(system_message(notice)).await;
        let agent = self.session.agent_id();
        self.gate.point(agent, &mut self.turn.agent_loop, &next);
        self.gate.show(self.turn.ui, &next).await;
        true
    }
}
