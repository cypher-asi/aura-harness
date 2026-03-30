/// LLM routing mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingMode {
    /// Call the LLM provider directly (e.g., api.anthropic.com).
    Direct,
    /// Route through the aura-router proxy with JWT auth.
    Proxy,
}

/// Anthropic provider configuration.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// API key
    pub api_key: String,
    /// Default model to use
    pub default_model: String,
    /// Request timeout in milliseconds
    pub timeout_ms: u64,
    /// Maximum retries per model before falling back.
    pub max_retries: u32,
    /// API base URL
    pub base_url: String,
    pub routing_mode: RoutingMode,
    /// Optional fallback model when the primary is overloaded (429/529).
    pub fallback_model: Option<String>,
}

impl AnthropicConfig {
    /// Create a new config from environment variables.
    ///
    /// Reads:
    /// - `AURA_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY`
    /// - `AURA_ANTHROPIC_MODEL` (defaults to "claude-opus-4-6")
    ///
    /// # Errors
    ///
    /// Returns error if API key is not set.
    ///
    /// NOTE: This method embeds Aura-specific environment variable names
    /// (AURA_ROUTER_URL, AURA_LLM_ROUTING). Consider accepting these as
    /// parameters or moving deployment config to the caller.
    pub fn from_env() -> anyhow::Result<Self> {
        let routing_mode = match std::env::var("AURA_LLM_ROUTING").as_deref() {
            Ok("direct") => RoutingMode::Direct,
            _ => RoutingMode::Proxy,
        };

        let (api_key, base_url) = match routing_mode {
            RoutingMode::Direct => {
                let key = std::env::var("AURA_ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "Direct mode requires AURA_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY"
                        )
                    })?;
                let url = std::env::var("AURA_ANTHROPIC_BASE_URL")
                    .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
                (key, url)
            }
            RoutingMode::Proxy => {
                let url = std::env::var("AURA_ROUTER_URL")
                    .unwrap_or_else(|_| "https://aura-router.onrender.com".to_string());
                (String::new(), url)
            }
        };

        let default_model = std::env::var("AURA_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| aura_core::DEFAULT_MODEL.to_string());

        let timeout_ms = std::env::var("AURA_MODEL_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300_000);

        let fallback_model = std::env::var("AURA_ANTHROPIC_FALLBACK_MODEL")
            .ok()
            .filter(|s| !s.is_empty());

        Ok(Self {
            api_key,
            default_model,
            timeout_ms,
            max_retries: 2,
            base_url,
            routing_mode,
            fallback_model,
        })
    }

    /// Create a config with explicit values.
    #[must_use]
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            default_model: model.into(),
            timeout_ms: 300_000,
            max_retries: 2,
            base_url: "https://api.anthropic.com".to_string(),
            routing_mode: RoutingMode::Direct,
            fallback_model: None,
        }
    }
}
