mod builder;
mod primitives;
mod thinking;

pub use builder::ModelRequestBuilder;
pub use primitives::{MaxTokens, ModelName, PromptCacheRetention, Temperature};
pub use thinking::{ThinkingConfig, ThinkingEffort};

use super::content_profile::ModelRequestMetadata;
use super::message::Message;
use super::tool::{ToolChoice, ToolDefinition};
use std::sync::Arc;

// ============================================================================
// Model Request
// ============================================================================

/// Request to the model.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Model identifier (e.g., "claude-opus-4-6")
    pub model: ModelName,
    /// System prompt
    pub system: String,
    /// Conversation messages, shared with the conversation they were
    /// taken from: an agent asks again with all of them each step
    pub messages: Arc<Vec<Message>>,
    /// Available tools
    pub tools: Vec<ToolDefinition>,
    /// Tool choice mode
    pub tool_choice: ToolChoice,
    /// Maximum tokens to generate
    pub max_tokens: MaxTokens,
    /// Sampling temperature
    pub temperature: Option<Temperature>,
    /// Extended thinking configuration. When `Some`, the provider enables
    /// thinking with the given budget. When `None`, provider-default behavior
    /// applies.
    pub thinking: Option<ThinkingConfig>,
    /// Phase 2: explicit reasoning-effort policy applied per request.
    ///
    /// When `Some(_)`, the Anthropic provider uses the matching budget
    /// preset (see [`ThinkingEffort`]). When `None`, the provider falls
    /// through to the legacy `max_tokens > 2048` auto-enable path so
    /// non-migrated callers keep their current behaviour.
    pub thinking_effort: Option<ThinkingEffort>,
    /// Phase 3: allow the model to emit multiple `tool_use` blocks in
    /// one assistant turn. Codex enables the equivalent
    /// `parallel_tool_calls: true` flag by default
    /// (codex-rs/core/src/client.rs:759); aura ships the same default
    /// so a 5-file exploration can collapse into a single iteration
    /// instead of feeding the doom loop with one read per turn.
    ///
    /// Set `false` to add `disable_parallel_tool_use: true` to the
    /// Anthropic `tool_choice` payload, forcing serial tool execution.
    pub parallel_tool_use: bool,
    /// Optional JWT auth token for proxy routing.
    pub auth_token: Option<String>,
    /// Optional upstream provider family hint for managed proxy routing.
    pub upstream_provider_family: Option<String>,
    /// Extra HTTP headers required by a direct subscription route
    /// (e.g. `anthropic-beta` flags). `anthropic-beta` values are merged
    /// into the provider's own beta header.
    pub extra_headers: Vec<(String, String)>,
    /// Identity line a direct subscription route requires ahead of
    /// `system`. Anthropic plans only accept it as its own first system
    /// block, never merged into the rest of the prompt.
    pub system_preamble: Option<String>,
    /// Project ID for X-Aura-Project-Id billing header.
    pub aura_project_id: Option<String>,
    /// Project-agent UUID for X-Aura-Agent-Id billing header.
    pub aura_agent_id: Option<String>,
    /// Storage session UUID for X-Aura-Session-Id billing header.
    pub aura_session_id: Option<String>,
    /// Org UUID for X-Aura-Org-Id billing header.
    pub aura_org_id: Option<String>,
    /// Optional stable cache key forwarded to the router so OpenAI-family
    /// upstreams can pin identical prefixes to the same backend partition
    /// (`prompt_cache_key` in the OpenAI API). The harness only carries
    /// it on the wire; the router rewrites Anthropic-shape requests into
    /// OpenAI-native ones and is responsible for actually attaching this
    /// to the OpenAI Responses/Chat API call. Ignored for Anthropic
    /// family because Anthropic's own prompt caching is opt-in via
    /// `cache_control` blocks rather than a routing hint.
    pub prompt_cache_key: Option<String>,
    /// Optional retention hint paired with `prompt_cache_key`. See
    /// [`PromptCacheRetention`] for the wire values.
    pub prompt_cache_retention: Option<PromptCacheRetention>,
    /// Keeps this request's provider prompt-cache entries apart from those
    /// of requests with another salt, or none, even when everything else
    /// they send is the same (the two arms of a `/compare`). Each provider
    /// writes it as a short inert marker into the earliest prompt content
    /// it caches; `None` sends the request exactly as without this field.
    pub cache_salt: Option<String>,
    /// Optional request-contract metadata used by provider-bound validation.
    pub metadata: ModelRequestMetadata,
}

impl ModelRequest {
    /// Create a new model request builder.
    #[must_use]
    pub fn builder(model: impl Into<String>, system: impl Into<String>) -> ModelRequestBuilder {
        ModelRequestBuilder::new(model, system)
    }

    /// Give the request `salt` as its [`Self::cache_salt`]. It is also
    /// the OpenAI-family `prompt_cache_key` of a request that has none, so
    /// the router does not pin differently salted requests to one cache
    /// partition.
    pub fn scope_cache(&mut self, salt: &str) {
        self.cache_salt = Some(salt.to_string());
        if self.prompt_cache_key.is_none() {
            self.prompt_cache_key =
                Some(aura_core_protocol::clamp_prompt_cache_key(salt.to_string()));
        }
    }
}

#[cfg(test)]
mod newtype_tests;
#[cfg(test)]
mod thinking_effort_tests;
