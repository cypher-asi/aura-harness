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
    ///
    /// Overridable via `AURA_LLM_MAX_RETRIES`. Default bumped to 8 to
    /// give the per-tool-call streaming retry loop
    /// (`aura_agent::agent_loop::streaming`) a meaningful budget when
    /// a 5xx hits mid-stream.
    pub max_retries: u32,
    /// Initial backoff before the first retry, in milliseconds.
    /// Doubled on each subsequent retry up to `backoff_cap_ms`.
    /// Overridable via `AURA_LLM_BACKOFF_INITIAL_MS`.
    pub backoff_initial_ms: u64,
    /// Maximum backoff between retries, in milliseconds. Overridable
    /// via `AURA_LLM_BACKOFF_CAP_MS`.
    pub backoff_cap_ms: u64,
    /// API base URL
    pub base_url: String,
    pub routing_mode: RoutingMode,
    /// Optional fallback model when the primary is overloaded (429/529).
    pub fallback_model: Option<String>,
    /// Whether Anthropic prompt-caching directives should be attached.
    pub prompt_caching_enabled: bool,
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
    /// (`AURA_ROUTER_URL`, `AURA_LLM_ROUTING`). Consider accepting these as
    /// parameters or moving deployment config to the caller.
    pub fn from_env() -> Result<Self, crate::ReasonerError> {
        let routing_mode = match std::env::var("AURA_LLM_ROUTING").as_deref() {
            Ok("direct") => RoutingMode::Direct,
            _ => RoutingMode::Proxy,
        };

        let (api_key, base_url) = match routing_mode {
            RoutingMode::Direct => {
                let key = std::env::var("AURA_ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .map_err(|_| {
                        crate::ReasonerError::Internal(
                            "Direct mode requires AURA_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY"
                                .into(),
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

        let default_model =
            std::env::var("AURA_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-4-6".to_string());

        let timeout_ms = std::env::var("AURA_MODEL_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300_000);

        let fallback_model = std::env::var("AURA_ANTHROPIC_FALLBACK_MODEL")
            .ok()
            .filter(|s| !s.is_empty());
        let prompt_caching_enabled = !matches!(
            std::env::var("AURA_DISABLE_PROMPT_CACHING").ok().as_deref(),
            Some("1" | "true" | "TRUE" | "yes" | "YES")
        );

        let max_retries: u32 = std::env::var("AURA_LLM_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let backoff_initial_ms: u64 = std::env::var("AURA_LLM_BACKOFF_INITIAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(250);
        let backoff_cap_ms: u64 = std::env::var("AURA_LLM_BACKOFF_CAP_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);

        Ok(Self {
            api_key,
            default_model,
            timeout_ms,
            max_retries,
            backoff_initial_ms,
            backoff_cap_ms,
            base_url,
            routing_mode,
            fallback_model,
            prompt_caching_enabled,
        })
    }

    /// Create a config with explicit values.
    #[must_use]
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            default_model: model.into(),
            timeout_ms: 300_000,
            max_retries: 8,
            backoff_initial_ms: 250,
            backoff_cap_ms: 30_000,
            base_url: "https://api.anthropic.com".to_string(),
            routing_mode: RoutingMode::Direct,
            fallback_model: None,
            prompt_caching_enabled: true,
        }
    }
}

#[cfg(test)]
mod env_backoff_tests {
    use super::*;

    /// Serializes env-var mutation so the tests in this module do not race
    /// each other (and do not race other `from_env` tests in the crate).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII helper that sets an env var for the life of the test and
    /// restores the previous value on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn with_env<F: FnOnce() -> AnthropicConfig>(f: F) -> AnthropicConfig {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        f()
    }

    #[test]
    fn backoff_fields_default_when_env_unset() {
        let cfg = with_env(|| {
            let _g1 = EnvGuard::unset("AURA_LLM_MAX_RETRIES");
            let _g2 = EnvGuard::unset("AURA_LLM_BACKOFF_INITIAL_MS");
            let _g3 = EnvGuard::unset("AURA_LLM_BACKOFF_CAP_MS");
            // `from_env` needs a routing mode + key; force Direct with a
            // dummy key so we don't wander through the proxy branch.
            let _g4 = EnvGuard::set("AURA_LLM_ROUTING", "direct");
            let _g5 = EnvGuard::set("AURA_ANTHROPIC_API_KEY", "sk-test");
            AnthropicConfig::from_env().expect("from_env")
        });
        assert_eq!(cfg.max_retries, 8, "default max_retries");
        assert_eq!(cfg.backoff_initial_ms, 250, "default backoff_initial_ms");
        assert_eq!(cfg.backoff_cap_ms, 30_000, "default backoff_cap_ms");
    }

    #[test]
    fn backoff_fields_read_env_overrides() {
        let cfg = with_env(|| {
            let _g1 = EnvGuard::set("AURA_LLM_MAX_RETRIES", "12");
            let _g2 = EnvGuard::set("AURA_LLM_BACKOFF_INITIAL_MS", "500");
            let _g3 = EnvGuard::set("AURA_LLM_BACKOFF_CAP_MS", "60000");
            let _g4 = EnvGuard::set("AURA_LLM_ROUTING", "direct");
            let _g5 = EnvGuard::set("AURA_ANTHROPIC_API_KEY", "sk-test");
            AnthropicConfig::from_env().expect("from_env")
        });
        assert_eq!(cfg.max_retries, 12);
        assert_eq!(cfg.backoff_initial_ms, 500);
        assert_eq!(cfg.backoff_cap_ms, 60_000);
    }
}
