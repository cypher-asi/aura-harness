use super::support::body_with_user_text;
use crate::anthropic::provider::body_cap::{TRUNCATION_MARKER_BUDGET, TRUNCATION_MARKER_PREFIX};
use crate::anthropic::AnthropicProvider;
use crate::AnthropicConfig;

#[test]
fn apply_body_cap_disabled_passthrough() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    config.emergency_body_cap_bytes = 0;
    let provider = AnthropicProvider::new(config).unwrap();

    let body = body_with_user_text(&"X".repeat(10_000));
    let original = body.clone();
    let cap = provider.effective_body_cap(None);
    let returned = provider.apply_body_cap("aura-claude-opus-4-7", body, cap);

    assert_eq!(returned, original, "cap=0 must be a passthrough");
}

#[test]
fn apply_body_cap_under_threshold_passthrough() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    config.emergency_body_cap_bytes = 1_000_000;
    let provider = AnthropicProvider::new(config).unwrap();

    let body = body_with_user_text("small");
    let original = body.clone();
    let cap = provider.effective_body_cap(None);
    let returned = provider.apply_body_cap("aura-claude-opus-4-7", body, cap);

    assert_eq!(
        returned, original,
        "body smaller than cap must be a passthrough"
    );
}

#[test]
fn apply_body_cap_truncates_when_over_threshold() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    let body = body_with_user_text(&"abcdefghij".repeat(10_000));
    config.emergency_body_cap_bytes = body.len() / 4;
    let cap = config.emergency_body_cap_bytes;
    let provider = AnthropicProvider::new(config).unwrap();

    let original_len = body.len();
    let effective = provider.effective_body_cap(None);
    let returned = provider.apply_body_cap("aura-claude-opus-4-7", body, effective);

    assert!(returned.len() < original_len);
    assert!(returned.len() <= cap + TRUNCATION_MARKER_BUDGET);

    // The wire bytes use WAF-safe Unicode escaping, so the marker's
    // `<<<` shows up as `\u003c\u003c\u003c`. Parse the body back
    // out to check the canonical marker is present in the decoded
    // message text.
    let parsed: serde_json::Value = serde_json::from_slice(&returned).unwrap();
    let last_user_text = parsed["messages"][2]["content"][0]["text"]
        .as_str()
        .expect("last user message text");
    assert!(
        last_user_text.starts_with(TRUNCATION_MARKER_PREFIX),
        "truncated body must contain the canonical marker; got: {}",
        &last_user_text[..last_user_text.len().min(80)]
    );
}

#[test]
fn effective_body_cap_honors_override_when_tighter() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    config.emergency_body_cap_bytes = 512 * 1024;
    let provider = AnthropicProvider::new(config).unwrap();

    assert_eq!(provider.effective_body_cap(None), 512 * 1024);
    assert_eq!(
        provider.effective_body_cap(Some(128 * 1024)),
        128 * 1024,
        "tighter override must win"
    );
    assert_eq!(
        provider.effective_body_cap(Some(2 * 1024 * 1024)),
        512 * 1024,
        "looser override must not raise the ceiling"
    );
}

#[test]
fn effective_body_cap_zero_disable_ignores_override() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    config.emergency_body_cap_bytes = 0;
    let provider = AnthropicProvider::new(config).unwrap();

    assert_eq!(
        provider.effective_body_cap(Some(64 * 1024)),
        0,
        "explicit 0 = disabled must dominate; the operator opted out"
    );
}

/// Regression test for the original `truncated_ok:false` symptom:
/// when only the last user message can be truncated and the cap is
/// extremely tight, the previous code would bail with
/// `"emergency body cap …B is smaller than non-content overhead"`
/// and let the oversized body go out anyway — straight into the
/// Cloudflare WAF. The ladder must always succeed.
#[test]
fn apply_body_cap_collapses_when_cap_is_tiny() {
    let mut config = AnthropicConfig::new("aura-claude-opus-4-7");
    let body = body_with_user_text(&"X".repeat(100_000)); // ~100 KB user
    config.emergency_body_cap_bytes = 4 * 1024; // tighter than the historical 24 KiB
    let cap = config.emergency_body_cap_bytes;
    let provider = AnthropicProvider::new(config).unwrap();

    let original_len = body.len();
    let returned = provider.apply_body_cap("aura-claude-opus-4-7", body, cap);

    assert!(
        returned.len() < original_len,
        "ladder must always shrink an oversized body"
    );
    assert!(
        returned.len() <= cap + TRUNCATION_MARKER_BUDGET * 4,
        "collapsed body ({} B) must be at or below cap+marker budgets ({} B). \
             This is the regression: the old code returned the unshrunk body, \
             which is exactly what triggered the Cloudflare 403s.",
        returned.len(),
        cap + TRUNCATION_MARKER_BUDGET * 4
    );
    // Should still be parseable JSON with a messages array.
    let parsed: serde_json::Value = serde_json::from_slice(&returned).unwrap();
    assert!(
        parsed["messages"].is_array(),
        "post-cap body must still be valid Anthropic schema"
    );
}
