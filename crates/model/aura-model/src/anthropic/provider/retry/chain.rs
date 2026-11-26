//! The retry loop over the provider's model fallback chain.

use super::{classify_retry_action, retry_reason_for, FailedAttempt, RetryAction, RetryPolicy};
use crate::anthropic::provider::millis_as_u64;
use crate::anthropic::{AnthropicConfig, ApiError};
use crate::error::ReasonerError;
use crate::{emit_retry, RetryInfo};
use std::time::Duration;

/// Emit a `debug.retry` observation to the task-local observer (if
/// any). `attempt_that_failed` is the 0-based attempt counter of the
/// call that just failed; the 1-based "upcoming" attempt number is
/// `attempt_that_failed + 2`.
fn emit_retry_observation(err: &ApiError, sleep: Duration, attempt_that_failed: u32, model: &str) {
    let wait_ms = u64::try_from(sleep.as_millis()).unwrap_or(u64::MAX);
    let info = RetryInfo {
        reason: retry_reason_for(err).to_string(),
        attempt: attempt_that_failed.saturating_add(2),
        wait_ms,
        provider: "anthropic".to_string(),
        model: model.to_string(),
    };
    emit_retry(info);
}

type AttemptFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'a>>;

/// Per-attempt context passed into the body-building closure. The
/// closure consumes `body_cap_override` and forwards it into
/// [`AnthropicProvider::send_checked_with_cap`] so a 403 retry uses a
/// tighter cap than the previous attempt did.
///
/// [`AnthropicProvider::send_checked_with_cap`]: crate::anthropic::AnthropicProvider
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttemptContext {
    pub model_idx: usize,
    /// `None` on the first attempt for a given model. Populated by
    /// the retry classifier when the previous attempt hit a 403.
    pub body_cap_override: Option<usize>,
}

/// Drive an attempt closure across the provider's model fallback chain
/// with the retry / backoff schedule set by [`crate::anthropic::AnthropicConfig`].
///
/// `attempt(model_idx, model)` performs one full request → response
/// round-trip for the given model and returns either `Ok(T)` (success,
/// returned immediately to the caller) or `Err(ApiError)` (consumed by
/// [`classify_retry_action`] to decide between sleeping + retrying,
/// dropping to the next model in the chain, or propagating). The
/// classification, exponential-backoff schedule, and `last_err`
/// surfacing logic stay in one place so the streaming and
/// non-streaming `ModelProvider` impls below differ only in the
/// per-attempt body — see the bullet on "Anthropic retry loops" in
/// the system-audit refactor plan.
///
/// Errors are surfaced as `ReasonerError` (the trait error type); the
/// classifier converts the underlying `ApiError` so callers don't have
/// to handle the internal variant.
pub(in crate::anthropic::provider) async fn run_model_chain_with_retries<'env, T, F>(
    config: &AnthropicConfig,
    models: &[String],
    mut attempt: F,
) -> Result<T, ReasonerError>
where
    F: FnMut(AttemptContext, String) -> AttemptFuture<'env, T> + 'env,
{
    let mut chain = ModelChain {
        config,
        policy: RetryPolicy {
            max_retries: config.max_retries,
            // 0 explicitly disables Cloudflare retries; the classifier
            // short-circuits via the `attempt < ...` guard.
            cloudflare_max_retries: config.cloudflare_max_retries,
            backoff_initial_ms: config.backoff_initial_ms,
            backoff_cap_ms: config.backoff_cap_ms,
        },
        models,
        last_err: None,
    };
    for (model_idx, model) in models.iter().enumerate() {
        if let Some(result) = chain.run_model(model_idx, model, &mut attempt).await {
            return result;
        }
    }
    chain.exhausted()
}

/// Loop state shared by every model in the chain.
struct ModelChain<'c> {
    config: &'c AnthropicConfig,
    policy: RetryPolicy,
    models: &'c [String],
    last_err: Option<ReasonerError>,
}

/// What the loop does after a failed attempt.
enum NextStep {
    RetrySameModel,
    NextModel,
    Fail(ReasonerError),
}

