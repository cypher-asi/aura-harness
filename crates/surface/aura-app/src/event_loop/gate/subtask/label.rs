//! What a subtask's prompt is recorded with: its route and kind, and,
//! when Gate routed it, the category and difficulty the classifier gave.

use crate::event_loop::agents::AgentSession;
use crate::event_loop::turns::record_prompt;
use aura_core_types::{Assessment, Difficulty, Hash, PromptCategory, PromptPayload, Route};
use aura_gate::live;
use tokio_util::sync::CancellationToken;

/// What a subtask's prompt is recorded with.
pub(crate) struct SubtaskLabel {
    pub(super) text: String,
    pub(super) kind: String,
    pub(super) classifier: String,
    pub(super) auth_token: Option<String>,
    pub(super) route: Route,
    /// Set when Gate routed the subtask; `None` when it is pinned
    pub category: Option<PromptCategory>,
    /// How hard the classifier judged the work, once it said
    pub(super) difficulty: Option<Difficulty>,
    /// What the classifier found about the work, once it said
    pub(super) assessment: Option<Assessment>,
    /// Where Gate sent the subtask, for the child's chat
    pub shown: Option<String>,
}

impl SubtaskLabel {
    /// The prompt's category, as the classifier says, its difficulty and
    /// what the classifier found about it kept on `self`.
    ///
    /// # Errors
    /// Says why when nothing classifies prompts, the subtask is cancelled
    /// first, or the classifier gave no category and difficulty.
    pub(super) async fn classify(
        &mut self,
        session: &AgentSession,
        cancel: &CancellationToken,
    ) -> Result<PromptCategory, String> {
        if self.classifier.is_empty() {
            return Err("Gate has no model to classify the subtask's prompt".to_string());
        }
        let label = tokio::select! {
            label = live::classify_prompt(
                &session.kernel,
                &self.classifier,
                &self.text,
                self.auth_token.clone(),
            ) => label?,
            () = cancel.cancelled() => return Err("cancelled by the user".to_string()),
        };
        self.difficulty = Some(label.difficulty);
        self.assessment = label.assessment;
        Ok(label.category)
    }

    /// Record the prompt on `session`, returning its transaction hash and
    /// category; a pinned subtask's is recorded with no category, and
    /// `None` is returned for it as Gate records no outcome of it.
    pub(crate) async fn record(
        &self,
        session: &AgentSession,
    ) -> Result<Option<(Hash, PromptCategory)>, String> {
        let payload = PromptPayload {
            text: self.text.clone(),
            category: self.category,
            classifier: self
                .category
                .map(|_| self.classifier.clone())
                .unwrap_or_default(),
            route: self.route.clone(),
            kind: Some(self.kind.clone()),
            led: None,
            difficulty: self.difficulty,
            assessment: self.assessment.clone(),
        };
        let (tx, _) = record_prompt(session, &payload).await?;
        Ok(self.category.map(|category| (tx.hash, category)))
    }
}
