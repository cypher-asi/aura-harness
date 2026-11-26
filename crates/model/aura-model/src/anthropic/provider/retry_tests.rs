use super::errors::{
    classify_transport_error, parse_retry_after_from_body, parse_retry_after_header,
    parse_retry_after_prose, provider_api_error_message,
};
use super::retry::{classify_retry_action, retry_reason_for, sleep_for_overloaded, RetryAction};
use crate::anthropic::ApiError;
use crate::error::ReasonerError;
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::Duration;

#[test]
fn transport_builder_errors_are_not_retryable() {
    let error = reqwest::Client::new()
        .post("://invalid-url")
        .build()
        .unwrap_err();
    assert!(error.is_builder());
    assert!(matches!(
        classify_transport_error(&error, "invalid request"),
        ApiError::Other(ReasonerError::Request(_))
    ));
}

#[test]
fn retry_after_header_parses_integer_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, HeaderValue::from_static("7"));
    assert_eq!(
        parse_retry_after_header(&headers),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn retry_after_header_absent_is_none() {
    let headers = HeaderMap::new();
    assert_eq!(parse_retry_after_header(&headers), None);
}

#[test]
fn retry_after_body_parses_aura_router_shape() {
    let body = r#"{"error":{"code":"RATE_LIMITED","message":"Too many requests. Retry after 7 seconds."}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn retry_after_body_parses_structured_retry_after() {
    let body = r#"{"error":{"code":"RATE_LIMITED","retry_after":12,"message":"slow down"}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(12))
    );
}

#[test]
fn retry_after_body_parses_top_level_retry_after() {
    let body = r#"{"retry_after":3,"error":{"code":"OTHER"}}"#;
    assert_eq!(
        parse_retry_after_from_body(body),
        Some(Duration::from_secs(3))
    );
}

#[test]
fn retry_after_body_returns_none_when_absent() {
    let body = r#"{"error":{"code":"RATE_LIMITED","message":"slow down"}}"#;
    assert_eq!(parse_retry_after_from_body(body), None);
}

#[test]
fn provider_api_error_message_does_not_misidentify_routed_provider() {
    let body = r#"{"error":{"message":"You exceeded your current quota."}}"#;
    let message = provider_api_error_message(reqwest::StatusCode::TOO_MANY_REQUESTS, body);

    assert_eq!(
        message,
        format!("Model provider API error: 429 Too Many Requests - {body}")
    );
    assert!(!message.contains("Anthropic"));
}

#[test]
fn retry_after_prose_is_case_insensitive_and_handles_plural() {
    assert_eq!(
        parse_retry_after_prose("please Retry After 5 seconds"),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        parse_retry_after_prose("retry after 1 second"),
        Some(Duration::from_secs(1))
    );
}

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

#[test]
fn classify_retry_action_honours_retry_after_hint() {
    let err = ApiError::Overloaded {
        message: "429 rate limited".into(),
        retry_after: Some(Duration::from_secs(7)),
    };
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        0,
        2,
        3,
        1000,
        30_000,
        0,
        1,
        "test-model",
        &mut last_err,
        524_288,
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
        2,
        2,
        3,
        1000,
        30_000,
        0,
        2,
        "primary",
        &mut last_err,
        524_288,
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
        2,
        2,
        3,
        1000,
        30_000,
        0,
        1,
        "only",
        &mut last_err,
        524_288,
    );
    assert!(matches!(action, RetryAction::Propagate));
}

#[test]
fn classify_retry_action_other_errors_propagate() {
    let err = ApiError::Other(ReasonerError::Request("boom".into()));
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        0,
        2,
        3,
        1000,
        30_000,
        0,
        1,
        "m",
        &mut last_err,
        524_288,
    );
    assert!(matches!(action, RetryAction::Propagate));
}

// ---------- Axis 2 coverage ----------

