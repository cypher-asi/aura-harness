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

use std::collections::BTreeMap;
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use serde::Deserialize;

use aura_core_auth::ProviderKind;

use crate::Usage;

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

/// Output for a model whose API sets no limit of its own beyond its
/// context window (Grok, the open-weight models served by Fireworks):
/// the largest limit any API aura calls publishes. Fireworks lowers it to
/// what the window has left.
const CONTEXT_BOUND_OUTPUT_TOKENS: u32 = 128_000;

/// Output for a model this table does not know.
const UNKNOWN_OUTPUT_TOKENS: u32 = 32_000;

/// The most tokens one reply of `model` may hold: the limit its API
/// publishes, which requests are sent with unless a caller asks for less.
/// OpenRouter ids take the limit OpenRouter listed for them; other models
/// fall back to a listed limit only when the curated table does not know
/// them (see [the module docs](self#live-model-info)).
#[must_use]
pub fn max_output_tokens(model: &str) -> u32 {
    let backend = backend_for(model);
    if backend == ModelBackend::Local {
        return DEFAULT_LOCAL_MAX_OUTPUT_TOKENS;
    }
    let live = live_model_info(model).and_then(|info| info.max_output);
    if backend == ModelBackend::OpenRouter {
        if let Some(limit) = live {
            return limit;
        }
    }
    match curated_max_output_tokens(&model_key(model)) {
        UNKNOWN_OUTPUT_TOKENS => live.unwrap_or(UNKNOWN_OUTPUT_TOKENS),
        limit => limit,
    }
}

/// Output limit the curated table gives the model named by `key` (a
/// [`model_key`]), or [`UNKNOWN_OUTPUT_TOKENS`].
fn curated_max_output_tokens(key: &str) -> u32 {
    let has = |words: &[&str]| words.iter().any(|w| key.contains(w));
    if key.contains("claude") {
        return if has(&["opus-4-5", "sonnet-4-5", "haiku-4-5", "3-7-sonnet"]) {
            64_000
        } else if has(&["opus-4-1"]) || key.starts_with("claude-opus-4-20") {
            32_000
        } else if key.starts_with("claude-sonnet-4-20") {
            64_000
        } else if has(&["claude-3-5"]) {
            8_192
        } else if has(&["claude-3-"]) {
            4_096
        } else {
            128_000
        };
    }
    if has(&[
        "oss-120b", "qwen", "deepseek", "kimi", "minimax", "glm", "grok",
    ]) {
        CONTEXT_BOUND_OUTPUT_TOKENS
    } else if has(&["gpt-5", "gpt-6"]) {
        128_000
    } else if has(&["gpt-4-1"]) {
        32_768
    } else if has(&["gpt-4o"]) {
        16_384
    } else if key.starts_with("o1") || key.starts_with("o3") || key.starts_with("o4") {
        100_000
    } else if has(&["gemini"]) {
        65_536
    } else {
        UNKNOWN_OUTPUT_TOKENS
    }
}

/// Minimum Ollama version exposing the Anthropic-compatible `/v1/messages`.
const MIN_OLLAMA_VERSION: (u32, u32) = (0, 14);

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

/// Curated aura-router aliases offered in the selector.
const ROUTER_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "aura-claude-opus-4-7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "aura-claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    ("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
    ("aura-gpt-5-4", "GPT-5.4", ModelTier::Frontier),
];

/// Curated models offered with a linked Claude plan.
const CLAUDE_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "claude/claude-opus-4-7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "claude/claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    (
        "claude/claude-haiku-4-5",
        "Claude Haiku 4.5",
        ModelTier::Fast,
    ),
];

