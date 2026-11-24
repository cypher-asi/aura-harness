//! Gate's choice of model for a turn: the prompt is classified, the run's
//! own held to acceptance criteria, and the turn pointed at Gate's pick;
//! or at the stronger model a failed turn's work goes on to.

use super::drive::Driving;
use super::Headless;
use crate::event_loop::gate::followup::takeover_prompt;
use crate::model_selection::routed_effort;
use aura_agent::AgentLoopConfig;
use aura_core_types::{AgentId, Difficulty, PromptCategory, PromptPayload};
use aura_core_types::{Route, RouteLevel, RouteReason};
use aura_gate::live;
use aura_gate::route::{Candidate, Decision, Work};
use tracing::info;

/// The prompt a turn records, what its work asks for when Gate routed
/// it, and what its model is told after the prompt.
pub(super) type Prompted = (PromptPayload, Option<PromptCategory>, Option<String>);

impl Driving<'_> {
    /// The prompt record of a turn answering `text`, and the category of
    /// its work when Gate routed it. Gate routes the turn when the user
    /// selected gate, the agent names no model of its own and there is a
    /// model to route to: the prompt is classified and `config` pointed
    /// at Gate's pick, which the subtasks the turn dispatches are routed
    /// by. Otherwise the prompt is recorded as it is. With `escalation`,
    /// the turn goes to that model with that category, recorded with
    /// route reason `escalate`.
    ///
    /// # Errors
    /// Says why when Gate routes the turn and could not classify it.
    pub(super) async fn route_turn(
        &mut self,
        config: &mut AgentLoopConfig,
        text: String,
        escalation: Option<(Decision, PromptCategory)>,
    ) -> Result<Prompted, String> {
        let agent = self.session.agent_id();
        if let Some((decision, category)) = escalation {
            self.headless.point(agent, config, &decision);
            let payload = takeover_prompt(text, category, &decision, String::new());
            return Ok((payload, Some(category), None));
        }
        let candidates = self.candidates.as_deref().filter(|c| !c.is_empty());
        let Some(candidates) = candidates else {
            return Ok((PromptPayload::new(text), None, None));
        };
        let classifier = self.headless.hub.classifier();
        if classifier.is_empty() {
            return Err("Gate has no model to classify the prompt".to_string());
        }
        let kernel = &self.session.kernel;
        let token = config.auth_token.clone();
        let label = live::classify_prompt(kernel, &classifier, &text, token).await?;
        let category = label.category;
        let mut difficulty = Some(label.difficulty);
        let assessment = label.assessment;
        let mut told = None;
        if std::mem::take(&mut self.headless.criteria_due) {
            let token = config.auth_token.clone();
            let held = live::hold_to_criteria(
                kernel,
                (&self.headless.hub.gate, candidates),
                category,
                &mut difficulty,
                &text,
                token,
            );
            self.headless.criteria = held.await.ok().flatten();
            told = live::worker_note_for(self.headless.criteria.as_deref());
        }
        let route = self
            .headless
            .route(agent, config, candidates, (category, difficulty));
        let payload = PromptPayload {
            category: Some(category),
            classifier,
            route,
            difficulty,
            assessment,
            ..PromptPayload::new(text)
        };
        Ok((payload, Some(category), told))
    }
}

impl Headless {
    /// The route of a turn of `agent` whose work asks for `category` of
    /// `difficulty`: Gate's pick among `candidates`, which `config` is
    /// pointed at, else the model `config` already runs.
    fn route(
        &mut self,
        agent: AgentId,
        config: &mut AgentLoopConfig,
        candidates: &[Candidate],
        (category, difficulty): (PromptCategory, Option<Difficulty>),
    ) -> Route {
        let work = Work::turn(category).of(difficulty);
        let Some(decision) = self.hub.gate.decide(&work, candidates) else {
            return Route::turn(config.model.clone(), RouteReason::Default);
        };
        info!(
            model = %decision.model.id,
            %category,
            reason = %decision.reason,
            "Gate routed the turn"
        );
        self.point(agent, config, &decision);
        decision.route(RouteLevel::Turn)
    }

    /// Point `config`, of a turn of `agent`, at `decision`'s model and
    /// effort.
    fn point(&mut self, agent: AgentId, config: &mut AgentLoopConfig, decision: &Decision) {
        self.hub.model_limits().route(config, decision);
        self.model.clone_from(&decision.model.id);
        self.effort = routed_effort(config);
        self.hub.gate.note_model(agent, &decision.model.id);
    }
}
