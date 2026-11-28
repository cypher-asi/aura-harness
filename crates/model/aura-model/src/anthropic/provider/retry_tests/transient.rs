//! Axis 2 coverage: transient upstream 5xx responses.

use super::support::{cloudflare_block, failed, policy, DEFAULT_BODY_CAP};
use crate::anthropic::provider::retry::{classify_retry_action, retry_reason_for, RetryAction};
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use std::time::Duration;

#[test]
fn classify_retry_action_retries_transient_5xx_with_exp_backoff() {
    let err = ApiError::TransientServer {
        status: 500,
        message: "Model provider API error: 500 Internal Server Error - body".into(),
    };
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(0, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    match action {
        RetryAction::Retry {
            sleep,
            body_cap_override,
        } => {
            // `exp_backoff_with_jitter(0)` → base 1s + up to 250ms jitter.
            assert!(
                sleep >= Duration::from_secs(1),
                "first-attempt 5xx backoff must be >= 1s, got {sleep:?}"
            );
            assert!(
                sleep <= Duration::from_millis(1_300),
                "first-attempt 5xx backoff must stay under the jitter cap, got {sleep:?}"
            );
            assert_eq!(
                body_cap_override, None,
                "5xx retries must not tighten the body cap"
            );
        }
        other => panic!("expected Retry on 5xx, got {other:?}"),
    }
    match last_err {
        Some(ReasonerError::Transient { status, .. }) => assert_eq!(status, 500),
        other => panic!("expected ReasonerError::Transient last_err, got {other:?}"),
    }
}

#[test]
fn classify_retry_action_falls_back_when_5xx_retries_exhausted() {
    let err = ApiError::TransientServer {
        status: 502,
        message: "Model provider API error: 502 Bad Gateway - body".into(),
    };
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(2, 2, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(
        matches!(action, RetryAction::FallbackModel),
        "expected FallbackModel after 5xx retries are used up"
    );
    match last_err {
        Some(ReasonerError::Transient { status, .. }) => assert_eq!(status, 502),
        other => panic!("expected ReasonerError::Transient last_err, got {other:?}"),
    }
}

#[test]
fn classify_retry_action_propagates_5xx_when_no_fallback_available() {
    let err = ApiError::TransientServer {
        status: 504,
        message: "Model provider API error: 504 Gateway Timeout - body".into(),
    };
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(2, 1, "only", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(
        matches!(action, RetryAction::Propagate),
        "no fallback model → 5xx must propagate so the dev loop can retry"
    );
}

#[test]
fn retry_reason_for_labels_transient_5xx_distinctly() {
    // `upstream_5xx` must be distinct from the Cloudflare-specific
    // `cloudflare_block` bucket so run heuristics can separate
    // provider-internal outages from Cloudflare/WAF
    // blocks in retry histograms.
    let err = ApiError::TransientServer {
        status: 503,
        message: "Model provider API error: 503 - body".into(),
    };
    assert_eq!(retry_reason_for(&err), "upstream_5xx");
    assert_eq!(
        retry_reason_for(&cloudflare_block(None)),
        "cloudflare_block"
    );
}

#[test]
fn a_408_is_retried_like_a_5xx_until_the_cap() {
    use crate::anthropic::provider::errors::status_error;
    let status = reqwest::StatusCode::REQUEST_TIMEOUT;
    let (err, class) = status_error(status, "Request timed out", None);
    assert_eq!(class, "request_timeout_408");
    assert!(
        matches!(err, ApiError::TransientServer { status: 408, .. }),
        "{err:?}"
    );
    let mut last_err = None;
    let below = classify_retry_action(
        &err,
        &policy(2),
        &failed(1, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(matches!(below, RetryAction::Retry { .. }), "{below:?}");
    let at_cap = classify_retry_action(
        &err,
        &policy(2),
        &failed(2, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(matches!(at_cap, RetryAction::Propagate), "{at_cap:?}");
    assert!(
        matches!(last_err, Some(ReasonerError::Transient { status: 408, .. })),
        "{last_err:?}"
    );
}