/// Curated models offered with a linked ChatGPT plan.
const CHATGPT_MODELS: &[(&str, &str, ModelTier)] =
    &[("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier)];

/// Curated models offered through OpenRouter.
const OPENROUTER_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "openrouter/anthropic/claude-opus-4.7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "openrouter/anthropic/claude-sonnet-4.6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    (
        "openrouter/anthropic/claude-haiku-4.5",
        "Claude Haiku 4.5",
        ModelTier::Fast,
    ),
    ("openrouter/openai/gpt-5.4", "GPT-5.4", ModelTier::Frontier),
];

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

/// API list price of a model, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Price {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

const fn price(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write,
        cache_read,
    }
}

/// Standard short-context list prices (Anthropic and OpenAI, November
/// 2026). Keys write `.` as `-`; a key also covers its dated snapshots.
/// Anthropic cache writes are the 5-minute rate.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", price(10.0, 50.0, 12.5, 0.25)),
    ("claude-fable-5", price(10.0, 50.0, 12.5, 1.0)),
    ("claude-mythos-5-1", price(10.0, 50.0, 12.5, 0.25)),
    ("claude-mythos-5", price(10.0, 50.0, 12.5, 1.0)),
    ("claude-opus-5-5", price(4.0, 20.0, 5.0, 0.2)),
    ("claude-opus-5", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-8", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-7", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-6", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-5", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-1", price(15.0, 75.0, 18.75, 1.5)),
    ("claude-opus-4", price(15.0, 75.0, 18.75, 1.5)),
    ("claude-sonnet-5-5", price(2.0, 10.0, 2.5, 0.1)),
    ("claude-sonnet-5", price(2.0, 10.0, 2.5, 0.2)),
    ("claude-sonnet-4-6", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-sonnet-4-5", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-sonnet-4", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-haiku-5-5", price(0.1, 0.5, 0.125, 0.01)),
    ("claude-haiku-4-5", price(1.0, 5.0, 1.25, 0.1)),
    ("claude-3-5-haiku", price(0.8, 4.0, 1.0, 0.08)),
    ("gpt-6-astra", price(10.0, 50.0, 12.5, 1.0)),
    ("gpt-6-1-sol", price(2.0, 10.0, 2.5, 0.1)),
    ("gpt-6-1-terra", price(1.0, 5.0, 1.25, 0.05)),
    ("gpt-6-sol", price(2.0, 10.0, 2.5, 0.2)),
    ("gpt-6-luna", price(0.1, 0.5, 0.125, 0.01)),
    ("gpt-5-6-sol", price(4.0, 20.0, 5.0, 0.4)),
    ("gpt-5-6-terra", price(2.0, 12.0, 2.5, 0.2)),
    ("gpt-5-6-luna", price(0.2, 1.2, 0.25, 0.02)),
    ("gpt-5-5-pro", price(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5-5", price(5.0, 30.0, 5.0, 0.5)),
    ("gpt-5-4-pro", price(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5-4-mini", price(0.75, 4.5, 0.75, 0.075)),
    ("gpt-5-4-nano", price(0.2, 1.25, 0.2, 0.02)),
    ("gpt-5-4", price(2.5, 15.0, 2.5, 0.25)),
    ("gpt-5-2-pro", price(21.0, 168.0, 21.0, 21.0)),
    ("gpt-5-2", price(1.75, 14.0, 1.75, 0.175)),
    ("gpt-5-1", price(1.25, 10.0, 1.25, 0.125)),
    ("gpt-5-pro", price(15.0, 120.0, 15.0, 15.0)),
    ("gpt-5-mini", price(0.25, 2.0, 0.25, 0.025)),
    ("gpt-5-nano", price(0.05, 0.4, 0.05, 0.005)),
    ("gpt-5", price(1.25, 10.0, 1.25, 0.125)),
];

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
fn without_variant(id: &str) -> Option<&str> {
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

/// Whether changing the thinking effort between two requests of one
/// conversation makes `model`'s provider write the conversation to its
/// prompt cache again. Claude models do: only the tools and the system
/// prompt are still read from the cache after a change.
#[must_use]
pub fn effort_change_resets_prompt_cache(model: &str) -> bool {
    model_key(model).starts_with("claude-")
}

/// Whether an agent's turns on `model` reason at high effort when nobody
/// chose one: Claude Sonnet 5.5 and the Claude Opus 5 models. They get
/// the work that is hard to get right, where an attempt that fails costs
/// more than the extra reasoning does. Not max: at max one call on a long
/// conversation thinks for minutes, and a stream cut off before it ends
/// starts that thinking over.
#[must_use]
pub fn reasons_high(model: &str) -> bool {
    let key = model_key(model);
    is_snapshot_of(&key, "claude-sonnet-5-5")
        || is_snapshot_of(&key, "claude-opus-5")
        || is_snapshot_of(&key, "claude-opus-5-5")
}

/// Whether `model` reasons harder at max effort than at high: the Claude
/// models whose effort ladder goes up to `max` (Sonnet and Opus 4.6 and
/// later, Fable and Mythos 5). Others fold max into their highest effort.
#[must_use]
pub fn has_max_effort(model: &str) -> bool {
    let key = model_key(model);
    [
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-mythos-5",
    ]
    .iter()
    .any(|family| key.starts_with(family))
}

/// Whether `model` takes a tool choice that forces a call (any tool, or a
/// named one). Claude Sonnet 5.5 does not: the API answers `tool_choice:
/// type "tool" and "any" are not supported for this model`. Nor does Opus
/// 5.5, which cannot turn thinking off, and forced tool use needs thinking
/// off. Ask them with `auto` instead.
#[must_use]
pub fn takes_forced_tool_choice(model: &str) -> bool {
    let key = model_key(model);
    !["claude-sonnet-5-5", "claude-opus-5-5"]
        .iter()
        .any(|name| is_snapshot_of(&key, name))
}

/// Whether `name` is `key` or one of its dated snapshots
/// (`claude-haiku-4-5-20251001`, `gpt-5-4-2026-03-05`).
fn is_snapshot_of(name: &str, key: &str) -> bool {
    match name.strip_prefix(key) {
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|date| {
            date.len() >= 8 && date.bytes().take(4).all(|b| b.is_ascii_digit())
        }),
        None => false,
    }
}

/// List price of `model`: the curated [`PRICES`] entry, else the price
/// its provider listed (see [the module docs](self#live-model-info)).
fn list_price(model: &str) -> Option<Price> {
    if backend_for(model) == ModelBackend::Local {
        return None;
    }
    let key = model_key(model);
    PRICES
        .iter()
        .find(|(name, _)| is_snapshot_of(&key, name))
        .map(|(_, price)| *price)
        .or_else(|| {
            live_model_info(model)
                .and_then(|info| info.price)
                .map(|live| price(live.input, live.output, live.cache_write, live.cache_read))
        })
}

/// The family `key` (a [`model_key`]) belongs to and its version there:
/// the words of its name, and the numbers among them
/// (`claude-sonnet-5-5` -> `claude-sonnet` 5.5, `gpt-5-4-mini` ->
/// `gpt-mini` 5.4). A dated snapshot's date is left out. `None` when the
/// name has no version.
fn family_version(key: &str) -> Option<(String, Vec<u32>)> {
    let mut words = Vec::new();
    let mut version = Vec::new();
    for part in key.split('-') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            words.push(part);
        } else if part.len() >= 4 {
            break;
        } else {
            version.push(part.parse().ok()?);
        }
    }
    (!version.is_empty()).then(|| (words.join("-"), version))
}

/// Whether `newer` leaves no reason to send work to `older`: it is a
/// later version of the same model family, and no kind of token costs
/// more on it. A model whose name has no version or whose price is not
/// known supersedes nothing, and nothing supersedes it.
#[must_use]
pub fn supersedes(newer: &str, older: &str) -> bool {
    let (Some(new), Some(old)) = (list_price(newer), list_price(older)) else {
        return false;
    };
    let (Some((new_family, new_version)), Some((old_family, old_version))) = (
        family_version(&model_key(newer)),
        family_version(&model_key(older)),
    ) else {
        return false;
    };
    new_family == old_family
        && new_version > old_version
        && new.input <= old.input
        && new.output <= old.output
        && new.cache_write <= old.cache_write
        && new.cache_read <= old.cache_read
}

/// A call's prompt and output tokens, split the way they are billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BilledTokens {
    /// Prompt tokens neither read from nor written to the cache
    pub uncached_input: u64,
    /// Prompt tokens written to the cache
    pub cache_write: u64,
    /// Prompt tokens read from the cache
    pub cache_read: u64,
    /// Output tokens
    pub output: u64,
}

/// Split `usage` of a call to `model` the way it is billed.
#[must_use]
pub fn billed_tokens(model: &str, usage: &Usage) -> BilledTokens {
    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
    let cache_write = usage.cache_creation_input_tokens.unwrap_or(0);
    // OpenAI counts cached tokens inside `input_tokens`; Anthropic-format
    // responses report them on top of it. Only the ChatGPT route and
    // aura-router's GPT aliases answer in OpenAI shape: OpenRouter's
    // `/v1/messages` (and the local server's) reports Anthropic-format
    // usage whatever model is upstream. Pass the id the call asked for:
    // the name an answer comes back with does not say which endpoint
    // reported it.
    let openai = match backend_for(model) {
        ModelBackend::Chatgpt => true,
        ModelBackend::Router => model_key(model).starts_with("gpt-"),
        ModelBackend::Local | ModelBackend::Claude | ModelBackend::OpenRouter => false,
    };
    let uncached_input = if openai {
        usage.input_tokens.saturating_sub(cache_read)
    } else {
        usage.input_tokens
    };
    BilledTokens {
        uncached_input,
        cache_write,
        cache_read,
        output: usage.output_tokens,
    }
}

/// Tokens a model call used: its whole prompt (cached or not) plus output.
#[must_use]
pub fn call_tokens(model: &str, usage: &Usage) -> u64 {
    let tokens = billed_tokens(model, usage);
    tokens.uncached_input + tokens.cache_write + tokens.cache_read + tokens.output
}

/// What a model call costs at API list prices, in USD. `None` for local
/// models and models without a known price.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn estimate_cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    let price = list_price(model)?;
    let tokens = billed_tokens(model, usage);
    let micro_usd = tokens.uncached_input as f64 * price.input
        + tokens.cache_write as f64 * price.cache_write
        + tokens.cache_read as f64 * price.cache_read
        + tokens.output as f64 * price.output;
    Some(micro_usd / 1_000_000.0)
}

/// What a provider's model listing says about one model. Every field is
/// optional: a listing may leave any of them out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveModelInfo {
    /// Context window, in tokens.
    pub context_window: Option<u64>,
    /// Most output tokens one reply may hold.
    pub max_output: Option<u32>,
    /// Price per token kind.
    pub price: Option<LivePrice>,
}

