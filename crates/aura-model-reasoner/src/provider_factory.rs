//! Provider factory for Aura.
//!
//! The main wire client is Anthropic-shaped. It talks to `aura-router`
//! (the Aura proxy) with a per-request JWT and, for the terminal UI only
//! via [`routed_provider`], to a local Anthropic-compatible server
//! (Ollama), to the Anthropic API with a Claude plan, and — through the
//! Responses client — to OpenAI with a ChatGPT plan. For tests, [`mock_provider`]
//! returns a fixture-friendly `MockProvider`.
//!
//! Per-session overrides — model, fallback model, prompt-caching toggle —
//! arrive on the wire as `aura_protocol::SessionModelOverrides` and are
//! converted by callers into [`SessionOverrides`] before invoking
//! [`with_session_overrides`].

use std::sync::Arc;

use aura_core_auth::{CredentialResolver, ProviderKind};
use tracing::info;

use crate::anthropic::{AnthropicConfig, AnthropicProvider};
use crate::credentialed::CredentialedProvider;
use crate::error::ReasonerError;
use crate::mock::MockProvider;
use crate::openai_responses::OpenAiResponsesProvider;
use crate::routed::RoutedProvider;
use crate::ModelProvider;

/// Result of a provider selection.
pub struct ProviderSelection {
    /// Shared, thread-safe provider instance.
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Human-readable provider name used for logging / status display.
    pub name: String,
}

/// Per-session overrides extracted from `SessionModelOverrides` on the wire.
///
/// These three values are the only knobs that still mean anything once
/// the LLM path is collapsed to "always proxy through aura-router with
/// JWT". Everything else is server-side env config or a per-request
/// header.
#[derive(Debug, Clone, Default)]
pub struct SessionOverrides {
    pub default_model: Option<String>,
    pub fallback_model: Option<String>,
    pub prompt_caching_enabled: Option<bool>,
    /// Stable cache key forwarded to OpenAI-family upstreams as
    /// `prompt_cache_key`. See `aura_model_reasoner::ModelRequest::prompt_cache_key`.
    pub prompt_cache_key: Option<String>,
    /// Retention hint paired with `prompt_cache_key`. Wire values are
    /// `"in_memory"` or `"24h"`.
    pub prompt_cache_retention: Option<String>,
}

/// Build the default router-backed provider from environment variables.
///
/// Wraps [`AnthropicConfig::from_env`] + [`AnthropicProvider::new`].
///
/// # Errors
///
/// Returns [`ReasonerError`] if HTTP client construction fails.
pub fn default_provider() -> Result<ProviderSelection, ReasonerError> {
    let cfg = AnthropicConfig::from_env();
    info!(
        base_url = %cfg.base_url,
        default_model = %cfg.default_model,
        "LLM provider ready (router-backed proxy)"
    );
    let provider = AnthropicProvider::new(cfg)?;
    Ok(ProviderSelection {
        provider: Arc::new(provider),
        name: "anthropic".to_string(),
    })
}

/// Build a provider with per-session overrides applied to the env-default
/// config.
///
/// # Errors
///
/// Returns [`ReasonerError`] only if HTTP client construction fails.
pub fn with_session_overrides(
    overrides: SessionOverrides,
) -> Result<ProviderSelection, ReasonerError> {
    let mut cfg = AnthropicConfig::from_env();
    if let Some(model) = overrides.default_model.filter(|v| !v.trim().is_empty()) {
        cfg.default_model = model;
    }
    if let Some(fallback) = overrides
        .fallback_model
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        cfg.fallback_model = Some(fallback);
    }
    if let Some(caching) = overrides.prompt_caching_enabled {
        cfg.prompt_caching_enabled = caching;
    }
    info!(
        base_url = %cfg.base_url,
        default_model = %cfg.default_model,
        prompt_caching_enabled = cfg.prompt_caching_enabled,
        "LLM provider ready (session overrides applied)"
    );
    let provider = AnthropicProvider::new(cfg)?;
    Ok(ProviderSelection {
        provider: Arc::new(provider),
        name: "anthropic".to_string(),
    })
}

/// Build the terminal provider. `local/*` models go to the local
/// Anthropic-compatible server (Ollama), `claude/*` and `chatgpt/*` to
/// the user's linked plan via `subscriptions`, everything else to
/// aura-router. Without a resolver, plan models fail with an auth error.
///
/// # Errors
///
/// Returns [`ReasonerError`] if HTTP client construction fails.
pub fn routed_provider(
    subscriptions: Option<Arc<dyn CredentialResolver>>,
) -> Result<ProviderSelection, ReasonerError> {
    let local_cfg = AnthropicConfig::local_from_env();
    let router_cfg = AnthropicConfig::from_env();
    info!(
        local_url = %local_cfg.base_url,
        router_url = %router_cfg.base_url,
        plans = subscriptions.is_some(),
        "LLM provider ready (local + router-backed proxy)"
    );
    let local = AnthropicProvider::new(local_cfg)?.with_sse_for_all_models();
    let router = AnthropicProvider::new(router_cfg)?;
    let mut routed = RoutedProvider::new(Arc::new(local), Arc::new(router));
    if let Some(resolver) = subscriptions {
        let claude = AnthropicProvider::new(AnthropicConfig::anthropic_direct())?;
        let chatgpt = OpenAiResponsesProvider::from_env()?;
        routed = routed
            .with_claude(Arc::new(CredentialedProvider::new(
                ProviderKind::Claude,
                Arc::new(claude),
                resolver.clone(),
            )))
            .with_chatgpt(Arc::new(CredentialedProvider::new(
                ProviderKind::Chatgpt,
                Arc::new(chatgpt),
                resolver,
            )));
    }
    Ok(ProviderSelection {
        provider: Arc::new(routed),
        name: "aura".to_string(),
    })
}

/// Build a mock-backed provider for tests and offline boot. Never fails.
#[must_use]
pub fn mock_provider() -> ProviderSelection {
    ProviderSelection {
        provider: Arc::new(MockProvider::simple_response("Mock provider (tests only)")),
        name: "mock".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_provider_returns_mock() {
        let selection = mock_provider();
        assert_eq!(selection.name, "mock");
        assert_eq!(selection.provider.name(), "mock");
    }

    #[test]
    fn default_provider_returns_anthropic_or_error() {
        std::env::set_var("AURA_ROUTER_URL", "http://127.0.0.1:3999");
        let selection = default_provider().expect("default_provider");
        assert_eq!(selection.name, "anthropic");
        assert_eq!(selection.provider.name(), "anthropic");
    }

    #[test]
    fn with_session_overrides_applies_default_model() {
        std::env::set_var("AURA_ROUTER_URL", "http://127.0.0.1:3999");
        let selection = with_session_overrides(SessionOverrides {
            default_model: Some("aura-claude-sonnet-4-6".to_string()),
            fallback_model: None,
            prompt_caching_enabled: Some(true),
            prompt_cache_key: None,
            prompt_cache_retention: None,
        })
        .expect("with_session_overrides");
        assert_eq!(selection.provider.name(), "anthropic");
    }

    #[test]
    fn with_session_overrides_ignores_blank_strings() {
        std::env::set_var("AURA_ROUTER_URL", "http://127.0.0.1:3999");
        let selection = with_session_overrides(SessionOverrides {
            default_model: Some("   ".to_string()),
            fallback_model: Some("".to_string()),
            prompt_caching_enabled: None,
            prompt_cache_key: None,
            prompt_cache_retention: None,
        })
        .expect("with_session_overrides");
        assert_eq!(selection.provider.name(), "anthropic");
    }
}
