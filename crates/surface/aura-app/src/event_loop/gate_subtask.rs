//! Gate's part in a `task` spawn: the subtask's prompt is classified and,
//! when the user selected gate and nothing pins the subtask's model, Gate
//! picks the model from its subtask rows. The child records the prompt at
//! level `subtask` with its category, route and kind, and the `Outcome`
//! written when the subtask ends.
//!
//! The model stays pinned (still classified and recorded) for a permanent
//! agent that names its own model, a council member, a model given in the
//! `task` call, and every subtask when the user pinned the main model.
//!
//! A subtask dispatched by an agent working on a frontier model is first
//! routed below that tier ([`aura_agent_gate::below_lead`]), so the most
//! expensive model plans and a cheaper one does the work.
//!
//! A subtask Gate routed whose attempt failed is retried once on the next
//! stronger model, the lead's tier included; the retry is recorded with
//! route reason `escalate`, which Gate's table does not count. Gate's
//! judge reviews the failed attempt first (see
//! [`aura_agent_gate::turn_judge`]): a failure it does not put down to
//! the model is not retried and counts for nothing.

use super::agents::AgentSession;
use super::gate_live::GateLive;
use super::gate_record;
use super::turns::record_prompt;
use crate::model_selection::{ModelLimits, ModelSelection};
use aura_agent::AgentLoopConfig;
use aura_agent_gate::{Candidate, Decision};
use aura_core_types::{
    Hash, PromptCategory, PromptPayload, Route, RouteLevel, RouteReason, SubagentDispatchRequest,
};
use aura_model::model_catalog::GATE_MODEL_ID;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// What Gate does with the subtasks turns delegate, as the selected
/// model stands.
#[derive(Debug, Clone, Default)]
pub struct GateBase {
    /// Model that classifies subtask prompts; empty leaves them
    /// unclassified.
    pub classifier: String,
    /// What Gate routes subtasks among when the user selected gate;
    /// `None` when they pinned the main model.
    pub candidates: Option<Vec<Candidate>>,
}

impl GateBase {
    /// Gate's part in subtasks under `models`.
    #[must_use]
    pub fn of(models: &ModelSelection) -> Self {
        Self {
            classifier: models.classifier_model(),
            candidates: models.is_gate().then(|| models.gate_candidates()),
        }
    }
}

/// The model a `task` call names, unless it names Gate itself.
pub(super) fn explicit_model(request: &SubagentDispatchRequest) -> Option<&str> {
    request
        .model_override
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != GATE_MODEL_ID)
}

/// How one subtask is classified and routed.
pub(super) struct SubtaskGate {
    /// The permanent agent the `task` call named
    pub kind: String,
    pub classifier: String,
    /// What Gate picks the subtask's model among; `None` when it is pinned
    pub candidates: Option<Vec<Candidate>>,
    /// The model Gate sent the dispatching agent's work to, if it did
    pub lead_model: Option<String>,
    pub limits: ModelLimits,
    pub live: Arc<GateLive>,
    /// Whether Gate may explore; off under the Gate arm of `/compare`
    pub explore: bool,
    /// Set for the retry of a failed attempt: where it goes, and the
    /// first attempt's category
    pub escalation: Option<(Decision, PromptCategory)>,
}

impl SubtaskGate {
    /// The label of the subtask asking `text` of an agent set up with
    /// `config`. When Gate routes it, the prompt is classified now and
    /// `config` is pointed at Gate's pick; a pinned subtask is classified
    /// beside its turn.
    pub(super) async fn label(
        &self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        text: &str,
        cancel: &CancellationToken,
    ) -> SubtaskLabel {
        let mut label = SubtaskLabel {
            text: text.to_string(),
            kind: self.kind.clone(),
            classifier: self.classifier.clone(),
            auth_token: config.auth_token.clone(),
            route: Route::subtask(config.model.clone(), RouteReason::Pinned),
            category: None,
            shown: None,
        };
        if let Some((decision, category)) = &self.escalation {
            info!(kind = %self.kind, model = %decision.model.id, %category, "Gate retries the subtask on a stronger model");
            if config.model != decision.model.id {
                self.limits.configure(config, &decision.model.id);
            }
            self.live.note_model(session.agent_id(), &decision.model.id);
            label.category = Some(*category);
            label.shown = Some(route_line(decision, *category));
            label.route = decision.route(RouteLevel::Subtask);
            return label;
        }
        let Some(candidates) = &self.candidates else {
            return label;
        };
        let category = label.classify(session, cancel).await;
        label.category = Some(category);
        let among = aura_agent_gate::below_lead(candidates, self.lead_model.as_deref());
        let kind = Some(self.kind.as_str());
        let decision = if self.explore {
            self.live
                .decide(RouteLevel::Subtask, category, kind, &among)
        } else {
            self.live
                .redecide(RouteLevel::Subtask, category, kind, &among)
        };
        let Some(decision) = decision else {
            label.route.reason = RouteReason::Default;
            return label;
        };
        info!(
            kind = %self.kind,
            model = %decision.model.id,
            %category,
            reason = %decision.reason,
            "Gate routed the subtask"
        );
        if config.model != decision.model.id {
            self.limits.configure(config, &decision.model.id);
        }
        self.live.note_model(session.agent_id(), &decision.model.id);
        label.shown = Some(route_line(&decision, category));
        label.route = decision.route(RouteLevel::Subtask);
        label
    }
}

/// The chat line saying where Gate sent a subtask asking for `category`.
fn route_line(decision: &Decision, category: PromptCategory) -> String {
    format!(
        "gate → {} · subtask, {category}, {}",
        decision.model.route_label(),
        decision.reason
    )
}

/// What a subtask's prompt is recorded with.
pub(super) struct SubtaskLabel {
    text: String,
    kind: String,
    classifier: String,
    auth_token: Option<String>,
    route: Route,
    /// Known before the turn when Gate routed the subtask
    pub category: Option<PromptCategory>,
    /// Where Gate sent the subtask, for the child's chat
    pub shown: Option<String>,
}

impl SubtaskLabel {
    /// The prompt's category, as the classifier says; unclassified when
    /// nothing classifies prompts or the subtask is cancelled first.
    async fn classify(&self, session: &AgentSession, cancel: &CancellationToken) -> PromptCategory {
        if self.classifier.is_empty() {
            return PromptCategory::Unclassified;
        }
        tokio::select! {
            category = gate_record::classify_prompt(
                &session.kernel,
                &self.classifier,
                &self.text,
                self.auth_token.clone(),
            ) => category,
            () = cancel.cancelled() => PromptCategory::Unclassified,
        }
    }

    /// Record the prompt on `session` as asking for `category`, returning
    /// its transaction hash and category.
    pub(super) async fn record(
        &self,
        session: &AgentSession,
        category: PromptCategory,
    ) -> Result<(Hash, PromptCategory), String> {
        let payload = PromptPayload {
            text: self.text.clone(),
            category,
            classifier: self.classifier.clone(),
            route: self.route.clone(),
            kind: Some(self.kind.clone()),
            led: None,
        };
        let (tx, _) = record_prompt(session, &payload).await?;
        Ok((tx.hash, category))
    }

    /// Classify the prompt and record it on `session`.
    pub(super) async fn classify_and_record(
        &self,
        session: &AgentSession,
        cancel: &CancellationToken,
    ) -> Result<(Hash, PromptCategory), String> {
        let category = self.classify(session, cancel).await;
        self.record(session, category).await
    }
}
