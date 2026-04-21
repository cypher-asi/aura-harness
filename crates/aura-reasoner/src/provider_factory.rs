//! Unified provider factory for Aura.
//!
//! Historically two parallel factories existed:
//! - `aura_agent::session_bootstrap::select_provider(name)` — used from the
//!   root binary to pick a provider by short name (`"anthropic"`, `"mock"`).
//! - `aura_node::provider_factory` — used from the WebSocket session handler
//!   to build a provider from a session-scoped configuration.
//!
//! Both ultimately constructed a [`Box<dyn ModelProvider>`]. Wave 4
//! collapses them into this single module. Callers pick the constructor
//! that matches their input shape:
//!
//! - [`from_name`] — for simple name-based selection (root bin).
//! - [`from_provider_config`] — for session-scoped configs (node). The node
//!   crate converts its wire-level `SessionProviderConfig` into the
//!   reasoner-owned [`ProviderConfig`] at the boundary to avoid pulling a
//!   cross-tree protocol dependency into `aura-reasoner`.
//!
//! # Fallback policy
//!
//! `from_name("anthropic")` attempts to read [`AnthropicConfig::from_env`]
//! and, if it succeeds, builds an Anthropic provider. Historically the
//! agent-side helper silently fell back to `MockProvider` on any error; the
//! unified factory preserves that fallback so existing `aura` headless
//! flows still boot in CI without secrets, but it emits a `warn!` so the
//! behaviour is observable. Callers that need hard failure semantics
//! should call `AnthropicProvider::from_env` directly.

use std::sync::Arc;

use tracing::{info, warn};

use crate::anthropic::{AnthropicConfig, AnthropicProvider, RoutingMode};
use crate::error::ReasonerError;
use crate::mock::MockProvider;
use crate::ModelProvider;

/// Result of a provider selection.
///
/// Holds the constructed provider along with a short name suitable for
/// logging or status display. `name` distinguishes the primary request
/// from any fallback (e.g. `"mock (fallback)"`).
pub struct ProviderSelection {
    /// The shared, thread-safe provider instance.
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Human-readable provider name used for logging / status display.
    pub name: String,
}

/// Logical specification of which provider to build.
#[derive(Debug, Clone)]
pub enum ProviderSpec {
    /// Mock provider (no external calls).
    Mock,
    /// Anthropic provider with a specific configuration.
    Anthropic(AnthropicConfig),
}

/// Reasoner-owned session-scoped provider configuration.
///
/// This mirrors the fields carried on the wire-level
/// `aura_protocol::SessionProviderConfig` but lives inside the reasoner so
/// `aura-reasoner` does not take a cross-tree dependency on the protocol
/// crate. Call sites (e.g. `aura-node`) convert the protocol DTO to this
/// struct at the boundary.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider short name. Currently only `"anthropic"` is supported for
    /// per-session configs; `"mock"` is constructed via [`from_name`].
    pub provider: String,
    /// Optional routing mode override (`"direct"` or `"proxy"`). Defaults
    /// to `Direct` when absent.
    pub routing_mode: Option<String>,
    /// Optional API key (required for `direct` mode).
    pub api_key: Option<String>,
    /// Optional base URL override. Defaults to the Anthropic API for
    /// direct mode and the env-configured router URL for proxy mode.
    pub base_url: Option<String>,
    /// Optional default model name.
    pub default_model: Option<String>,
    /// Optional fallback model for 429/529 retries.
    pub fallback_model: Option<String>,
    /// Whether Anthropic prompt-caching directives should be attached.
    /// Defaults to `true` when absent.
    pub prompt_caching_enabled: Option<bool>,
}

fn mock_provider(reason: &'static str) -> Arc<dyn ModelProvider + Send + Sync> {
    Arc::new(MockProvider::simple_response(reason))
}

fn proxy_base_url() -> String {
    std::env::var("AURA_ROUTER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://aura-router.onrender.com".to_string())
}

