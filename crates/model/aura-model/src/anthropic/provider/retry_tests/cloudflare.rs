use super::support::{cloudflare_block, failed, policy, retry_body_cap, DEFAULT_BODY_CAP};
use crate::anthropic::provider::retry::{classify_retry_action, RetryAction};

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
            &policy(8),
            &failed(attempt, 1, "primary", DEFAULT_BODY_CAP),
            &mut last_err,
        );
        assert!(
            matches!(action, RetryAction::Retry { .. }),
            "attempt {attempt} should still retry the Cloudflare block"
        );
    }

    let exhausted = classify_retry_action(
        &err,
        &policy(8),
        &failed(3, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
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
        &policy(8),
        &failed(0, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    let first_cap = retry_body_cap(first, "Cloudflare retry must set a tighter body cap");
    assert!(
        first_cap < 524_288,
        "shrunk cap ({first_cap}) must be strictly smaller than the previous one (524288)"
    );
    // 1/2 of 524288 = 262144.
    assert_eq!(first_cap, 262_144, "first shrink should halve the cap");

    // Second Cloudflare hit: cap shrinks again from the previous override.
    let second = classify_retry_action(
        &err,
        &policy(8),
        &failed(1, 1, "primary", first_cap),
        &mut last_err,
    );
    let second_cap = retry_body_cap(second, "Cloudflare retry must keep tightening the cap");
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
        &policy(8),
        &failed(0, 1, "primary", DEFAULT_BODY_CAP),
        &mut last_err,
    );
    let cap = retry_body_cap(action, "Cloudflare retry must set a tighter body cap");

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
    let mut current_cap = DEFAULT_BODY_CAP;

    for attempt in 0..3 {
        let action = classify_retry_action(
            &err,
            &policy(8),
            &failed(attempt, 1, "primary", current_cap),
            &mut last_err,
        );
        current_cap = retry_body_cap(action, "Cloudflare retry must set a tighter body cap");
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
        &policy(8),
        &failed(0, 1, "primary", 8 * 1024), // already below the floor
        &mut last_err,
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
