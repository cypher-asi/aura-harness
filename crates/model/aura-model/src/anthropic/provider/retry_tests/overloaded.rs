use super::support::{failed, policy, DEFAULT_BODY_CAP};
use crate::anthropic::provider::retry::{classify_retry_action, sleep_for_overloaded, RetryAction};
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use std::time::Duration;

#[test]
fn sleep_for_overloaded_waits_past_the_upstream_hint() {
    // Attempt 0 would otherwise sleep ~1s of exp backoff, but the upstream
    // told us 7s — the next attempt must land after the window.
    let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(7)), 1000, 30_000);
    assert!(
        sleep >= Duration::from_millis(7_500),
        "sleep ({:?}) must clear the 7s retry-after window",
        sleep
    );
    assert!(
        sleep <= Duration::from_secs(60),
        "sleep must be capped at 60s, got {:?}",
        sleep
    );
}

#[test]
fn sleep_for_overloaded_caps_absurd_retry_after() {
    let sleep = sleep_for_overloaded(0, Some(Duration::from_secs(3600)), 1000, 30_000);
    assert!(
        sleep <= Duration::from_secs(60),
        "sleep must be capped at 60s, got {:?}",
        sleep
    );
}

#[test]
fn sleep_for_overloaded_falls_back_to_exp_backoff_without_hint() {
    // attempt=0 → base 1s + up to 250ms jitter
    let sleep = sleep_for_overloaded(0, None, 1000, 30_000);
    assert!(sleep >= Duration::from_secs(1));
    assert!(sleep <= Duration::from_millis(1_250) + Duration::from_millis(50));
}

fn rate_limited_with_7s_hint() -> ApiError {
    ApiError::Overloaded {
        message: "429 rate limited".into(),
        retry_after: Some(Duration::from_secs(7)),
    }
}

#[test]
fn classify_retry_action_honours_retry_after_hint() {
    let mut last_err = None;
    let action = classify_retry_action(
        &rate_limited_with_7s_hint(),
        &policy(2),
        &failed(0, 1, "test-model", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    match action {
        RetryAction::Retry {
            sleep,
            body_cap_override,
        } => {
            assert!(
                sleep >= Duration::from_millis(7_500),
                "retry sleep ({:?}) must clear the 7s upstream window",
                sleep
            );
            assert_eq!(
                body_cap_override, None,
                "rate-limited retries must not tighten the body cap"
            );
        }
        other => panic!("expected Retry, got {:?}", other),
    }
}

#[test]
fn classify_retry_action_surfaces_retry_after_hint_in_last_err() {
    let mut last_err = None;
    classify_retry_action(
        &rate_limited_with_7s_hint(),
        &policy(2),
        &failed(0, 1, "test-model", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    match last_err {
        Some(ReasonerError::RateLimited {
            ref message,
            retry_after,
        }) => {
            assert!(
                message.to_ascii_lowercase().contains("retry after"),
                "last_err should surface the retry-after hint: {message}"
            );
            assert_eq!(
                retry_after,
                Some(Duration::from_secs(7)),
                "structured retry_after should match the upstream hint"
            );
        }
        ref other => panic!("expected RateLimited, got {:?}", other),
    }
}

#[test]
fn classify_retry_action_falls_back_after_exhausting_retries() {
    let err = ApiError::Overloaded {
        message: "429 rate limited".into(),
        retry_after: None,
    };
    let mut last_err = None;
    // attempt == max_retries → retries exhausted, fallback chain available
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(2, 2, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(matches!(action, RetryAction::FallbackModel));
    assert!(matches!(last_err, Some(ReasonerError::RateLimited { .. })));
}

#[test]
fn classify_retry_action_propagates_when_no_fallback_available() {
    let err = ApiError::Overloaded {
        message: "429 rate limited".into(),
        retry_after: None,
    };
    let mut last_err = None;
    // attempt == max_retries AND model_idx == model_count - 1 → no fallback left
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(2, 1, "only", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(matches!(action, RetryAction::Propagate));
}

#[test]
fn classify_retry_action_other_errors_propagate() {
    let err = ApiError::Other(ReasonerError::Request("boom".into()));
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        &policy(2),
        &failed(0, 1, "m", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    assert!(matches!(action, RetryAction::Propagate));
}
