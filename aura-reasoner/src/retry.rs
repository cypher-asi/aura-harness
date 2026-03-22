//! Retry and model fallback logic.
//!
//! Provides exponential backoff for rate-limit errors (429/529)
//! and automatic fallback to alternative models when retries are exhausted.

use crate::error::ReasonerError;
use crate::{ModelProvider, ModelRequest, ModelResponse};
use std::time::Duration;
use tracing::{debug, warn};

/// Configuration for retry and fallback behavior.
pub struct RetryConfig {
    /// Fallback chain of model names (e.g., `["claude-opus-4-6-20250514", "claude-sonnet-4-20250514"]`).
    pub fallback_chain: Vec<String>,
    /// Maximum retries per model before falling back.
    pub max_retries_per_model: u32,
    /// Base backoff duration.
    pub base_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            fallback_chain: vec![],
            max_retries_per_model: 2,
            base_backoff: Duration::from_secs(1),
        }
    }
}

/// Execute a model request with retry and fallback.
///
/// Behavior:
/// - On **429/529**: exponential backoff with up to `max_retries_per_model` retries.
/// - On **402**: immediate stop (insufficient credits).
/// - After retries exhausted for retryable errors: fall back to next model in chain.
/// - On other errors: return immediately.
/// - On success: return immediately.
///
/// # Errors
///
/// Returns error when all models/retries are exhausted, or on non-retryable errors.
pub async fn complete_with_retry(
    provider: &dyn ModelProvider,
    mut request: ModelRequest,
    config: &RetryConfig,
) -> anyhow::Result<ModelResponse> {
    let mut models = vec![request.model.clone()];
    models.extend(config.fallback_chain.iter().cloned());

    let mut last_error: Option<anyhow::Error> = None;

    for (model_idx, model) in models.iter().enumerate() {
        request.model.clone_from(model);

        for attempt in 0..=config.max_retries_per_model {
            debug!(model = %model, attempt, "Attempting completion");

            match provider.complete(request.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let classification = e.downcast_ref::<ReasonerError>();

                    if matches!(classification, Some(ReasonerError::InsufficientCredits(_))) {
                        return Err(e);
                    }

                    let is_retryable =
                        matches!(classification, Some(ReasonerError::RateLimited(_)));

                    if is_retryable && attempt < config.max_retries_per_model {
                        let backoff = config.base_backoff * 2u32.pow(attempt);
                        warn!(
                            model = %model,
                            attempt,
                            backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX),
                            "Rate limited, backing off"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    last_error = Some(e);

                    if is_retryable {
                        warn!(
                            model = %model,
                            next_model = models.get(model_idx + 1).map(String::as_str),
                            "Retries exhausted, falling back"
                        );
                        break;
                    }

                    return Err(last_error
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("unexpected missing error"))?);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All models in fallback chain exhausted")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ModelResponse, ProviderTrace, StopReason, Usage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// Test provider that returns errors from a queue, then succeeds.
    struct RetryTestProvider {
        errors: Mutex<Vec<String>>,
        call_count: AtomicU32,
    }

    impl RetryTestProvider {
        fn new(errors: Vec<String>) -> Self {
            Self {
                errors: Mutex::new(errors),
                call_count: AtomicU32::new(0),
            }
        }

        fn call_count(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    /// Classify a test error string into a typed `ReasonerError`.
    fn classify_test_error(err: String) -> anyhow::Error {
        if err.contains("402") {
            ReasonerError::InsufficientCredits(err).into()
        } else if err.contains("429") || err.contains("529") {
            ReasonerError::RateLimited(err).into()
        } else {
            anyhow::anyhow!(err)
        }
    }

    #[async_trait]
    impl ModelProvider for RetryTestProvider {
        fn name(&self) -> &'static str {
            "retry_test"
        }

        async fn complete(&self, _request: ModelRequest) -> anyhow::Result<ModelResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut errors = self.errors.lock().unwrap();
            if errors.is_empty() {
                drop(errors);
                Ok(ModelResponse::new(
                    StopReason::EndTurn,
                    Message::assistant("ok"),
                    Usage::new(10, 5),
                    ProviderTrace::new("test-model", 100),
                ))
            } else {
                let err = errors.remove(0);
                drop(errors);
                Err(classify_test_error(err))
            }
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert!(config.fallback_chain.is_empty());
        assert_eq!(config.max_retries_per_model, 2);
        assert_eq!(config.base_backoff, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn test_successful_request_no_retry() {
        let provider = RetryTestProvider::new(vec![]);
        let request = ModelRequest::builder("test-model", "system").build();
        let config = RetryConfig::default();

        let result = complete_with_retry(&provider, request, &config).await;
        assert!(result.is_ok());
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn test_immediate_stop_on_402() {
        let provider = RetryTestProvider::new(vec![
            "Anthropic API error: 402 - insufficient credits".to_string(),
        ]);
        let request = ModelRequest::builder("test-model", "system").build();
        let config = RetryConfig::default();

        let result = complete_with_retry(&provider, request, &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("402"));
        assert_eq!(provider.call_count(), 1);
    }
}