/// The sleep and body cap carried from one attempt of a model to the next.
#[derive(Debug, Default)]
struct PendingRetry {
    sleep: Option<Duration>,
    body_cap_override: Option<usize>,
}

impl PendingRetry {
    async fn wait(&mut self) {
        if let Some(sleep) = self.sleep.take() {
            tokio::time::sleep(sleep).await;
        }
    }

    fn schedule(&mut self, sleep: Duration, body_cap_override: Option<usize>) {
        self.sleep = Some(sleep);
        // A `Some` override carries forward; a `None`
        // leaves whatever we already had in place
        // (so a 403 → 5xx ladder keeps the tightened
        // cap rather than springing back up).
        if body_cap_override.is_some() {
            self.body_cap_override = body_cap_override;
        }
    }
}

impl ModelChain<'_> {
    /// Run up to `max_retries + 1` attempts against one model. `None`
    /// means move on to the next model in the chain.
    async fn run_model<'env, T, F>(
        &mut self,
        model_idx: usize,
        model: &str,
        attempt: &mut F,
    ) -> Option<Result<T, ReasonerError>>
    where
        F: FnMut(AttemptContext, String) -> AttemptFuture<'env, T>,
    {
        let mut pending = PendingRetry::default();
        for try_n in 0..=self.policy.max_retries {
            pending.wait().await;

            let ctx = AttemptContext {
                model_idx,
                body_cap_override: pending.body_cap_override,
            };
            let err = match attempt(ctx, model.to_string()).await {
                Ok(value) => return Some(Ok(value)),
                Err(e) => e,
            };
            let failed = FailedAttempt {
                attempt: try_n,
                model_idx,
                model_count: self.models.len(),
                model,
                current_body_cap: pending
                    .body_cap_override
                    .unwrap_or(self.config.emergency_body_cap_bytes),
            };
            match self.next_step(err, &failed, &mut pending) {
                NextStep::RetrySameModel => {}
                NextStep::NextModel => return None,
                NextStep::Fail(err) => return Some(Err(err)),
            }
        }
        None
    }

    fn next_step(
        &mut self,
        err: ApiError,
        failed: &FailedAttempt<'_>,
        pending: &mut PendingRetry,
    ) -> NextStep {
        match classify_retry_action(&err, &self.policy, failed, &mut self.last_err) {
            RetryAction::Retry {
                sleep,
                body_cap_override,
            } => {
                self.announce_retry(
                    &err,
                    failed,
                    sleep,
                    body_cap_override.or(pending.body_cap_override),
                );
                pending.schedule(sleep, body_cap_override);
                NextStep::RetrySameModel
            }
            RetryAction::FallbackModel => {
                let next_model = self
                    .models
                    .get(failed.model_idx + 1)
                    .map_or("(none)", String::as_str);
                crate::console::anthropic_retry_decision_line(
                    &crate::console::RetryDecisionView::Fallback { next_model },
                );
                NextStep::NextModel
            }
            RetryAction::Propagate => {
                crate::console::anthropic_retry_decision_line(
                    &crate::console::RetryDecisionView::Propagate {
                        reason: retry_reason_for(&err),
                    },
                );
                NextStep::Fail(err.into())
            }
        }
    }

    fn announce_retry(
        &self,
        err: &ApiError,
        failed: &FailedAttempt<'_>,
        sleep: Duration,
        body_cap_bytes: Option<usize>,
    ) {
        emit_retry_observation(err, sleep, failed.attempt, failed.model);
        #[allow(clippy::cast_possible_truncation)]
        let sleep_ms = millis_as_u64(sleep.as_millis());
        crate::console::anthropic_retry_decision_line(&crate::console::RetryDecisionView::Retry {
            attempt_that_failed: failed.attempt,
            max_retries: self.policy.max_retries,
            sleep_ms,
            body_cap_bytes,
        });
    }

    fn exhausted<T>(self) -> Result<T, ReasonerError> {
        crate::console::anthropic_retry_decision_line(
            &crate::console::RetryDecisionView::Propagate {
                reason: "all models exhausted",
            },
        );
        Err(self.last_err.unwrap_or_else(|| {
            ReasonerError::Internal("All models in fallback chain exhausted".into())
        }))
    }
}
