//! Gate's choice for a turn once its prompt is classified: the model it
//! runs on, at what effort, and whether it is led; and how the choice
//! shows in the chat and the status line.

use super::label::Label;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::gate::followup;
use crate::event_loop::handlers::system_message;
use crate::event_loop::subagents::{AgentHub, LeadProfile};
use crate::event_loop::turns::TurnParts;
use crate::model_selection::ModelLimits;
use aura_agent::AgentLoop;
use aura_core_types::{AgentId, Difficulty, PromptCategory, RouteLevel};
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::{self, GateLive};
use aura_gate::route::{Candidate, Decision, Work};
use aura_ui::UiCommand;
use std::sync::Arc;
use tracing::info;

/// What Gate routes a turn among once its prompt is classified, and how
/// to point an agent loop at its choice.
pub(crate) struct GateTurn {
    /// Models the turn may go to
    pub candidates: Vec<Candidate>,
    pub limits: ModelLimits,
    /// What Gate has learned
    pub live: Arc<GateLive>,
    /// The model selector and status line as they stand, for showing the
    /// turn's route
    pub models: UiCommand,
    /// How the turn leads when Gate has it led; `None` when the agent
    /// never leads
    pub lead: Option<LeadProfile>,
    /// For what the turn's subtasks cost
    pub hub: Arc<AgentHub>,
}

impl GateTurn {
    /// What the work of `label`'s prompt, asking for `category`, is
    /// checked against (see [`live::hold_to_criteria`]), which may rate
    /// it harder. A writer that failed is shown on `ui`, and the work
    /// goes on without.
    pub(super) async fn criteria(
        &self,
        session: &AgentSession,
        label: &mut Label,
        category: PromptCategory,
        ui: &AgentUi,
    ) -> Option<Arc<Criteria>> {
        let prompt = &mut label.prompt;
        let held = live::hold_to_criteria(
            &session.kernel,
            (&self.live, &self.candidates),
            category,
            &mut prompt.difficulty,
            &prompt.text,
            label.auth_token.clone(),
        )
        .await;
        match held {
            Ok(criteria) => criteria,
            Err(problem) => {
                ui.send(system_message(problem)).await;
                None
            }
        }
    }

    /// Have Gate pick the model of `session`'s turn on `turn`, asking for
    /// `category`, point the turn at it, show the choice and make it the
    /// route `label` records; then have Gate decide whether the turn is
    /// led. Nothing changes when Gate has no model to pick.
    pub(super) async fn route(
        &self,
        session: &mut AgentSession,
        turn: &mut TurnParts<'_>,
        label: &mut Label,
        category: PromptCategory,
    ) {
        let prompt = &mut label.prompt;
        let work = Work::turn(category).of(prompt.difficulty);
        let Some(decision) = self.live.decide(&work, &self.candidates) else {
            return;
        };
        info!(
            model = %decision.model.id,
            %category,
            reason = %decision.reason,
            "Gate routed the turn"
        );
        self.point(session.agent_id(), &mut turn.agent_loop, &decision);
        turn.ui
            .send(system_message(route_line(&decision, category)))
            .await;
        self.show(turn.ui, &decision).await;
        prompt.route = decision.route(RouteLevel::Turn);
        let (led, enforced) = self.lead(category, prompt.difficulty, turn);
        prompt.led = led;
        session.lead = enforced;
    }

    /// Gate's choice for a turn asking for `category`, of `difficulty`,
    /// once the routes of the models in `refused` turned the user's
    /// sign-in down (see [`followup::reroute`]).
    pub(super) fn reroute(
        &self,
        category: PromptCategory,
        difficulty: Option<Difficulty>,
        refused: &[String],
    ) -> Option<Decision> {
        let work = Work::turn(category).of(difficulty);
        followup::reroute(&self.live, &work, &self.candidates, refused)
    }

    /// Point `agent_loop`, running `agent`'s turn, at `decision`'s model
    /// and effort.
    pub(super) fn point(&self, agent: AgentId, agent_loop: &mut AgentLoop, decision: &Decision) {
        self.live.note_model(agent, &decision.model.id);
        let current = agent_loop.config();
        if current.model == decision.model.id
            && current.routed_thinking_effort == Some(decision.effort)
        {
            return;
        }
        let mut config = current.clone();
        self.limits.route(&mut config, decision);
        *agent_loop = AgentLoop::new(config);
    }

    /// Have Gate decide whether the turn on `turn`, asking for `category`
    /// of `difficulty`, is led ([`GateLive::way`]), and make it a lead's
    /// if so (see [`LeadProfile::apply`]). Returns whether it is led
    /// (`None` when the agent never leads) and whether its calls must stay
    /// within the lead's limits.
    fn lead(
        &self,
        category: PromptCategory,
        difficulty: Option<Difficulty>,
        turn: &mut TurnParts<'_>,
    ) -> (Option<bool>, bool) {
        let Some(lead) = self.lead else {
            return (None, false);
        };
        let mut config = turn.agent_loop.config().clone();
        let way = self.live.way((category, difficulty), &config.model, true);
        if way.explored {
            info!(led = way.led, %category, "Gate explores the other way of working");
        }
        let before = config.system_prompt.len();
        let enforced = lead.apply(way.led, &mut config, &mut turn.tools);
        if config.system_prompt.len() != before {
            *turn.agent_loop = AgentLoop::new(config);
        }
        (Some(way.led), enforced)
    }

    /// Show the model and route of `decision` in the status line.
    pub(super) async fn show(&self, ui: &AgentUi, decision: &Decision) {
        let mut models = self.models.clone();
        if let UiCommand::SetModels { route, .. } = &mut models {
            *route = Some(decision.model.route_label());
        }
        ui.send_app(models).await;
    }
}

/// The chat line saying where Gate sent a turn asking for `category`, e.g.
/// `gate → claude-sonnet-4-6 (claude plan) at medium effort · question,
/// default`.
pub(super) fn route_line(decision: &Decision, category: PromptCategory) -> String {
    format!(
        "gate → {} · {category}, {}",
        decision.label(),
        decision.reason
    )
}