/// A listed model price, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePrice {
    /// Uncached prompt tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Prompt tokens written to the cache.
    pub cache_write: f64,
    /// Prompt tokens read from the cache.
    pub cache_read: f64,
}

impl LivePrice {
    /// Whether every rate is a real, non-negative number.
    fn is_valid(&self) -> bool {
        [self.input, self.output, self.cache_write, self.cache_read]
            .iter()
            .all(|rate| rate.is_finite() && *rate >= 0.0)
    }
}

impl LiveModelInfo {
    /// `self` without the values no model can have (zero limits, prices
    /// that are negative or not numbers), or `None` when nothing is left.
    fn sanitized(self) -> Option<Self> {
        let info = Self {
            context_window: self.context_window.filter(|tokens| *tokens > 0),
            max_output: self.max_output.filter(|tokens| *tokens > 0),
            price: self.price.filter(LivePrice::is_valid),
        };
        let is_empty =
            info.context_window.is_none() && info.max_output.is_none() && info.price.is_none();
        (!is_empty).then_some(info)
    }
}

/// Listed model info by full model id; see [the module
/// docs](self#live-model-info) for who writes it and how it is read.
static LIVE_MODELS: RwLock<BTreeMap<String, LiveModelInfo>> = RwLock::new(BTreeMap::new());

