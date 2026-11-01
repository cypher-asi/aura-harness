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
//! There is no automatic fallback between routes.

use std::time::Duration;

use serde::Deserialize;

use aura_core_auth::ProviderKind;

/// Prefix that marks a model as served by the local endpoint.
pub const LOCAL_PREFIX: &str = "local/";

/// Prefix that marks a model as served directly with a Claude subscription.
pub const CLAUDE_PREFIX: &str = "claude/";

/// Prefix that marks a model as served directly with a ChatGPT subscription.
pub const CHATGPT_PREFIX: &str = "chatgpt/";

/// Lightweight local model used when nothing else is configured: small
/// enough for CPU-only laptops and supports native tool calling.
pub const DEFAULT_LOCAL_MODEL: &str = "qwen3:4b";

/// Default base URL of the local model server (Ollama).
pub const DEFAULT_LOCAL_URL: &str = "http://127.0.0.1:11434";

/// Context window assumed for local models unless overridden via
/// `AURA_LOCAL_CONTEXT_TOKENS`. Ollama must be started with a matching
/// `OLLAMA_CONTEXT_LENGTH`; its built-in default is much smaller.
pub const DEFAULT_LOCAL_CONTEXT_TOKENS: u64 = 32_768;

/// Output budget for local models; the 16K agent default would consume
/// half of a 32K local context window.
pub const DEFAULT_LOCAL_MAX_OUTPUT_TOKENS: u32 = 4_096;

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
const ROUTER_MODELS: &[(&str, &str)] = &[
    ("aura-claude-opus-4-7", "Claude Opus 4.7"),
    ("aura-claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("aura-claude-haiku-4-5", "Claude Haiku 4.5"),
    ("aura-gpt-5-4", "GPT-5.4"),
];

/// Curated models offered with a linked Claude plan.
const CLAUDE_MODELS: &[(&str, &str)] = &[
    ("claude/claude-opus-4-7", "Claude Opus 4.7"),
    ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("claude/claude-haiku-4-5", "Claude Haiku 4.5"),
];

/// Curated models offered with a linked ChatGPT plan.
const CHATGPT_MODELS: &[(&str, &str)] = &[("chatgpt/gpt-5.4", "GPT-5.4")];

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
    let mut options = Vec::new();
    let default_name = local_model_name(default_local).unwrap_or(DEFAULT_LOCAL_MODEL);
    options.push(ModelOption {
        id: local_model_id(default_name),
        label: default_name.to_string(),
        backend: ModelBackend::Local,
        installed: is_installed(installed_local, default_name),
    });
    for name in installed_local {
        if !is_installed(std::slice::from_ref(name), default_name) {
            options.push(ModelOption {
                id: local_model_id(name),
                label: name.clone(),
                backend: ModelBackend::Local,
                installed: true,
            });
        }
    }
    for (models, backend) in [
        (ROUTER_MODELS, ModelBackend::Router),
        (CLAUDE_MODELS, ModelBackend::Claude),
        (CHATGPT_MODELS, ModelBackend::Chatgpt),
    ] {
        for (id, label) in models {
            options.push(ModelOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                backend,
                installed: true,
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
