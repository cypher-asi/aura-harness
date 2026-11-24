//! What a turn's prompt is recorded with once Gate labels it: what the
//! classifier says it asks for and how hard it is, where the turn runs,
//! and whether it is led.

use crate::event_loop::agents::AgentSession;
use crate::event_loop::turns::record_turn_prompt;
use aura_agent::AgentLoopConfig;
use aura_core_types::{Hash, PromptCategory, PromptPayload, Route, RouteReason};
use aura_gate::live;

/// How Gate records a turn's prompt: who classifies it, and why the turn
/// runs on its model. A turn on a pinned model runs beside the
/// classifier; a turn Gate routes waits for its category.
pub(crate) struct Routing {
    /// Model that classifies the prompt
    pub classifier: String,
    /// Why the turn runs on its model, until Gate decides
    pub reason: RouteReason,
}

/// A turn's prompt as Gate labels it, and the sign-in its classifier and
/// criteria writer are asked with.
pub(super) struct Label {
    /// The prompt as it is recorded once classified; Gate's choices fill
    /// in its route and whether the turn is led
    pub prompt: PromptPayload,
    pub auth_token: Option<String>,
}

impl Label {
    /// `text`, the prompt of a turn on `config` that `routing` records,
    /// before it is classified.
    pub(super) fn new(text: String, routing: &Routing, config: &AgentLoopConfig) -> Self {
        let prompt = PromptPayload {
            classifier: routing.classifier.clone(),
            route: Route::turn(config.model.clone(), routing.reason),
            ..PromptPayload::new(text)
        };
        Self {
            prompt,
            auth_token: config.auth_token.clone(),
        }
    }

    /// The prompt's category, as the classifier says; its difficulty and
    /// what the classifier found about it are kept on the prompt.
    ///
    /// # Errors
    /// Says why when the classifier gave no category and difficulty.
    pub(super) async fn classify(
        &mut self,
        session: &AgentSession,
    ) -> Result<PromptCategory, String> {
        let prompt = &mut self.prompt;
        let token = self.auth_token.clone();
        let label =
            live::classify_prompt(&session.kernel, &prompt.classifier, &prompt.text, token).await?;
        prompt.difficulty = Some(label.difficulty);
        prompt.assessment = label.assessment;
        Ok(label.category)
    }

    /// Record the prompt on `session` as asking for `category`, returning
    /// its transaction hash.
    ///
    /// # Errors
    /// Says why when the prompt could not be recorded.
    pub(super) async fn record(
        &mut self,
        session: &AgentSession,
        category: PromptCategory,
    ) -> Result<Hash, String> {
        self.prompt.category = Some(category);
        record_turn_prompt(session, &self.prompt).await
    }
}
