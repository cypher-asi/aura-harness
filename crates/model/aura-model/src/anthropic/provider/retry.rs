use super::millis_as_u64;
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use crate::{emit_retry, RetryInfo};
use std::time::{Duration, Instant};
use tracing::warn;

/// On every successive Cloudflare 403, multiply the emergency body cap
/// by this factor before retrying. Once the managed edge has already
/// rejected the request, recovering the turn is more important than
/// preserving the maximum possible context. A 1/2 shrink gives the
/// default three Cloudflare retries enough room to cross the observed
/// ~64 KiB chat WAF cliff; the normal proactive cap remains much more
/// generous for requests that have not been blocked.
const CLOUDFLARE_RETRY_SHRINK_NUMER: usize = 1;
const CLOUDFLARE_RETRY_SHRINK_DENOM: usize = 2;

/// Outcome of `classify_retry_action`.
///
/// `Retry { sleep, body_cap_override }` → sleep the given duration
/// then attempt again with the same model. `body_cap_override` is
/// `Some(cap)` only on Cloudflare 403 retries, where the cap shrinks
/// 50% per attempt so a body-size WAF rule is guaranteed to clear
/// within `cloudflare_max_retries` attempts. All other retry paths
/// (5xx, 429/529 overload) leave the cap alone.
///
/// `FallbackModel` → abandon this model, try the next in the
/// fallback chain. `Propagate` → give up, surface the underlying error.
#[derive(Debug)]
pub(super) enum RetryAction {
    Retry {
        sleep: Duration,
        body_cap_override: Option<usize>,
    },
    FallbackModel,
    Propagate,
}

