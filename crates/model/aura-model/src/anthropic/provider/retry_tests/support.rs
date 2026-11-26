use crate::anthropic::provider::retry::{FailedAttempt, RetryAction, RetryPolicy};
use crate::anthropic::ApiError;

/// Body cap the attempts in these tests were sent under unless a test
/// says otherwise.
pub(super) const DEFAULT_BODY_CAP: usize = 524_288;

/// Policy with 3 Cloudflare retries and a 1s..30s backoff schedule.
pub(super) fn policy(max_retries: u32) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        cloudflare_max_retries: 3,
        backoff_initial_ms: 1000,
        backoff_cap_ms: 30_000,
    }
}

/// A failed attempt on the first model of a `model_count`-long chain.
pub(super) fn failed(
    attempt: u32,
    model_count: usize,
    model: &str,
    current_body_cap: usize,
) -> FailedAttempt<'_> {
    FailedAttempt {
        attempt,
        model_idx: 0,
        model_count,
        model,
        current_body_cap,
    }
}

pub(super) fn cloudflare_block(wire_body_bytes: Option<usize>) -> ApiError {
    ApiError::CloudflareBlock {
        message: "cf".into(),
        wire_body_bytes,
    }
}

/// The tightened body cap of a `Retry`; panics with `missing` when the
/// retry kept the cap and on any other action.
pub(super) fn retry_body_cap(action: RetryAction, missing: &str) -> usize {
    match action {
        RetryAction::Retry {
            body_cap_override, ..
        } => body_cap_override.expect(missing),
        other => panic!("expected Retry, got {other:?}"),
    }
}