/// Replace everything known live about `backend`'s models with `entries`
/// (full model id, info), as fetched from its provider's model listing.
/// Entries whose id names another backend are ignored, so each backend's
/// entries always come from its own latest listing; values no model can
/// have are dropped.
pub fn replace_live_models(
    backend: ModelBackend,
    entries: impl IntoIterator<Item = (String, LiveModelInfo)>,
) {
    let fresh: Vec<(String, LiveModelInfo)> = entries
        .into_iter()
        .filter(|(id, _)| backend_for(id) == backend)
        .filter_map(|(id, info)| info.sanitized().map(|info| (id, info)))
        .collect();
    let mut table = LIVE_MODELS.write().unwrap_or_else(PoisonError::into_inner);
    table.retain(|id, _| backend_for(id) != backend);
    table.extend(fresh);
}

/// What the latest model listing of its provider said about `model`:
/// the entry listed under `model` itself, else, for an OpenRouter id,
/// under that id without its `:variant` (`:nitro` is listed as its base
/// model).
#[must_use]
pub fn live_model_info(model: &str) -> Option<LiveModelInfo> {
    let table = LIVE_MODELS.read().unwrap_or_else(PoisonError::into_inner);
    let listed = |id: &str| table.get(id).copied();
    listed(model).or_else(|| {
        if backend_for(model) != ModelBackend::OpenRouter {
            return None;
        }
        listed(without_variant(model)?)
    })
}