/// Classify an `ApiError` into the next action for the retry loop.
///
/// For 429/529 we honour the upstream `Retry-After` hint (header or body) by
/// sleeping `max(retry_after, exponential_backoff)` plus a small jitter.
/// Previously this function used exponential backoff only (1s, 2s, 4s),
/// which — when the aura-router proxy reported `Retry after 7 seconds` —
/// burned every retry inside the rate-limit window and surfaced the 429 to
/// the user even though a single longer sleep would have unblocked the turn.
#[allow(clippy::too_many_arguments)]
pub(super) fn classify_retry_action(
    err: &ApiError,
    attempt: u32,
    max_retries: u32,
    cloudflare_max_retries: u32,
    backoff_initial_ms: u64,
    backoff_cap_ms: u64,
    model_idx: usize,
    model_count: usize,
    model: &str,
    last_err: &mut Option<ReasonerError>,
    current_body_cap: usize,
) -> RetryAction {
    match err {
        // Retry only this pending model request. No tools or conversation
        // state have been delivered by this attempt, so prior tool results
        // remain in the immutable request and are never executed again.
        ApiError::Transport(error) if attempt < max_retries => {
            let sleep = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
            warn!(model = %model, attempt, error = %error, "Model transport failed, will retry");
            RetryAction::Retry {
                sleep,
                body_cap_override: None,
            }
        }
        ApiError::CloudflareBlock {
            message,
            wire_body_bytes,
        } if attempt < max_retries.min(cloudflare_max_retries) => {
            let sleep = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
            // `Duration::as_millis` returns u128 but 30s backoff caps well below
            // u64::MAX; truncation cannot happen. `warn!` field value expressions
            // can't carry attributes directly, so bind first.
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = millis_as_u64(sleep.as_millis());
            // Shrink the body cap by 50% per attempt. When the cap is
            // disabled (0) we still produce an override so the next
            // attempt has *some* ceiling — pick a generous starting
            // point (256 KiB) so the first shrink isn't a sudden
            // step-change from "unlimited" to "tiny".
            let starting_cap = if current_body_cap == 0 {
                262_144
            } else {
                current_body_cap
            };
            let shrink_basis = wire_body_bytes
                .filter(|bytes| *bytes > 0)
                .map_or(starting_cap, |bytes| bytes.min(starting_cap));
            let next_cap = (shrink_basis.saturating_mul(CLOUDFLARE_RETRY_SHRINK_NUMER))
                / CLOUDFLARE_RETRY_SHRINK_DENOM;
            // Floor: never shrink below 16 KiB; below that point a
            // tighter cap won't appease the WAF, and we just damage
            // the conversation further.
            let next_cap = next_cap.max(16 * 1024);
            warn!(
                model = %model,
                attempt,
                backoff_ms,
                max_cloudflare_retries = cloudflare_max_retries,
                current_body_cap,
                wire_body_bytes = ?wire_body_bytes,
                shrink_basis,
                next_body_cap = next_cap,
                "Cloudflare block, will retry with tighter body cap"
            );
            *last_err = Some(ReasonerError::Transient {
                status: 403,
                message: message.clone(),
                retry_after: None,
            });
            RetryAction::Retry {
                sleep,
                body_cap_override: Some(next_cap),
            }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if attempt < max_retries => {
            let sleep =
                sleep_for_overloaded(attempt, *retry_after, backoff_initial_ms, backoff_cap_ms);
            // 60s cap on `sleep_for_overloaded` means u128 -> u64 is safe here.
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = millis_as_u64(sleep.as_millis());
            warn!(
                model = %model,
                attempt,
                backoff_ms,
                retry_after_s = ?retry_after.map(|d| d.as_secs()),
                "API overloaded, will retry"
            );
            *last_err = Some(ReasonerError::RateLimited {
                message: crate::anthropic::format_rate_limited_message(message, *retry_after),
                retry_after: *retry_after,
            });
            RetryAction::Retry {
                sleep,
                body_cap_override: None,
            }
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if model_idx < model_count - 1 => {
            warn!(model = %model, "Retries exhausted, falling back to next model");
            *last_err = Some(ReasonerError::RateLimited {
                message: crate::anthropic::format_rate_limited_message(message, *retry_after),
                retry_after: *retry_after,
            });
            RetryAction::FallbackModel
        }
        // Axis 2: retry generic 5xx just like Cloudflare cold-starts,
        // using the same exponential-backoff-with-jitter schedule.
        // These resolve on the order of seconds on the provider side;
        // `exp_backoff_with_jitter` caps at 30s so we never wedge the
        // dev loop behind a single provider incident.
        ApiError::TransientServer { status, message } if attempt < max_retries => {
            let sleep = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
            #[allow(clippy::cast_possible_truncation)]
            let backoff_ms = millis_as_u64(sleep.as_millis());
            warn!(
                model = %model,
                attempt,
                status = *status,
                backoff_ms,
                "Upstream 5xx, will retry"
            );
            *last_err = Some(ReasonerError::Transient {
                status: *status,
                message: message.clone(),
                retry_after: None,
            });
            RetryAction::Retry {
                sleep,
                body_cap_override: None,
            }
        }
        // After retries are exhausted, try the fallback model rather
        // than surfacing the 5xx to the caller — the same escape hatch
        // we already give 429/529 overload errors.
        ApiError::TransientServer { status, message } if model_idx < model_count - 1 => {
            warn!(
                model = %model,
                status = *status,
                "5xx retries exhausted, falling back to next model"
            );
            *last_err = Some(ReasonerError::Transient {
                status: *status,
                message: message.clone(),
                retry_after: None,
            });
            RetryAction::FallbackModel
        }
        _ => RetryAction::Propagate,
    }
}

/// Classify an `ApiError` into the `reason` string we expose through
/// [`RetryInfo::reason`]. Keep the strings stable: the aura-harness
/// debug-event pipeline writes them verbatim into `retries.jsonl`.
pub(super) fn retry_reason_for(err: &ApiError) -> &'static str {
    match err {
        ApiError::Overloaded { .. } => "rate_limited_429",
        ApiError::CloudflareBlock { .. } => "cloudflare_block",
        // Axis 2: distinct label so the dev loop can tell a real
        // upstream 5xx apart from Cloudflare/WAF blocks in
        // `retries.jsonl` (the heuristic reports bucket by reason).
        ApiError::TransientServer { .. } => "upstream_5xx",
        ApiError::InsufficientCredits(_) => "insufficient_credits",
        ApiError::Transport(ReasonerError::Timeout) => "transport_timeout",
        ApiError::Transport(_) => "transport",
        ApiError::Other(_) => "transient",
    }
}

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
type AttemptFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'a>>;

/// Per-attempt context passed into the body-building closure. The
/// closure consumes `body_cap_override` and forwards it into
/// [`AnthropicProvider::send_checked_with_cap`] so a 403 retry uses a
/// tighter cap than the previous attempt did.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttemptContext {
    pub model_idx: usize,
    /// `None` on the first attempt for a given model. Populated by
    /// the retry classifier when the previous attempt hit a 403.
    pub body_cap_override: Option<usize>,
}

