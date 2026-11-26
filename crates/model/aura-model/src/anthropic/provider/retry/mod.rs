mod chain;

pub(super) use chain::run_model_chain_with_retries;

use super::millis_as_u64;
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
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

/// Retry limits and backoff schedule taken from
/// [`crate::anthropic::AnthropicConfig`].
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryPolicy {
    pub(super) max_retries: u32,
    pub(super) cloudflare_max_retries: u32,
    pub(super) backoff_initial_ms: u64,
    pub(super) backoff_cap_ms: u64,
}

impl RetryPolicy {
    fn backoff(&self, attempt: u32) -> Duration {
        exp_backoff_with_jitter(attempt, self.backoff_initial_ms, self.backoff_cap_ms)
    }
}

/// The attempt that just failed: its 0-based number, where its model
/// sits in the fallback chain, and the body cap it was sent under.
#[derive(Debug, Clone, Copy)]
pub(super) struct FailedAttempt<'a> {
    pub(super) attempt: u32,
    pub(super) model_idx: usize,
    pub(super) model_count: usize,
    pub(super) model: &'a str,
    pub(super) current_body_cap: usize,
}

impl FailedAttempt<'_> {
    fn has_fallback_model(&self) -> bool {
        self.model_idx < self.model_count - 1
    }
}

/// Classify an `ApiError` into the next action for the retry loop.
///
/// For 429/529 we honour the upstream `Retry-After` hint (header or body) by
/// sleeping `max(retry_after, exponential_backoff)` plus a small jitter.
/// Previously this function used exponential backoff only (1s, 2s, 4s),
/// which — when the aura-router proxy reported `Retry after 7 seconds` —
/// burned every retry inside the rate-limit window and surfaced the 429 to
/// the user even though a single longer sleep would have unblocked the turn.
pub(super) fn classify_retry_action(
    err: &ApiError,
    policy: &RetryPolicy,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    let can_retry = failed.attempt < policy.max_retries;
    match err {
        // Retry only this pending model request. No tools or conversation
        // state have been delivered by this attempt, so prior tool results
        // remain in the immutable request and are never executed again.
        ApiError::Transport(error) if can_retry => retry_transport(error, policy, failed),
        ApiError::CloudflareBlock {
            message,
            wire_body_bytes,
        } if failed.attempt < policy.max_retries.min(policy.cloudflare_max_retries) => {
            retry_cloudflare(message, *wire_body_bytes, policy, failed, last_err)
        }
        ApiError::Overloaded {
            message,
            retry_after,
        } if can_retry => retry_overloaded(message, *retry_after, policy, failed, last_err),
        ApiError::Overloaded {
            message,
            retry_after,
        } if failed.has_fallback_model() => {
            fallback_overloaded(message, *retry_after, failed, last_err)
        }
        // Axis 2: retry generic 5xx just like Cloudflare cold-starts,
        // using the same exponential-backoff-with-jitter schedule.
        // These resolve on the order of seconds on the provider side;
        // `exp_backoff_with_jitter` caps at 30s so we never wedge the
        // dev loop behind a single provider incident.
        ApiError::TransientServer { status, message } if can_retry => {
            retry_transient_server(*status, message, policy, failed, last_err)
        }
        // After retries are exhausted, try the fallback model rather
        // than surfacing the 5xx to the caller — the same escape hatch
        // we already give 429/529 overload errors.
        ApiError::TransientServer { status, message } if failed.has_fallback_model() => {
            fallback_transient_server(*status, message, failed, last_err)
        }
        _ => RetryAction::Propagate,
    }
}

fn retry_transport(
    error: &ReasonerError,
    policy: &RetryPolicy,
    failed: &FailedAttempt<'_>,
) -> RetryAction {
    let sleep = policy.backoff(failed.attempt);
    let (model, attempt) = (failed.model, failed.attempt);
    warn!(model = %model, attempt, error = %error, "Model transport failed, will retry");
    RetryAction::Retry {
        sleep,
        body_cap_override: None,
    }
}

