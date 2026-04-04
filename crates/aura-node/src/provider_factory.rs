use std::sync::Arc;

use aura_protocol::SessionProviderConfig;
use aura_reasoner::{AnthropicConfig, AnthropicProvider, MockProvider, ModelProvider, RoutingMode};
use tracing::{info, warn};

pub(crate) fn create_default_model_provider() -> Arc<dyn ModelProvider + Send + Sync> {
    match AnthropicConfig::from_env() {
        Ok(config) => create_provider_from_anthropic_config(config),
        Err(e) => {
            warn!(error = %e, "LLM provider not configured, using mock");
            Arc::new(MockProvider::simple_response("(mock provider)"))
        }
    }
}

pub(crate) fn create_provider_from_session_config(
    config: &SessionProviderConfig,
) -> anyhow::Result<Arc<dyn ModelProvider + Send + Sync>> {
    match config.provider.as_str() {
        "anthropic" => {
            let routing_mode = match config.routing_mode.as_deref() {
                Some("proxy") => RoutingMode::Proxy,
                _ => RoutingMode::Direct,
            };

            let provider_config = AnthropicConfig {
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
                        RoutingMode::Proxy => "https://aura-router.onrender.com".to_string(),
                    }),
                routing_mode,
                fallback_model: config.fallback_model.clone(),
                prompt_caching_enabled: config.prompt_caching_enabled.unwrap_or(true),
            };

            if provider_config.routing_mode == RoutingMode::Direct
                && provider_config.api_key.is_empty()
            {
                anyhow::bail!("anthropic direct mode requires an API key");
            }

            AnthropicProvider::new(provider_config)
                .map(|provider| Arc::new(provider) as Arc<dyn ModelProvider + Send + Sync>)
                .map_err(|e| anyhow::anyhow!("creating anthropic session provider: {e}"))
        }
        other => anyhow::bail!("unsupported session provider `{other}`"),
    }
}

fn create_provider_from_anthropic_config(
    config: AnthropicConfig,
) -> Arc<dyn ModelProvider + Send + Sync> {
    let mode_label = if config.routing_mode == RoutingMode::Proxy {
        "proxy"
    } else {
        "direct"
    };
    match AnthropicProvider::new(config) {
        Ok(provider) => {
            info!(mode = mode_label, "LLM provider ready ({mode_label} mode)");
            Arc::new(provider)
        }
        Err(e) => {
            warn!(error = %e, "Failed to create LLM provider, using mock");
            Arc::new(MockProvider::simple_response("(mock provider)"))
        }
    }
}