/// Context window the latest model listing gave `model`, in tokens.
#[must_use]
pub fn live_context_window(model: &str) -> Option<u64> {
    live_model_info(model).and_then(|info| info.context_window)
}

/// Model name as the local server knows it (`local/qwen3:4b` -> `qwen3:4b`).
#[must_use]
pub fn local_model_name(model: &str) -> Option<&str> {
    model
        .strip_prefix(LOCAL_PREFIX)
        .filter(|name| !name.trim().is_empty())
}

/// Prefix a local server model name into a full model id.
#[must_use]
pub fn local_model_id(name: &str) -> String {
    format!("{LOCAL_PREFIX}{name}")
}

/// Local model selected by default: `AURA_LOCAL_MODEL` or [`DEFAULT_LOCAL_MODEL`].
#[must_use]
pub fn default_local_model_id() -> String {
    let name = std::env::var("AURA_LOCAL_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LOCAL_MODEL.to_string());
    local_model_id(name.strip_prefix(LOCAL_PREFIX).unwrap_or(&name))
}

/// Base URL of the local model server: `AURA_LOCAL_MODEL_URL` or [`DEFAULT_LOCAL_URL`].
#[must_use]
pub fn local_base_url() -> String {
    std::env::var("AURA_LOCAL_MODEL_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LOCAL_URL.to_string())
}

/// Context window for local models: `AURA_LOCAL_CONTEXT_TOKENS` or
/// [`DEFAULT_LOCAL_CONTEXT_TOKENS`].
#[must_use]
pub fn local_context_tokens() -> u64 {
    std::env::var("AURA_LOCAL_CONTEXT_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|tokens| *tokens > 0)
        .unwrap_or(DEFAULT_LOCAL_CONTEXT_TOKENS)
}

/// Build the selector catalog: the default local model, any other
/// installed local models, the curated router aliases, then the
/// subscription models (shown regardless of link state; selection is
/// gated by [`ModelOption::required_provider`]).
#[must_use]
pub fn catalog(default_local: &str, installed_local: &[String]) -> Vec<ModelOption> {
    catalog_with_plans(default_local, installed_local, &[])
}

/// [`catalog`], with each curated list of a cloud backend (plans and
/// OpenRouter) replaced by `plan_models` (fetched live from the provider)
/// for every backend it covers.
#[must_use]
pub fn catalog_with_plans(
    default_local: &str,
    installed_local: &[String],
    plan_models: &[ModelOption],
) -> Vec<ModelOption> {
    let mut options = Vec::new();
    let default_name = local_model_name(default_local).unwrap_or(DEFAULT_LOCAL_MODEL);
    options.push(ModelOption {
        id: local_model_id(default_name),
        label: default_name.to_string(),
        backend: ModelBackend::Local,
        installed: is_installed(installed_local, default_name),
        tier: ModelTier::Local,
    });
    for name in installed_local {
        if !is_installed(std::slice::from_ref(name), default_name) {
            options.push(ModelOption {
                id: local_model_id(name),
                label: name.clone(),
                backend: ModelBackend::Local,
                installed: true,
                tier: ModelTier::Local,
            });
        }
    }
    for (models, backend) in [
        (ROUTER_MODELS, ModelBackend::Router),
        (CLAUDE_MODELS, ModelBackend::Claude),
        (CHATGPT_MODELS, ModelBackend::Chatgpt),
        (OPENROUTER_MODELS, ModelBackend::OpenRouter),
    ] {
        let live: Vec<&ModelOption> = plan_models
            .iter()
            .filter(|m| m.backend == backend)
            .collect();
        if !live.is_empty() {
            options.extend(live.into_iter().cloned());
            continue;
        }
        for (id, label, tier) in models {
            options.push(ModelOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                backend,
                installed: true,
                tier: *tier,
            });
        }
    }
    options
}

/// Result of probing the local model server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalServerStatus {
    /// Server answered and supports `/v1/messages`.
    Ready {
        /// Reported server version.
        version: String,
        /// Installed model names (e.g. `qwen3:4b`).
        models: Vec<String>,
    },
    /// Server answered but is too old for the Anthropic-compatible API.
    Outdated {
        /// Reported server version.
        version: String,
    },
    /// Nothing reachable at the configured URL.
    Unreachable,
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagEntry>,
}

#[derive(Deserialize)]
struct TagEntry {
    name: String,
}

/// Probe an Ollama-compatible server at `base_url` via `/api/version` and `/api/tags`.
pub async fn probe_local_server(base_url: &str) -> LocalServerStatus {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return LocalServerStatus::Unreachable;
    };
    let base = base_url.trim_end_matches('/');

    let version = match client.get(format!("{base}/api/version")).send().await {
        Ok(resp) => match resp.json::<VersionResponse>().await {
            Ok(body) => body.version,
            Err(_) => return LocalServerStatus::Unreachable,
        },
        Err(_) => return LocalServerStatus::Unreachable,
    };
    if !version_supports_messages_api(&version) {
        return LocalServerStatus::Outdated { version };
    }

    let models = match client.get(format!("{base}/api/tags")).send().await {
        Ok(resp) => resp
            .json::<TagsResponse>()
            .await
            .map(|tags| tags.models.into_iter().map(|m| m.name).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    LocalServerStatus::Ready { version, models }
}

/// Unparseable versions (dev builds) are assumed to be recent.
fn version_supports_messages_api(version: &str) -> bool {
    let mut parts = version.trim().trim_start_matches('v').split(['.', '-']);
    let major = parts.next().and_then(|p| p.parse::<u32>().ok());
    let minor = parts.next().and_then(|p| p.parse::<u32>().ok());
    match (major, minor) {
        (Some(major), Some(minor)) => (major, minor) >= MIN_OLLAMA_VERSION,
        _ => true,
    }
}

/// Whether `installed` contains `name`, treating a missing tag as `:latest`.
#[must_use]
pub fn is_installed(installed: &[String], name: &str) -> bool {
    let with_latest = if name.contains(':') {
        None
    } else {
        Some(format!("{name}:latest"))
    };
    installed
        .iter()
        .any(|m| m == name || with_latest.as_deref() == Some(m.as_str()))
}

#[cfg(test)]
mod tests;
