//! Where a subtask goes: on to the stronger model a failed first attempt
//! was sent to, to the model the by-difficulty arm of `/compare` gives
//! its difficulty, or to Gate's pick; or nowhere new, its model pinned.

use super::{SubtaskGate, SubtaskLabel};
use crate::event_loop::agents::AgentSession;
use aura_agent::AgentLoopConfig;
use aura_core_types::{PromptCategory, Route, RouteLevel, RouteReason};
use aura_gate::compare::DifficultyRoutes;
use aura_gate::route::{Candidate, Decision, Work};
use tokio_util::sync::CancellationToken;
use tracing::info;

impl SubtaskGate {
    /// The label of the subtask asking `text` of an agent set up with
    /// `config`. When Gate routes it, the prompt is classified now and
    /// `config` is pointed at Gate's pick; a pinned subtask is not
    /// classified.
    ///
    /// # Errors
    /// Says why when Gate routes the subtask and could not classify it.
    pub(crate) async fn label(
        &self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        text: &str,
        cancel: &CancellationToken,
    ) -> Result<SubtaskLabel, String> {
        let mut label = SubtaskLabel {
            text: text.to_string(),
            kind: self.kind.clone(),
            classifier: self.classifier.clone(),
            auth_token: config.auth_token.clone(),
            route: Route::subtask(config.model.clone(), RouteReason::Pinned),
            category: None,
            difficulty: None,
            shown: None,
        };
        if let Some((decision, category)) = &self.escalation {
            info!(kind = %self.kind, model = %decision.model.id, effort = ?decision.effort, %category, "Gate retries the subtask on a stronger model");
            label.category = Some(*category);
            let shown = route_line(decision, *category);
            self.send(session, config, &mut label, decision, shown);
        } else if let Some(table) = &self.table {
            self.by_difficulty(session, config, &mut label, table, cancel)
                .await?;
        } else if let Some(candidates) = &self.candidates {
            self.by_gate(session, config, &mut label, candidates, cancel)
                .await?;
        }
        Ok(label)
    }

    /// Classify `label`'s prompt and send the subtask to the model
    /// `table` gives its difficulty.
    async fn by_difficulty(
        &self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        label: &mut SubtaskLabel,
        table: &DifficultyRoutes,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let category = label.classify(session, cancel).await?;
        label.category = Some(category);
        let work =
            Work::new(RouteLevel::Subtask, category, Some(self.kind.as_str())).of(label.difficulty);
        let decision = table.decide(&work);
        info!(
            kind = %self.kind,
            model = %decision.model.id,
            %category,
            difficulty = ?label.difficulty,
            "The by-difficulty arm routed the subtask"
        );
        let shown = format!(
            "by difficulty → {} · subtask, {category}, {}",
            decision.label(),
            label
                .difficulty
                .map_or_else(|| "unrated".to_string(), |d| d.to_string())
        );
        self.send(session, config, label, &decision, shown);
        Ok(())
    }

    /// Classify `label`'s prompt and send the subtask to Gate's pick among
    /// `candidates`, below the lead's tier when the lead works on a
    /// frontier model. With no pick it keeps its model, routed by default.
    async fn by_gate(
        &self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        label: &mut SubtaskLabel,
        candidates: &[Candidate],
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let category = label.classify(session, cancel).await?;
        label.category = Some(category);
        let among = aura_gate::route::below_lead(candidates, self.lead_model.as_deref());
        let work =
            Work::new(RouteLevel::Subtask, category, Some(self.kind.as_str())).of(label.difficulty);
        let decision = if self.explore {
            self.live.decide(&work, &among)
        } else {
            self.live.redecide(&work, &among)
        };
        let Some(decision) = decision else {
            label.route.reason = RouteReason::Default;
            return Ok(());
        };
        info!(
            kind = %self.kind,
            model = %decision.model.id,
            %category,
            reason = %decision.reason,
            "Gate routed the subtask"
        );
        self.send(
            session,
            config,
            label,
            &decision,
            route_line(&decision, category),
        );
        Ok(())
    }

    /// Point `config` at `decision`'s model and note it as the model
    /// `session`'s work is on; `label` records the route, and the child's
    /// chat shows `shown`.
    fn send(
        &self,
        session: &AgentSession,
        config: &mut AgentLoopConfig,
        label: &mut SubtaskLabel,
        decision: &Decision,
        shown: String,
    ) {
        self.limits.route(config, decision);
        self.live.note_model(session.agent_id(), &decision.model.id);
        label.shown = Some(shown);
        label.route = decision.route(RouteLevel::Subtask);
    }
}

/// The chat line saying where Gate sent a subtask asking for `category`.
fn route_line(decision: &Decision, category: PromptCategory) -> String {
    format!(
        "gate → {} · subtask, {category}, {}",
        decision.label(),
        decision.reason
    )
}
