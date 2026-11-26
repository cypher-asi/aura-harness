//! Model catalog shared by the terminal surface's model selector and the
//! [`crate::RoutedProvider`].
//!
//! The model id carries its route, chosen by the user:
//!
//! - [`LOCAL_PREFIX`] (`local/qwen3:4b`): local Anthropic-compatible
//!   endpoint (Ollama >= 0.14); no login.
//! - [`CLAUDE_PREFIX`] (`claude/claude-sonnet-4-6`): Anthropic API billed
//!   to the user's linked Claude plan.
//! - [`CHATGPT_PREFIX`] (`chatgpt/gpt-5.4`): OpenAI Responses API billed
//!   to the user's linked ChatGPT plan.
//! - [`OPENROUTER_PREFIX`] (`openrouter/anthropic/claude-sonnet-4.6`):
//!   OpenRouter, billed to the user's OpenRouter key. The rest of the id
//!   is OpenRouter's own `vendor/model[:variant]` slug.
//! - anything else: an aura-router alias that needs a zOS JWT from `/login`.
//!
//! [`GATE_MODEL_ID`] is not a model: the surface resolves it to one of
//! these before a request is built.
//!
//! # Live model info
//!
//! The curated tables here (prices, output limits, tiers) only know the
//! models aura ships with. Providers that list their models with limits
//! and prices (OpenRouter's `/models`) can fill the gaps through a
//! process-wide table, [`replace_live_models`] / [`live_model_info`].
//!
//! Invariants:
//!
//! - It is written only by the surface, after it fetched a provider's
//!   model listing, one backend at a time: every entry of a backend comes
//!   from the latest [`replace_live_models`] call for that backend.
//! - It is read only as a fallback: a curated price always wins, and a
//!   curated output limit wins except on OpenRouter ids, whose listing is
//!   the authority for what that route accepts.
//! - Its values only stand in for defaults the catalog would otherwise
//!   guess; entries that are not finite and positive are dropped on write,
//!   so a bad listing can never produce a zero limit or a negative price.
//! - A poisoned lock is recovered rather than propagated: the table holds
//!   plain fallback data, and a half-applied write leaves it no worse than
//!   an empty one.

use aura_core_auth::ProviderKind;

mod capabilities;
mod live;
mod local;
mod output_limits;
mod pricing;
mod selector;

pub use capabilities::{
    effort_change_resets_prompt_cache, has_max_effort, reasons_high, takes_forced_tool_choice,
};
pub use live::{
    live_context_window, live_model_info, replace_live_models, LiveModelInfo, LivePrice,
};
pub use local::{
    default_local_model_id, is_installed, local_base_url, local_context_tokens, local_model_id,
    local_model_name, probe_local_server, LocalServerStatus,
};
pub use output_limits::max_output_tokens;
pub use pricing::{billed_tokens, call_tokens, estimate_cost_usd, supersedes, BilledTokens};
pub use selector::{catalog, catalog_with_plans};

/// Prefix that marks a model as served by the local endpoint.
pub const LOCAL_PREFIX: &str = "local/";

/// Prefix that marks a model as served directly with a Claude subscription.
pub const CLAUDE_PREFIX: &str = "claude/";

/// Prefix that marks a model as served directly with a ChatGPT subscription.
pub const CHATGPT_PREFIX: &str = "chatgpt/";

/// Prefix that marks a model as served by OpenRouter with the user's
/// OpenRouter key. What follows is OpenRouter's model slug,
/// `vendor/model` with an optional `:variant` (`:nitro`, `:free`), sent
/// upstream unchanged.
pub const OPENROUTER_PREFIX: &str = "openrouter/";

/// Model id of Gate, the router that picks a model for each turn from
/// what the user's sign-ins allow.
pub const GATE_MODEL_ID: &str = "gate";

/// Lightweight local model used when nothing else is configured: small
/// enough for CPU-only laptops and supports native tool calling.
pub const DEFAULT_LOCAL_MODEL: &str = "qwen3:4b";