fn retry_cloudflare(
    message: &str,
    wire_body_bytes: Option<usize>,
    policy: &RetryPolicy,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    let sleep = policy.backoff(failed.attempt);
    // `Duration::as_millis` returns u128 but 30s backoff caps well below
    // u64::MAX; truncation cannot happen. `warn!` field value expressions
    // can't carry attributes directly, so bind first.
    #[allow(clippy::cast_possible_truncation)]
    let backoff_ms = millis_as_u64(sleep.as_millis());
    let current_body_cap = failed.current_body_cap;
    let (shrink_basis, next_cap) = shrink_body_cap(current_body_cap, wire_body_bytes);
    warn!(
        model = %failed.model,
        attempt = failed.attempt,
        backoff_ms,
        max_cloudflare_retries = policy.cloudflare_max_retries,
        current_body_cap,
        wire_body_bytes = ?wire_body_bytes,
        shrink_basis,
        next_body_cap = next_cap,
        "Cloudflare block, will retry with tighter body cap"
    );
    *last_err = Some(ReasonerError::Transient {
        status: 403,
        message: message.to_string(),
        retry_after: None,
    });
    RetryAction::Retry {
        sleep,
        body_cap_override: Some(next_cap),
    }
}

/// Returns `(shrink_basis, next_cap)` for a Cloudflare retry.
fn shrink_body_cap(current_body_cap: usize, wire_body_bytes: Option<usize>) -> (usize, usize) {
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
    (shrink_basis, next_cap.max(16 * 1024))
}

fn retry_overloaded(
    message: &str,
    retry_after: Option<Duration>,
    policy: &RetryPolicy,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    let sleep = sleep_for_overloaded(
        failed.attempt,
        retry_after,
        policy.backoff_initial_ms,
        policy.backoff_cap_ms,
    );
    // 60s cap on `sleep_for_overloaded` means u128 -> u64 is safe here.
    #[allow(clippy::cast_possible_truncation)]
    let backoff_ms = millis_as_u64(sleep.as_millis());
    warn!(
        model = %failed.model,
        attempt = failed.attempt,
        backoff_ms,
        retry_after_s = ?retry_after.map(|d| d.as_secs()),
        "API overloaded, will retry"
    );
    *last_err = Some(rate_limited(message, retry_after));
    RetryAction::Retry {
        sleep,
        body_cap_override: None,
    }
}

fn fallback_overloaded(
    message: &str,
    retry_after: Option<Duration>,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    warn!(model = %failed.model, "Retries exhausted, falling back to next model");
    *last_err = Some(rate_limited(message, retry_after));
    RetryAction::FallbackModel
}

fn rate_limited(message: &str, retry_after: Option<Duration>) -> ReasonerError {
    ReasonerError::RateLimited {
        message: crate::anthropic::format_rate_limited_message(message, retry_after),
        retry_after,
    }
}

fn retry_transient_server(
    status: u16,
    message: &str,
    policy: &RetryPolicy,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    let sleep = policy.backoff(failed.attempt);
    #[allow(clippy::cast_possible_truncation)]
    let backoff_ms = millis_as_u64(sleep.as_millis());
    warn!(
        model = %failed.model,
        attempt = failed.attempt,
        status,
        backoff_ms,
        "Upstream 5xx, will retry"
    );
    *last_err = Some(transient(status, message));
    RetryAction::Retry {
        sleep,
        body_cap_override: None,
    }
}

fn fallback_transient_server(
    status: u16,
    message: &str,
    failed: &FailedAttempt<'_>,
    last_err: &mut Option<ReasonerError>,
) -> RetryAction {
    warn!(
        model = %failed.model,
        status,
        "5xx retries exhausted, falling back to next model"
    );
    *last_err = Some(transient(status, message));
    RetryAction::FallbackModel
}

fn transient(status: u16, message: &str) -> ReasonerError {
    ReasonerError::Transient {
        status,
        message: message.to_string(),
        retry_after: None,
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