pub(super) async fn run_model_chain_with_retries<'env, T, F>(
    config: &crate::anthropic::AnthropicConfig,
    models: &[String],
    mut attempt: F,
) -> Result<T, ReasonerError>
where
    F: FnMut(AttemptContext, String) -> AttemptFuture<'env, T> + 'env,
{
    let mut last_err: Option<ReasonerError> = None;

    let cloudflare_max_retries = if config.cloudflare_max_retries == 0 {
        // 0 explicitly disables Cloudflare retries; the classifier
        // already short-circuits via the `attempt < ...` guard, but
        // we keep the const fallback so legacy callers still get
        // sensible behaviour.
        0
    } else {
        config.cloudflare_max_retries
    };

    'outer: for (model_idx, model) in models.iter().enumerate() {
        let mut pending_sleep: Option<Duration> = None;
        let mut pending_body_cap_override: Option<usize> = None;
        for try_n in 0..=config.max_retries {
            if let Some(sleep) = pending_sleep.take() {
                tokio::time::sleep(sleep).await;
            }

            let ctx = AttemptContext {
                model_idx,
                body_cap_override: pending_body_cap_override,
            };
            match attempt(ctx, model.clone()).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    let current_body_cap =
                        pending_body_cap_override.unwrap_or(config.emergency_body_cap_bytes);
                    let action = classify_retry_action(
                        &e,
                        try_n,
                        config.max_retries,
                        cloudflare_max_retries,
                        config.backoff_initial_ms,
                        config.backoff_cap_ms,
                        model_idx,
                        models.len(),
                        model,
                        &mut last_err,
                        current_body_cap,
                    );
                    match action {
                        RetryAction::Retry {
                            sleep,
                            body_cap_override,
                        } => {
                            emit_retry_observation(&e, sleep, try_n, model);
                            #[allow(clippy::cast_possible_truncation)]
                            let sleep_ms = millis_as_u64(sleep.as_millis());
                            crate::console::anthropic_retry_decision_line(
                                &crate::console::RetryDecisionView::Retry {
                                    attempt_that_failed: try_n,
                                    max_retries: config.max_retries,
                                    sleep_ms,
                                    body_cap_bytes: body_cap_override.or({
                                        if pending_body_cap_override.is_some() {
                                            pending_body_cap_override
                                        } else {
                                            None
                                        }
                                    }),
                                },
                            );
                            pending_sleep = Some(sleep);
                            // A `Some` override carries forward; a `None`
                            // leaves whatever we already had in place
                            // (so a 403 → 5xx ladder keeps the tightened
                            // cap rather than springing back up).
                            if body_cap_override.is_some() {
                                pending_body_cap_override = body_cap_override;
                            }
                        }
                        RetryAction::FallbackModel => {
                            let next_model =
                                models.get(model_idx + 1).map_or("(none)", String::as_str);
                            crate::console::anthropic_retry_decision_line(
                                &crate::console::RetryDecisionView::Fallback { next_model },
                            );
                            continue 'outer;
                        }
                        RetryAction::Propagate => {
                            crate::console::anthropic_retry_decision_line(
                                &crate::console::RetryDecisionView::Propagate {
                                    reason: retry_reason_for(&e),
                                },
                            );
                            return Err(e.into());
                        }
                    }
                }
            }
        }
    }

    crate::console::anthropic_retry_decision_line(&crate::console::RetryDecisionView::Propagate {
        reason: "all models exhausted",
    });
    Err(last_err.unwrap_or_else(|| {
        ReasonerError::Internal("All models in fallback chain exhausted".into())
    }))
}

/// Pure exponential backoff with small jitter for non-overloaded retries
/// (e.g. Cloudflare cold-starts, per-tool-call streaming retries in
/// `aura_agent::agent_loop::streaming`).
///
/// The `initial_ms` and `cap_ms` parameters come from
/// [`crate::anthropic::AnthropicConfig::backoff_initial_ms`] /
/// [`crate::anthropic::AnthropicConfig::backoff_cap_ms`] (env-overridable via
/// `AURA_LLM_BACKOFF_INITIAL_MS` / `AURA_LLM_BACKOFF_CAP_MS`) so
/// operators can widen the window without rebuilding. `pub` because
/// the agent crate reuses this exact schedule for its per-tool-call
/// retry loop.
#[must_use]
pub fn exp_backoff_with_jitter(attempt: u32, initial_ms: u64, cap_ms: u64) -> Duration {
    let base_ms = initial_ms.saturating_mul(2u64.saturating_pow(attempt));
    let capped = base_ms.min(cap_ms);
    let jitter = jitter_ms(capped);
    Duration::from_millis(capped.saturating_add(jitter))
}

/// Compute the sleep before retrying an overloaded/429 error.
///
/// Returns `max(retry_after, exp_backoff) + jitter`. When the upstream tells
/// us to wait N seconds we always honour it (and then some), otherwise we
/// fall back to exponential backoff. Capped at 60s so a mis-reported
/// retry-after cannot wedge the loop indefinitely.
pub(super) fn sleep_for_overloaded(
    attempt: u32,
    retry_after: Option<Duration>,
    backoff_initial_ms: u64,
    backoff_cap_ms: u64,
) -> Duration {
    let exp = exp_backoff_with_jitter(attempt, backoff_initial_ms, backoff_cap_ms);
    let chosen = match retry_after {
        // Pad by 500ms to clear the window edge.
        Some(hint) => exp.max(hint + Duration::from_millis(500)),
        None => exp,
    };
    chosen.min(Duration::from_secs(60))
}

/// Deterministic-ish low-amplitude jitter (0..=250ms) based on the current
/// instant. Using `Instant` avoids pulling in a `rand` dependency for a
/// harmless spread.
fn jitter_ms(base_ms: u64) -> u64 {
    // Low-amplitude jitter only; we intentionally discard the high 64
    // bits of `as_nanos()` because we only need entropy, not precision.
    #[allow(clippy::cast_possible_truncation)]
    let seed = Instant::now().elapsed().as_nanos() as u64;
    // Scale jitter to at most 25% of base, capped at 250ms.
    let max_jitter = (base_ms / 4).min(250);
    if max_jitter == 0 {
        0
    } else {
        seed % (max_jitter + 1)
    }
}