/// Default base URL of the local model server (Ollama).
pub const DEFAULT_LOCAL_URL: &str = "http://127.0.0.1:11434";

/// Context window assumed for local models unless overridden via
/// `AURA_LOCAL_CONTEXT_TOKENS`. Ollama must be started with a matching
/// `OLLAMA_CONTEXT_LENGTH`; its built-in default is much smaller.
pub const DEFAULT_LOCAL_CONTEXT_TOKENS: u64 = 32_768;

/// Output budget for local models: a reply shares the local server's
/// small context window ([`DEFAULT_LOCAL_CONTEXT_TOKENS`]) with the
/// prompt, and the agent loop keeps this much of it free.
pub const DEFAULT_LOCAL_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// Where a model is served from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelBackend {
    /// Local Anthropic-compatible server; no login required.
    Local,
    /// aura-router proxy; requires a zOS JWT.
    Router,
    /// Anthropic API with the user's Claude subscription.
    Claude,
    /// OpenAI Responses API with the user's ChatGPT subscription.
    Chatgpt,
    /// OpenRouter's Anthropic-compatible API with the user's OpenRouter key.
    OpenRouter,
}

impl ModelBackend {
    /// Short label for status displays.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Router => "cloud",
            Self::Claude => "claude plan",
            Self::Chatgpt => "chatgpt plan",
            Self::OpenRouter => "openrouter",
        }
    }

    /// Who pays for the tokens a call on this backend uses.
    #[must_use]
    pub const fn source_label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Router => "aura-router",
            Self::Claude => "Claude plan",
            Self::Chatgpt => "ChatGPT plan",
            Self::OpenRouter => "OpenRouter",
        }
    }

    /// Auth provider that must be signed in for this backend, if any.
    #[must_use]
    pub const fn required_provider(self) -> Option<ProviderKind> {
        match self {
            Self::Local => None,
            Self::Router => Some(ProviderKind::Zos),
            Self::Claude => Some(ProviderKind::Claude),
            Self::Chatgpt => Some(ProviderKind::Chatgpt),
            Self::OpenRouter => Some(ProviderKind::OpenRouter),
        }
    }
}

/// How capable a model is, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelTier {
    /// Served by the local model server.
    Local,
    /// Small and cheap cloud models (Haiku, GPT mini).
    Fast,
    /// Mid-size cloud models (Sonnet).
    Standard,
    /// The strongest cloud models (Opus, GPT-5).
    Frontier,
}

impl ModelTier {
    /// Short label for status displays.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Fast => "fast",
            Self::Standard => "standard",
            Self::Frontier => "frontier",
        }
    }
}

/// Tier of `model`, inferred from its name for models outside the curated
/// lists. Unknown cloud models count as standard.
#[must_use]
pub fn infer_tier(model: &str) -> ModelTier {
    if backend_for(model) == ModelBackend::Local {
        return ModelTier::Local;
    }
    let key = model_key(model);
    let has = |words: &[&str]| words.iter().any(|w| key.contains(w));
    if has(&["haiku", "mini", "nano", "luna"]) {
        ModelTier::Fast
    } else if has(&["sonnet", "terra"]) {
        ModelTier::Standard
    } else if has(&["opus", "fable", "mythos"]) || key.starts_with("gpt-") {
        ModelTier::Frontier
    } else {
        ModelTier::Standard
    }
}

/// One selectable model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// Full model id as sent in `ModelRequest.model` (local ids carry [`LOCAL_PREFIX`]).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Serving backend.
    pub backend: ModelBackend,
    /// For local models: whether the model is installed on the local server.
    pub installed: bool,
    /// How capable the model is.
    pub tier: ModelTier,
}

impl ModelOption {
    /// Whether selecting this model requires an aura-router login.
    #[must_use]
    pub fn requires_login(&self) -> bool {
        self.backend == ModelBackend::Router
    }