#[test]
fn classify_retry_action_retries_transient_5xx_with_exp_backoff() {
    let err = ApiError::TransientServer {
        status: 500,
        message: "Model provider API error: 500 Internal Server Error - body".into(),
    };
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        0,
        2,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        524_288,
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
        2,
        2,
        3,
        1000,
        30_000,
        0,
        2,
        "primary",
        &mut last_err,
        524_288,
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
        2,
        2,
        3,
        1000,
        30_000,
        0,
        1,
        "only",
        &mut last_err,
        524_288,
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

fn cloudflare_block(wire_body_bytes: Option<usize>) -> ApiError {
    ApiError::CloudflareBlock {
        message: "cf".into(),
        wire_body_bytes,
    }
}

#[test]
fn classify_retry_action_caps_cloudflare_retries() {
    let err = cloudflare_block(None);
    let mut last_err = None;

    // With cloudflare_max_retries=3, attempts 0/1/2 should all
    // retry; attempt 3 must propagate so a chronically WAF-blocked
    // request cannot burn the full generic retry budget on
    // duplicate 403s. The shrink-per-attempt schedule makes those
    // three retries useful rather than wasted.
    for attempt in 0..3 {
        let action = classify_retry_action(
            &err,
            attempt,
            8,
            3,
            1000,
            30_000,
            0,
            1,
            "primary",
            &mut last_err,
            524_288,
        );
        assert!(
            matches!(action, RetryAction::Retry { .. }),
            "attempt {attempt} should still retry the Cloudflare block"
        );
    }

    let exhausted = classify_retry_action(
        &err,
        3,
        8,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        524_288,
    );
    assert!(
        matches!(exhausted, RetryAction::Propagate),
        "Cloudflare block must propagate once cloudflare_max_retries is exhausted"
    );
}

#[test]
fn classify_retry_action_shrinks_configured_body_cap_on_cloudflare() {
    let err = cloudflare_block(None);
    let mut last_err = None;

    let first = classify_retry_action(
        &err,
        0,
        8,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        524_288,
    );
    let first_cap = match first {
        RetryAction::Retry {
            body_cap_override, ..
        } => body_cap_override.expect("Cloudflare retry must set a tighter body cap"),
        other => panic!("expected Retry, got {other:?}"),
    };
    assert!(
        first_cap < 524_288,
        "shrunk cap ({first_cap}) must be strictly smaller than the previous one (524288)"
    );
    // 1/2 of 524288 = 262144.
    assert_eq!(first_cap, 262_144, "first shrink should halve the cap");

    // Second Cloudflare hit: cap shrinks again from the previous override.
    let second = classify_retry_action(
        &err,
        1,
        8,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        first_cap,
    );
    let second_cap = match second {
        RetryAction::Retry {
            body_cap_override, ..
        } => body_cap_override.expect("Cloudflare retry must keep tightening the cap"),
        other => panic!("expected Retry, got {other:?}"),
    };
    assert!(
        second_cap < first_cap,
        "second shrink ({second_cap}) must be strictly smaller than first ({first_cap})"
    );
}

#[test]
fn classify_retry_action_shrinks_observed_body_on_cloudflare() {
    let observed_body_bytes = 158_300;
    let err = cloudflare_block(Some(observed_body_bytes));
    let mut last_err = None;

    let action = classify_retry_action(
        &err,
        0,
        8,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        524_288,
    );
    let cap = match action {
        RetryAction::Retry {
            body_cap_override, ..
        } => body_cap_override.expect("Cloudflare retry must set a tighter body cap"),
        other => panic!("expected Retry, got {other:?}"),
    };

    assert!(
        cap < observed_body_bytes,
        "retry cap ({cap}) must be lower than the blocked wire body ({observed_body_bytes})"
    );
    assert_eq!(
        cap, 79_150,
        "cap should halve from the observed blocked wire body"
    );
}

#[test]
fn classify_retry_action_crosses_low_waf_band_with_default_retries() {
    // Production desktop traces showed chat WAF blocks with
    // messages_text_bytes around 64-82 KiB. The old 25% shrink
    // could still leave a ~158 KiB blocked wire body above that
    // band after the default three Cloudflare retries. The retry
    // ladder should now get under 64 KiB before it gives up.
    let err = cloudflare_block(Some(158_300));
    let mut last_err = None;
    let mut current_cap = 524_288;

    for attempt in 0..3 {
        let action = classify_retry_action(
            &err,
            attempt,
            8,
            3,
            1000,
            30_000,
            0,
            1,
            "primary",
            &mut last_err,
            current_cap,
        );
        current_cap = match action {
            RetryAction::Retry {
                body_cap_override, ..
            } => body_cap_override.expect("Cloudflare retry must set a tighter body cap"),
            other => panic!("expected Retry, got {other:?}"),
        };
    }

    assert!(
            current_cap < 64 * 1024,
            "three Cloudflare retries should drive the cap below the observed low-WAF band; got {current_cap}"
        );
}

#[test]
fn classify_retry_action_cloudflare_shrink_has_a_floor() {
    // Repeatedly shrinking from a tiny starting cap must never go
    // below 16 KiB — that's the floor where further shrinks just
    // damage the conversation without appeasing any WAF rule.
    let err = cloudflare_block(None);
    let mut last_err = None;
    let action = classify_retry_action(
        &err,
        0,
        8,
        3,
        1000,
        30_000,
        0,
        1,
        "primary",
        &mut last_err,
        8 * 1024, // already below the floor
    );
    match action {
        RetryAction::Retry {
            body_cap_override, ..
        } => {
            assert!(
                body_cap_override.unwrap() >= 16 * 1024,
                "shrink must floor at 16 KiB; got {body_cap_override:?}"
            );
        }
        other => panic!("expected Retry, got {other:?}"),
    }
}
