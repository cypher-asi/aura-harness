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
//! - anything else: an aura-router alias that needs a zOS JWT from `/login`.
//!
//! [`GATE_MODEL_ID`] is not a model: the surface resolves it to one of
//! these before a request is built.

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
#[must_use]
pub fn max_output_tokens(model: &str) -> u32 {
    if backend_for(model) == ModelBackend::Local {
        return DEFAULT_LOCAL_MAX_OUTPUT_TOKENS;
    }
    let key = model_key(model);
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

/// Backend serving `model`.
#[must_use]
pub fn backend_for(model: &str) -> ModelBackend {
    if model.starts_with(LOCAL_PREFIX) {
        ModelBackend::Local
    } else if model.starts_with(CLAUDE_PREFIX) {
        ModelBackend::Claude
    } else if model.starts_with(CHATGPT_PREFIX) {
        ModelBackend::Chatgpt
    } else {
        ModelBackend::Router
    }
}

/// Model name as the serving backend knows it: strips the `local/`,
/// `claude/` or `chatgpt/` route prefix; router aliases are unchanged.
#[must_use]
pub fn upstream_model_name(model: &str) -> &str {
    [LOCAL_PREFIX, CLAUDE_PREFIX, CHATGPT_PREFIX]
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
/// `chatgpt/gpt-5.4` -> `gpt-5-4`).
#[must_use]
pub fn model_key(model: &str) -> String {
    let name = upstream_model_name(model);
    let name = name.strip_prefix("aura-").unwrap_or(name);
    name.to_ascii_lowercase().replace('.', "-")
}

/// Whether changing the thinking effort between two requests of one
/// conversation makes `model`'s provider write the conversation to its
/// prompt cache again. Claude models do: only the tools and the system
/// prompt are still read from the cache after a change.
#[must_use]
pub fn effort_change_resets_prompt_cache(model: &str) -> bool {
    model_key(model).starts_with("claude-")
}

/// Whether an agent's turns on `model` reason at the highest effort the
/// model has when nobody chose one: Claude Sonnet 5.5 and the Claude
/// Opus 5 models. They get the work that is hard to get right, where an
/// attempt that fails costs more than the extra reasoning does.
#[must_use]
pub fn reasons_at_max(model: &str) -> bool {
    let key = model_key(model);
    is_snapshot_of(&key, "claude-sonnet-5-5")
        || is_snapshot_of(&key, "claude-opus-5")
        || is_snapshot_of(&key, "claude-opus-5-5")
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

fn list_price(model: &str) -> Option<Price> {
    if backend_for(model) == ModelBackend::Local {
        return None;
    }
    let key = model_key(model);
    PRICES
        .iter()
        .find(|(name, _)| is_snapshot_of(&key, name))
        .map(|(_, price)| *price)
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
    // responses report them on top of it.
    let openai =
        backend_for(model) == ModelBackend::Chatgpt || model_key(model).starts_with("gpt-");
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

/// [`catalog`], with each plan's curated list replaced by `plan_models`
/// (fetched live from the provider) for every plan backend it covers.
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
mod tests {
    use super::*;

    #[test]
    fn backend_follows_prefix() {
        assert_eq!(backend_for("local/qwen3:4b"), ModelBackend::Local);
        assert_eq!(backend_for("aura-claude-sonnet-4-6"), ModelBackend::Router);
        assert_eq!(backend_for("qwen3:4b"), ModelBackend::Router);
        assert_eq!(backend_for("claude/claude-opus-4-7"), ModelBackend::Claude);
        assert_eq!(backend_for("chatgpt/gpt-5.4"), ModelBackend::Chatgpt);
        assert_eq!(
            ModelBackend::Router.required_provider(),
            Some(ProviderKind::Zos)
        );
        assert_eq!(ModelBackend::Local.required_provider(), None);
    }

    #[test]
    fn live_plan_models_replace_the_built_in_list() {
        let live = [ModelOption {
            id: "claude/claude-opus-5-5".to_string(),
            label: "Claude Opus 5.5".to_string(),
            backend: ModelBackend::Claude,
            installed: true,
            tier: ModelTier::Frontier,
        }];
        let options = catalog_with_plans("local/qwen3:4b", &[], &live);
        let claude: Vec<&str> = options
            .iter()
            .filter(|o| o.backend == ModelBackend::Claude)
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(claude, ["claude/claude-opus-5-5"]);
        assert!(options.iter().any(|o| o.backend == ModelBackend::Router));
        assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
    }

    #[test]
    fn tiers_are_inferred_from_the_model_name() {
        assert_eq!(infer_tier("local/qwen3:4b"), ModelTier::Local);
        assert_eq!(infer_tier("claude/claude-haiku-5-5"), ModelTier::Fast);
        assert_eq!(infer_tier("chatgpt/gpt-5.4-mini"), ModelTier::Fast);
        assert_eq!(infer_tier("claude/claude-sonnet-5-5"), ModelTier::Standard);
        assert_eq!(infer_tier("claude/claude-opus-5-5"), ModelTier::Frontier);
        assert_eq!(infer_tier("chatgpt/gpt-5.6"), ModelTier::Frontier);
        assert_eq!(infer_tier("aura-gpt-5-4"), ModelTier::Frontier);
        assert_eq!(infer_tier("some-new-model"), ModelTier::Standard);
        assert!(ModelTier::Local < ModelTier::Fast && ModelTier::Standard < ModelTier::Frontier);
    }

    #[test]
    fn curated_tiers_match_inference() {
        for option in catalog("local/qwen3:4b", &[]) {
            assert_eq!(option.tier, infer_tier(&option.id), "{}", option.id);
        }
    }

    #[test]
    fn model_key_is_the_same_on_every_route() {
        assert_eq!(model_key("aura-claude-opus-4-7"), "claude-opus-4-7");
        assert_eq!(model_key("claude/claude-opus-4-7"), "claude-opus-4-7");
        assert_eq!(model_key("chatgpt/gpt-5.4"), model_key("aura-gpt-5-4"));
    }

    #[test]
    fn replies_may_be_as_long_as_each_api_allows() {
        for (model, limit) in [
            ("aura-claude-opus-5-5", 128_000),
            ("claude/claude-sonnet-4-6", 128_000),
            ("aura-claude-haiku-4-5", 64_000),
            ("claude-sonnet-4-20250514", 64_000),
            ("claude-opus-4-1", 32_000),
            ("chatgpt/gpt-5.4", 128_000),
            ("aura-gpt-4o", 16_384),
            ("aura-gemini-3-pro", 65_536),
            ("aura-kimi-k2", CONTEXT_BOUND_OUTPUT_TOKENS),
            ("some-new-model", UNKNOWN_OUTPUT_TOKENS),
            ("local/qwen3:4b", DEFAULT_LOCAL_MAX_OUTPUT_TOKENS),
        ] {
            assert_eq!(max_output_tokens(model), limit, "{model}");
        }
    }

    #[test]
    fn upstream_name_strips_route_prefix() {
        assert_eq!(
            upstream_model_name("claude/claude-opus-4-7"),
            "claude-opus-4-7"
        );
        assert_eq!(upstream_model_name("chatgpt/gpt-5.4"), "gpt-5.4");
        assert_eq!(upstream_model_name("local/qwen3:4b"), "qwen3:4b");
        assert_eq!(upstream_model_name("aura-gpt-5-4"), "aura-gpt-5-4");
        assert_eq!(upstream_model_name("claude/"), "claude/");
    }

    #[test]
    fn catalog_offers_both_routes_for_claude() {
        let options = catalog("local/qwen3:4b", &[]);
        assert!(options
            .iter()
            .any(|o| o.id == "aura-claude-opus-4-7" && o.backend == ModelBackend::Router));
        assert!(options
            .iter()
            .any(|o| o.id == "claude/claude-opus-4-7" && o.backend == ModelBackend::Claude));
        assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
    }

    fn usage(input: u64, cache_write: u64, cache_read: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: Some(cache_write),
            cache_read_input_tokens: Some(cache_read),
        }
    }

    fn assert_cost(model: &str, usage: &Usage, expected: f64) {
        let cost = estimate_cost_usd(model, usage).unwrap_or_else(|| panic!("no price: {model}"));
        assert!(
            (cost - expected).abs() < 1e-9,
            "{model}: {cost} != {expected}"
        );
    }

    #[test]
    fn cost_is_priced_the_same_on_every_route() {
        let usage = usage(1_000, 2_000, 1_000_000, 1_000);
        for model in [
            "claude-opus-4-7",
            "claude/claude-opus-4-7",
            "aura-claude-opus-4-7",
        ] {
            assert_cost(model, &usage, 0.5425);
            assert_eq!(call_tokens(model, &usage), 1_004_000);
        }
    }

    #[test]
    fn openai_cached_tokens_are_part_of_the_input() {
        let usage = usage(10_000, 0, 8_000, 1_000);
        for model in ["gpt-5.4", "chatgpt/gpt-5.4", "aura-gpt-5-4"] {
            assert_cost(model, &usage, 0.022);
            assert_eq!(call_tokens(model, &usage), 11_000);
        }
    }

    #[test]
    fn dated_snapshots_share_their_model_price() {
        let usage = usage(1_000_000, 0, 0, 0);
        assert_cost("claude-haiku-4-5-20251001", &usage, 1.0);
        assert_cost("claude-opus-4-20250514", &usage, 15.0);
        assert_cost("gpt-5.4-2026-03-05", &usage, 2.5);
        assert_cost("claude-opus-5-5", &usage, 4.0);
    }

    #[test]
    fn sonnet_5_5_and_opus_5_reason_at_max() {
        for model in [
            "claude/claude-sonnet-5-5",
            "claude-sonnet-5-5-20261001",
            "aura-claude-opus-5-5",
            "claude/claude-opus-5",
        ] {
            assert!(reasons_at_max(model), "{model}");
        }
        for model in [
            "claude/claude-sonnet-5",
            "claude/claude-haiku-5-5",
            "claude/claude-opus-4-8",
            "claude/claude-fable-5-2",
            "chatgpt/gpt-5.6-sol",
            "ollama/qwen3",
        ] {
            assert!(!reasons_at_max(model), "{model}");
        }
    }

    #[test]
    fn a_later_version_that_costs_no_more_supersedes_the_earlier() {
        // Same family, later version, no token dearer: on any route.
        assert!(supersedes("claude-sonnet-5-5", "claude-sonnet-5"));
        assert!(supersedes(
            "claude/claude-sonnet-5-5",
            "claude/claude-sonnet-4-6"
        ));
        assert!(supersedes("claude-opus-5-5", "claude-opus-4-8"));
        assert!(supersedes("claude-opus-4-8", "claude-opus-4-7"));
        assert!(supersedes("claude-haiku-5-5", "claude-3-5-haiku"));
        assert!(supersedes("gpt-6-sol", "gpt-5.6-sol"));
        // Not the other way, nor a model itself or its dated snapshot.
        assert!(!supersedes("claude-sonnet-5", "claude-sonnet-5-5"));
        assert!(!supersedes("claude-sonnet-5-5", "claude-sonnet-5-5"));
        assert!(!supersedes("claude-haiku-4-5-20251001", "claude-haiku-4-5"));
        // A later version that costs more is a different trade.
        assert!(!supersedes("gpt-5.5", "gpt-5.4"));
        assert!(!supersedes("claude-haiku-4-5", "claude-3-5-haiku"));
        // Another family, whatever it costs.
        assert!(!supersedes("claude-haiku-5-5", "claude-sonnet-5"));
        assert!(!supersedes("gpt-5.4-mini", "gpt-5.2"));
        // Nothing is known of an unpriced or local model.
        assert!(!supersedes("claude-fable-5-2", "claude-fable-5-1"));
        assert!(!supersedes("local/qwen3:8b", "local/qwen3:4b"));
    }

    #[test]
    fn local_and_unknown_models_have_no_cost() {
        let usage = usage(1_000, 0, 0, 1_000);
        assert_eq!(estimate_cost_usd("local/qwen3:4b", &usage), None);
        assert_eq!(estimate_cost_usd("qwen3:4b", &usage), None);
        assert_eq!(estimate_cost_usd("gpt-5-3", &usage), None);
        assert_eq!(estimate_cost_usd("claude-opus-9", &usage), None);
        assert_eq!(call_tokens("qwen3:4b", &usage), 2_000);
    }

    #[test]
    fn local_model_name_strips_prefix() {
        assert_eq!(local_model_name("local/qwen3:4b"), Some("qwen3:4b"));
        assert_eq!(local_model_name("local/"), None);
        assert_eq!(local_model_name("claude-opus-4-7"), None);
    }

    #[test]
    fn catalog_lists_default_local_first_and_dedupes() {
        let installed = vec!["qwen3:4b".to_string(), "llama3.2:3b".to_string()];
        let options = catalog("local/qwen3:4b", &installed);
        assert_eq!(options[0].id, "local/qwen3:4b");
        assert!(options[0].installed);
        assert_eq!(options[1].id, "local/llama3.2:3b");
        assert_eq!(
            options
                .iter()
                .filter(|o| o.backend == ModelBackend::Local)
                .count(),
            2
        );
        assert!(options
            .iter()
            .filter(|o| o.backend == ModelBackend::Router)
            .all(ModelOption::requires_login));
    }

    #[test]
    fn catalog_marks_missing_default_as_not_installed() {
        let options = catalog("local/qwen3:4b", &[]);
        assert!(!options[0].installed);
    }

    #[test]
    fn version_gate() {
        assert!(version_supports_messages_api("0.14.0"));
        assert!(version_supports_messages_api("0.15.2"));
        assert!(version_supports_messages_api("1.0.0"));
        assert!(!version_supports_messages_api("0.13.5"));
        assert!(version_supports_messages_api("dev"));
    }

    #[test]
    fn installed_matches_latest_tag() {
        let installed = vec!["llama3.2:latest".to_string()];
        assert!(is_installed(&installed, "llama3.2"));
        assert!(!is_installed(&installed, "qwen3:4b"));
    }

    #[tokio::test]
    async fn probe_unreachable_server() {
        let status = probe_local_server("http://127.0.0.1:9").await;
        assert_eq!(status, LocalServerStatus::Unreachable);
    }
}