    /// Auth provider that must be signed in to use this model, if any.
    #[must_use]
    pub const fn required_provider(&self) -> Option<ProviderKind> {
        self.backend.required_provider()
    }
}

/// Backend serving `model`.
#[must_use]
pub fn backend_for(model: &str) -> ModelBackend {
    if model.starts_with(LOCAL_PREFIX) {
        ModelBackend::Local
    } else if model.starts_with(CLAUDE_PREFIX) {
        ModelBackend::Claude
    } else if model.starts_with(CHATGPT_PREFIX) {
        ModelBackend::Chatgpt
    } else if model.starts_with(OPENROUTER_PREFIX) {
        ModelBackend::OpenRouter
    } else {
        ModelBackend::Router
    }
}

/// Model name as the serving backend knows it: strips the `local/`,
/// `claude/`, `chatgpt/` or `openrouter/` route prefix (leaving
/// OpenRouter's `vendor/model` slug); router aliases are unchanged.
#[must_use]
pub fn upstream_model_name(model: &str) -> &str {
    [
        LOCAL_PREFIX,
        CLAUDE_PREFIX,
        CHATGPT_PREFIX,
        OPENROUTER_PREFIX,
    ]
    .iter()
    .find_map(|prefix| model.strip_prefix(prefix))
    .filter(|name| !name.trim().is_empty())
    .unwrap_or(model)
}

/// The model and who serves it: `claude-sonnet-4-6 (claude plan)`.
#[must_use]
pub fn route_label(model: &str) -> String {
    format!(
        "{} ({})",
        upstream_model_name(model),
        backend_for(model).label()
    )
}

/// The upstream model `model` names on any route: without its route or
/// router prefix, lowercased, `.` as `-` (`aura-gpt-5-4`,
/// `chatgpt/gpt-5.4` -> `gpt-5-4`). OpenRouter ids also lose their vendor
/// segment and `:variant` (`openrouter/openai/gpt-5.4:nitro` ->
/// `gpt-5-4`), so one upstream model shares its key, price and limits
/// across every route.
///
/// A name without a route prefix is taken as it is, even when it looks
/// like an OpenRouter slug (`openai/gpt-oss-20b`): local servers name
/// their models that way too, so only the route a call took can say
/// that such a name is OpenRouter's.
#[must_use]
pub fn model_key(model: &str) -> String {
    let name = upstream_model_name(model);
    let name = if backend_for(model) == ModelBackend::OpenRouter {
        openrouter_model_name(name)
    } else {
        name
    };
    let name = name.strip_prefix("aura-").unwrap_or(name);
    name.to_ascii_lowercase().replace('.', "-")
}

/// `id` without its OpenRouter `:variant` (`openrouter/x/y:nitro` ->
/// `openrouter/x/y`), `None` when it has none.
pub(super) fn without_variant(id: &str) -> Option<&str> {
    id.rsplit_once(':')
        .filter(|(base, variant)| !base.is_empty() && !variant.contains('/'))
        .map(|(base, _)| base)
}

/// The model in an OpenRouter slug, without vendor or variant
/// (`anthropic/claude-sonnet-4.6:nitro` -> `claude-sonnet-4.6`). A slug
/// that would leave nothing is returned whole.
fn openrouter_model_name(slug: &str) -> &str {
    let model = slug.split_once('/').map_or(slug, |(_, model)| model);
    let model = model.split_once(':').map_or(model, |(model, _)| model);
    if model.trim().is_empty() {
        slug
    } else {
        model
    }
}

/// Whether `name` is `key` or one of its dated snapshots
/// (`claude-haiku-4-5-20251001`, `gpt-5-4-2026-03-05`).
pub(super) fn is_snapshot_of(name: &str, key: &str) -> bool {
    match name.strip_prefix(key) {
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|date| {
            date.len() >= 8 && date.bytes().take(4).all(|b| b.is_ascii_digit())
        }),
        None => false,
    }
}

#[cfg(test)]
mod tests;
