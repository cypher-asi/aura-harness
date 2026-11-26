use super::{
    MaxTokens, ModelName, ModelRequest, PromptCacheRetention, Temperature, ThinkingConfig,
    ThinkingEffort,
};
use crate::error::ReasonerError;
use crate::types::content_profile::{ModelCallPurpose, ModelRequestKind, ModelRequestMetadata};
use crate::types::message::Message;
use crate::types::tool::{ToolChoice, ToolDefinition};
use std::sync::Arc;

/// Builder for `ModelRequest`.
pub struct ModelRequestBuilder {
    model: String,
    system: String,
    messages: Arc<Vec<Message>>,
    tools: Vec<ToolDefinition>,
    tool_choice: ToolChoice,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    thinking: Option<ThinkingConfig>,
    thinking_effort: Option<ThinkingEffort>,
    parallel_tool_use: bool,
    auth_token: Option<String>,
    upstream_provider_family: Option<String>,
    aura_project_id: Option<String>,
    aura_agent_id: Option<String>,
    aura_session_id: Option<String>,
    aura_org_id: Option<String>,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<PromptCacheRetention>,
    cache_salt: Option<String>,
    metadata: ModelRequestMetadata,
}

impl ModelRequestBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new(model: impl Into<String>, system: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            messages: Arc::default(),
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: None,
            temperature: None,
            thinking: None,
            thinking_effort: None,
            // Phase 3: codex default. See `ModelRequest::parallel_tool_use`.
            parallel_tool_use: true,
            auth_token: None,
            upstream_provider_family: None,
            aura_project_id: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
            prompt_cache_key: None,
            prompt_cache_retention: None,
            cache_salt: None,
            metadata: ModelRequestMetadata::default(),
        }
    }

    /// Set messages.
    #[must_use]
    pub fn messages(mut self, messages: impl Into<Arc<Vec<Message>>>) -> Self {
        self.messages = messages.into();
        self
    }

    /// Add a message.
    #[must_use]
    pub fn message(mut self, message: Message) -> Self {
        Arc::make_mut(&mut self.messages).push(message);
        self
    }

    /// Set tools.
    #[must_use]
    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    /// Set tool choice.
    #[must_use]
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Hold the reply to `max` tokens, below the model's own limit
    /// ([`crate::model_catalog::max_output_tokens`]) that it is otherwise
    /// sent with; a larger `max` is lowered to that limit.
    #[must_use]
    pub const fn max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set temperature.
    #[must_use]
    pub const fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set extended thinking configuration.
    #[must_use]
    pub const fn thinking(mut self, config: ThinkingConfig) -> Self {
        self.thinking = Some(config);
        self
    }

    /// Phase 2: set the explicit reasoning-effort policy for this
    /// request. Pass `None` to keep the legacy `max_tokens`-coupled
    /// auto-enable behaviour for the Anthropic provider.
    #[must_use]
    pub const fn thinking_effort(mut self, effort: Option<ThinkingEffort>) -> Self {
        self.thinking_effort = effort;
        self
    }

    /// Phase 3: enable or disable parallel tool-use for this request.
    /// Defaults to `true` (codex's default); set to `false` to opt
    /// individual call sites back into serial tool execution.
    #[must_use]
    pub const fn parallel_tool_use(mut self, allow: bool) -> Self {
        self.parallel_tool_use = allow;
        self
    }

    /// Set the auth token for proxy routing.
    #[must_use]
    pub fn auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token;
        self
    }

    /// Set the upstream provider family hint for managed proxy routing.
    #[must_use]
    pub fn upstream_provider_family(mut self, family: Option<String>) -> Self {
        self.upstream_provider_family = family;
        self
    }

    #[must_use]
    pub fn aura_project_id(mut self, id: Option<String>) -> Self {
        self.aura_project_id = id;
        self
    }

    #[must_use]
    pub fn aura_agent_id(mut self, id: Option<String>) -> Self {
        self.aura_agent_id = id;
        self
    }

    #[must_use]
    pub fn aura_session_id(mut self, id: Option<String>) -> Self {
        self.aura_session_id = id;
        self
    }

    #[must_use]
    pub fn aura_org_id(mut self, id: Option<String>) -> Self {
        self.aura_org_id = id;
        self
    }

    #[must_use]
    pub fn prompt_cache_key(mut self, key: Option<String>) -> Self {
        self.prompt_cache_key = key;
        self
    }

    #[must_use]
    pub fn prompt_cache_retention(mut self, retention: Option<PromptCacheRetention>) -> Self {
        self.prompt_cache_retention = retention;
        self
    }

    /// Set the [`ModelRequest::cache_salt`].
    #[must_use]
    pub fn cache_salt(mut self, salt: Option<String>) -> Self {
        self.cache_salt = salt;
        self
    }

    #[must_use]
    pub const fn request_kind(mut self, kind: ModelRequestKind) -> Self {
        self.metadata.kind = Some(kind);
        self
    }

    #[must_use]
    pub const fn purpose(mut self, purpose: ModelCallPurpose) -> Self {
        self.metadata.purpose = Some(purpose);
        self
    }

    #[must_use]
    pub fn metadata(mut self, metadata: ModelRequestMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Build the request, validating newtypes at the edge.
    ///
    /// # Errors
    /// Returns [`ReasonerError::Internal`] if any of the newtype invariants
    /// fail: empty model name, `max_tokens == 0`, or temperature outside
    /// `0.0..=2.0`.
    pub fn try_build(self) -> Result<ModelRequest, ReasonerError> {
        let limit = crate::model_catalog::max_output_tokens(&self.model);
        let max_tokens = MaxTokens::try_new(self.max_tokens.map_or(limit, |max| max.min(limit)))?;
        let model = ModelName::try_new(self.model)?;
        let temperature = self.temperature.map(Temperature::try_new).transpose()?;
        Ok(ModelRequest {
            model,
            system: self.system,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            max_tokens,
            temperature,
            thinking: self.thinking,
            thinking_effort: self.thinking_effort,
            parallel_tool_use: self.parallel_tool_use,
            auth_token: self.auth_token,
            upstream_provider_family: self.upstream_provider_family,
            extra_headers: Vec::new(),
            system_preamble: None,
            aura_project_id: self.aura_project_id,
            aura_agent_id: self.aura_agent_id,
            aura_session_id: self.aura_session_id,
            aura_org_id: self.aura_org_id,
            // Single clamp chokepoint: OpenAI rejects `prompt_cache_key`
            // strings longer than 64 chars. Every production request is
            // built through this builder, and both the OpenAI body field
            // and the `X-Aura-Prompt-Cache-Key` header downstream read
            // this one field, so clamping here is the only place the
            // limit needs to be enforced across the whole stack.
            prompt_cache_key: self
                .prompt_cache_key
                .map(aura_core_protocol::clamp_prompt_cache_key),
            prompt_cache_retention: self.prompt_cache_retention,
            cache_salt: self.cache_salt,
            metadata: self.metadata,
        })
    }
}
