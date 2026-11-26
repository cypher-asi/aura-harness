//! The local model server: its settings and probing it.

use std::time::Duration;

use serde::Deserialize;

use super::{DEFAULT_LOCAL_CONTEXT_TOKENS, DEFAULT_LOCAL_MODEL, DEFAULT_LOCAL_URL, LOCAL_PREFIX};

/// Minimum Ollama version exposing the Anthropic-compatible `/v1/messages`.
const MIN_OLLAMA_VERSION: (u32, u32) = (0, 14);

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
pub(super) fn version_supports_messages_api(version: &str) -> bool {
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
