//! Provider factory for Aura.
//!
//! There is exactly one real LLM provider: an Anthropic-shaped HTTP client
//! that talks to `aura-router` (the Aura proxy) with a per-request JWT.
//! For tests, [`mock_provider`] returns a fixture-friendly `MockProvider`.
//!
//! Per-session overrides — model, fallback model, prompt-caching toggle —
//! arrive on the wire as `aura_protocol::SessionModelOverrides` and are
//! converted by callers into [`SessionOverrides`] before invoking
//! [`with_session_overrides`].

use std::sync::Arc;

use tracing::{info, warn};

use crate::anthropic::{AnthropicConfig, AnthropicProvider};
use crate::error::ReasonerError;
use crate::mock::MockProvider;
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
}

/// Build the default router-backed provider from environment variables.
///
/// Wraps [`AnthropicConfig::from_env`] + [`AnthropicProvider::new`]. On
/// the rare HTTP-client construction failure (e.g. invalid TLS
/// configuration), logs a warning and substitutes a mock so callers can
/// still boot — this preserves the historical "no secrets needed" UX in
/// CI / integration test entrypoints.
#[must_use]
pub fn default_provider() -> ProviderSelection {
    let cfg = AnthropicConfig::from_env();
    info!(
        base_url = %cfg.base_url,
        default_model = %cfg.default_model,
        "LLM provider ready (router-backed proxy)"
    );
    match AnthropicProvider::new(cfg) {
        Ok(provider) => ProviderSelection {
            provider: Arc::new(provider),
            name: "anthropic".to_string(),
        },
        Err(e) => {
            warn!(error = %e, "LLM provider build failed, using mock");
            ProviderSelection {
                provider: Arc::new(MockProvider::simple_response(
                    "Mock provider (HTTP client init failed)",
                )),
                name: "mock (fallback)".to_string(),
            }
        }
    }
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

/// Build a mock-backed provider for tests and offline boot. Never fails.
#[must_use]
pub fn mock_provider() -> ProviderSelection {
    ProviderSelection {
        provider: Arc::new(MockProvider::simple_response(
            "Mock provider (tests only)",
        )),
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
    fn with_session_overrides_applies_default_model() {
        std::env::set_var("AURA_ROUTER_URL", "http://127.0.0.1:3999");
        let selection = with_session_overrides(SessionOverrides {
            default_model: Some("aura-claude-sonnet-4-6".to_string()),
            fallback_model: None,
            prompt_caching_enabled: Some(true),
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
        })
        .expect("with_session_overrides");
        assert_eq!(selection.provider.name(), "anthropic");
    }
}
