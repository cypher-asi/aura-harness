//! Anthropic provider implementation.
//!
//! Uses `reqwest` directly to communicate with the Anthropic API.
//! This approach is more reliable than SDK wrappers which may have
//! incompatible or private internal types.
//!
//! Supports both synchronous and streaming completions via SSE.

mod api_types;
mod config;
mod convert;
mod provider;
mod sse;

pub use config::{AnthropicConfig, RoutingMode};

use crate::error::ReasonerError;

// ============================================================================
// Internal Error Classification (for retry logic)
// ============================================================================

#[derive(Debug)]
enum ApiError {
    /// 429 / 529 — retryable with backoff, then fallback.
    Overloaded(String),
    /// 402 — stop immediately, no retry or fallback.
    InsufficientCredits(String),
    /// 403 / 503 with Cloudflare HTML — retryable (service cold-starting).
    CloudflareBlock(String),
    /// Any other failure.
    Other(ReasonerError),
}

impl From<ApiError> for ReasonerError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Overloaded(msg) => ReasonerError::RateLimited(msg),
            ApiError::InsufficientCredits(msg) => ReasonerError::InsufficientCredits(msg),
            ApiError::CloudflareBlock(msg) => ReasonerError::Api {
                status: 403,
                message: msg,
            },
            ApiError::Other(e) => e,
        }
    }
}

fn is_cloudflare_html(body: &str) -> bool {
    body.contains("<!DOCTYPE html") && (body.contains("cloudflare") || body.contains("oldie"))
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Anthropic model provider.
///
/// Implements `ModelProvider` for the Anthropic API using direct HTTP calls.
/// Includes built-in retry with exponential backoff for overloaded (429/529)
/// errors and automatic fallback to a secondary model.
pub struct AnthropicProvider {
    client: reqwest::Client,
    config: AnthropicConfig,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    ///
    /// # Errors
    ///
    /// Returns error if client creation fails.
    pub fn new(config: AnthropicConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self { client, config })
    }

    /// Create from environment variables.
    ///
    /// # Errors
    ///
    /// Returns error if configuration or client creation fails.
    pub fn from_env() -> anyhow::Result<Self> {
        let config = AnthropicConfig::from_env()?;
        Self::new(config)
    }

    /// Build the ordered model fallback chain.
    pub(crate) fn model_chain(&self, primary: &str) -> Vec<String> {
        let mut models = vec![primary.to_string()];
        if let Some(ref fb) = self.config.fallback_model {
            if fb != primary {
                models.push(fb.clone());
            }
        }
        models
    }
}

#[cfg(test)]
mod tests;
