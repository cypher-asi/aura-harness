use super::message::Message;
use super::tool::{ToolChoice, ToolDefinition};
use serde::{Deserialize, Serialize};

// ============================================================================
// Thinking Configuration
// ============================================================================

/// Per-request extended thinking configuration.
///
/// When set on a `ModelRequest`, the provider will enable extended thinking
/// with the specified budget. When `None`, the provider may apply its own
/// heuristic (e.g., auto-enable for capable models).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Token budget allocated for the thinking phase.
    /// Must be >= 1024 and < `max_tokens`.
    pub budget_tokens: u32,
}

// ============================================================================
// Model Request
// ============================================================================

/// Request to the model.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Model identifier (e.g., "claude-opus-4-6")
    pub model: String,
    /// System prompt
    pub system: String,
    /// Conversation messages
    pub messages: Vec<Message>,
    /// Available tools
    pub tools: Vec<ToolDefinition>,
    /// Tool choice mode
    pub tool_choice: ToolChoice,
    /// Maximum tokens to generate
    pub max_tokens: u32,
    /// Sampling temperature
    pub temperature: Option<f32>,
    /// Extended thinking configuration. When `Some`, the provider enables
    /// thinking with the given budget. When `None`, provider-default behavior
    /// applies.
    pub thinking: Option<ThinkingConfig>,
    /// Optional JWT auth token for proxy routing.
    pub auth_token: Option<String>,
    /// Project ID for X-Aura-Project-Id billing header.
    pub aura_project_id: Option<String>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    pub aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    pub aura_session_id: Option<String>,
    /// Org UUID for X-Aura-Org-Id billing header.
    pub aura_org_id: Option<String>,
}

impl ModelRequest {
    /// Create a new model request builder.
    #[must_use]
    pub fn builder(model: impl Into<String>, system: impl Into<String>) -> ModelRequestBuilder {
        ModelRequestBuilder::new(model, system)
    }
}

/// Builder for `ModelRequest`.
pub struct ModelRequestBuilder {
    model: String,
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    tool_choice: ToolChoice,
    max_tokens: u32,
    temperature: Option<f32>,
    thinking: Option<ThinkingConfig>,
    auth_token: Option<String>,
    aura_project_id: Option<String>,
    aura_agent_id: Option<String>,
    aura_session_id: Option<String>,
    aura_org_id: Option<String>,
}

impl ModelRequestBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new(model: impl Into<String>, system: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: 4096,
            temperature: None,
            thinking: None,
            auth_token: None,
            aura_project_id: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
        }
    }

    /// Set messages.
    #[must_use]
    pub fn messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Add a message.
    #[must_use]
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
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

    /// Set max tokens.
    #[must_use]
    pub const fn max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = max;
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

    /// Set the auth token for proxy routing.
    #[must_use]
    pub fn auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token;
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

    /// Build the request.
    #[must_use]
    pub fn build(self) -> ModelRequest {
        ModelRequest {
            model: self.model,
            system: self.system,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            thinking: self.thinking,
            auth_token: self.auth_token,
            aura_project_id: self.aura_project_id,
            aura_agent_id: self.aura_agent_id,
            aura_session_id: self.aura_session_id,
            aura_org_id: self.aura_org_id,
        }
    }
}