const MOCK_BANNER: &str =
    "Mock mode: Set AURA_LLM_ROUTING and required credentials to enable real AI responses.";

/// Select a provider by short name.
///
/// Supported names:
/// - `"mock"` — always returns a [`MockProvider`].
/// - `"anthropic"` — attempts to build from environment variables; on
///   failure logs a warning and falls back to a mock.
/// - anything else — returns a `ReasonerError::Internal` describing the
///   unknown name. This replaces the older behaviour where any unknown
///   name silently fell through to Anthropic / mock.
///
/// # Errors
///
/// Returns [`ReasonerError::Internal`] for unknown provider names. The
/// Anthropic branch never errors; it emits a warning and returns a mock on
/// env-config failure to preserve the previous boot-without-secrets UX.
pub fn from_name(name: &str) -> Result<ProviderSelection, ReasonerError> {
    match name {
        "mock" => Ok(ProviderSelection {
            provider: mock_provider(MOCK_BANNER),
            name: "mock".to_string(),
        }),
        "anthropic" => Ok(build_anthropic_from_env()),
        other => Err(ReasonerError::Internal(format!(
            "unknown provider `{other}` (expected `anthropic` or `mock`)"
        ))),
    }
}

fn build_anthropic_from_env() -> ProviderSelection {
    match AnthropicConfig::from_env() {
        Ok(cfg) => match AnthropicProvider::new(cfg) {
            Ok(provider) => ProviderSelection {
                provider: Arc::new(provider),
                name: "anthropic".to_string(),
            },
            Err(e) => {
                warn!(error = %e, "LLM provider build failed, using mock");
                ProviderSelection {
                    provider: mock_provider(MOCK_BANNER),
                    name: "mock (fallback)".to_string(),
                }
            }
        },
        Err(e) => {
            warn!(error = %e, "LLM provider not configured, using mock");
            ProviderSelection {
                provider: mock_provider(MOCK_BANNER),
                name: "mock (fallback)".to_string(),
            }
        }
    }
}

/// Build a default provider from environment variables.
///
/// Equivalent to `from_name("anthropic")` but never fails: a mock is
/// substituted on any error. Node startup uses this when no per-session
/// override has been provided yet.
#[must_use]
pub fn default_from_env() -> ProviderSelection {
    build_anthropic_from_env()
}

/// Build a provider from an explicit [`ProviderSpec`].
///
/// # Errors
///
/// Returns [`ReasonerError`] if the Anthropic config is invalid (e.g.
/// direct-mode API key missing).
pub fn from_spec(spec: ProviderSpec) -> Result<ProviderSelection, ReasonerError> {
    match spec {
        ProviderSpec::Mock => Ok(ProviderSelection {
            provider: mock_provider(MOCK_BANNER),
            name: "mock".to_string(),
        }),
        ProviderSpec::Anthropic(cfg) => {
            let mode_label = if cfg.routing_mode == RoutingMode::Proxy {
                "proxy"
            } else {
                "direct"
            };
            let provider = AnthropicProvider::new(cfg).map_err(|e| {
                ReasonerError::Internal(format!("creating anthropic provider: {e}"))
            })?;
            info!(mode = mode_label, "LLM provider ready ({mode_label} mode)");
            Ok(ProviderSelection {
                provider: Arc::new(provider),
                name: "anthropic".to_string(),
            })
        }
    }
}

