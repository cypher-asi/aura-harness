//! One of Gate's requests to another model.

use aura_model::model_catalog::LOCAL_PREFIX;
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ReasonerError, ThinkingEffort,
    ToolChoice, ToolDefinition,
};

/// What Gate asks another model, before the model and the user's
/// sign-in are known: a system prompt and one user message. It goes as
/// an auxiliary request, apart from the agent's own calls, with thinking
/// off unless [`Self::thinking`] says otherwise, and is answered in text
/// unless it offers a tool.
pub(crate) struct Ask {
    purpose: ModelCallPurpose,
    system: &'static str,
    message: String,
    effort: ThinkingEffort,
    tool: Option<(ToolDefinition, ToolChoice)>,
}

impl Ask {
    /// Asking, for `purpose`, what `system` and `message` say.
    pub(crate) const fn new(
        purpose: ModelCallPurpose,
        system: &'static str,
        message: String,
    ) -> Self {
        Self {
            purpose,
            system,
            message,
            effort: ThinkingEffort::Off,
            tool: None,
        }
    }

    /// The same, thinking at `effort` before it answers.
    pub(crate) fn thinking(self, effort: ThinkingEffort) -> Self {
        Self { effort, ..self }
    }

    /// The same, answered by calling `tool` as `choice` says, and no
    /// other tool alongside it.
    pub(crate) fn tool(self, tool: ToolDefinition, choice: ToolChoice) -> Self {
        Self {
            tool: Some((tool, choice)),
            ..self
        }
    }

    /// The request asking `model`, sent with the user's `auth_token` when
    /// the route needs one. It sets no temperature: Claude 5 models
    /// reject any but the default.
    ///
    /// # Errors
    /// Returns the builder's error when `model` is empty.
    pub(crate) fn request(
        self,
        model: &str,
        auth_token: Option<String>,
    ) -> Result<ModelRequest, ReasonerError> {
        let builder = ModelRequest::builder(model, self.system)
            .message(Message::user(self.message))
            .thinking_effort(Some(self.effort))
            .auth_token(auth_token)
            .request_kind(ModelRequestKind::Auxiliary)
            .purpose(self.purpose);
        match self.tool {
            Some((tool, choice)) => builder
                .tools(vec![tool])
                .tool_choice(choice)
                .parallel_tool_use(false),
            None => builder.tool_choice(ToolChoice::None),
        }
        .try_build()
    }
}

/// Whether `model` is asked to answer in text rather than with a tool:
/// local models may not support a forced tool.
pub(crate) fn answers_in_text(model: &str) -> bool {
    model.starts_with(LOCAL_PREFIX)
}

/// `name: meaning` pairs as one line each, as a tool's schema describes
/// the values it takes.
pub(crate) fn meanings<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    pairs
        .into_iter()
        .map(|(name, meaning)| format!("{name}: {meaning}"))
        .collect::<Vec<_>>()
        .join("\n")
}