/// Build a provider from a session-scoped [`ProviderConfig`].
///
/// The node crate converts its wire-level `SessionProviderConfig` into
/// this shape before calling into the factory.
///
/// # Errors
///
/// Returns [`ReasonerError::Internal`] if:
/// - `config.provider` is not `"anthropic"`.
/// - `direct` mode is requested without an API key.
/// - The underlying [`AnthropicProvider::new`] call fails.
pub fn from_provider_config(config: &ProviderConfig) -> Result<ProviderSelection, ReasonerError> {
    match config.provider.as_str() {
        "anthropic" => {
            let routing_mode = match config.routing_mode.as_deref() {
                Some("proxy") => RoutingMode::Proxy,
                _ => RoutingMode::Direct,
            };

            let anthropic_cfg = AnthropicConfig {
                api_key: config.api_key.clone().unwrap_or_default(),
                default_model: config
                    .default_model
                    .clone()
                    .unwrap_or_else(|| "claude-opus-4-6".to_string()),
                timeout_ms: 300_000,
                max_retries: 2,
                base_url: config
                    .base_url
                    .clone()
                    .unwrap_or_else(|| match routing_mode {
                        RoutingMode::Direct => "https://api.anthropic.com".to_string(),
                        RoutingMode::Proxy => proxy_base_url(),
                    }),
                routing_mode,
                fallback_model: config.fallback_model.clone(),
                prompt_caching_enabled: config.prompt_caching_enabled.unwrap_or(true),
            };

            if anthropic_cfg.routing_mode == RoutingMode::Direct && anthropic_cfg.api_key.is_empty()
            {
                return Err(ReasonerError::Internal(
                    "anthropic direct mode requires an API key".to_string(),
                ));
            }

            from_spec(ProviderSpec::Anthropic(anthropic_cfg))
        }
        other => Err(ReasonerError::Internal(format!(
            "unsupported session provider `{other}`"
        ))),
    }
}

/// Test-only helper that returns a mock-backed [`ProviderSelection`].
#[cfg(test)]
#[must_use]
pub fn mock_selection() -> ProviderSelection {
    ProviderSelection {
        provider: mock_provider(MOCK_BANNER),
        name: "mock".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_mock_returns_mock_provider() {
        let selection = from_name("mock").expect("mock must succeed");
        assert_eq!(selection.name, "mock");
        assert_eq!(selection.provider.name(), "mock");
    }

    #[test]
    fn from_name_unknown_provider_errors() {
        match from_name("gpt-but-not-really") {
            Ok(_) => panic!("unknown names must error"),
            Err(ReasonerError::Internal(m)) => assert!(m.contains("unknown provider")),
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn from_provider_config_direct_without_api_key_errors() {
        let cfg = ProviderConfig {
            provider: "anthropic".to_string(),
            routing_mode: Some("direct".to_string()),
            api_key: None,
            base_url: None,
            default_model: None,
            fallback_model: None,
            prompt_caching_enabled: None,
        };
        match from_provider_config(&cfg) {
            Ok(_) => panic!("direct without key must fail"),
            Err(ReasonerError::Internal(m)) => assert!(m.contains("API key")),
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn from_provider_config_proxy_builds_without_api_key() {
        // ENV test: set router URL so `proxy_base_url` is deterministic.
        std::env::set_var("AURA_ROUTER_URL", "http://127.0.0.1:3999");
        let cfg = ProviderConfig {
            provider: "anthropic".to_string(),
            routing_mode: Some("proxy".to_string()),
            api_key: None,
            base_url: None,
            default_model: Some("aura-claude-sonnet-4-6".to_string()),
            fallback_model: None,
            prompt_caching_enabled: Some(true),
        };
        let selection = from_provider_config(&cfg).expect("proxy build");
        assert_eq!(selection.name, "anthropic");
        assert_eq!(selection.provider.name(), "anthropic");
    }

    #[test]
    fn from_provider_config_unsupported_provider_errors() {
        let cfg = ProviderConfig {
            provider: "definitely-not-real".to_string(),
            routing_mode: None,
            api_key: None,
            base_url: None,
            default_model: None,
            fallback_model: None,
            prompt_caching_enabled: None,
        };
        match from_provider_config(&cfg) {
            Ok(_) => panic!("unknown provider must fail"),
            Err(ReasonerError::Internal(m)) => {
                assert!(m.contains("unsupported session provider"));
            }
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn mock_selection_helper_returns_mock() {
        let selection = mock_selection();
        assert_eq!(selection.name, "mock");
        assert_eq!(selection.provider.name(), "mock");
    }
}
