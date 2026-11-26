use super::support::body_with_user_text;
use crate::anthropic::provider::body_cap::{
    truncate_last_user_message_to_cap, TRUNCATION_MARKER_BUDGET, TRUNCATION_MARKER_PREFIX,
};
use serde_json::json;

#[test]
fn truncate_returns_err_when_no_messages_array_present() {
    let body = serde_json::to_vec(&json!({"model": "x"})).unwrap();
    let err = truncate_last_user_message_to_cap(&body, 100).unwrap_err();
    assert!(err.contains("messages"), "got: {err}");
}

#[test]
fn truncate_returns_err_when_no_user_message_present() {
    let body = serde_json::to_vec(&json!({
        "model": "x",
        "messages": [
            {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
        ]
    }))
    .unwrap();
    let err = truncate_last_user_message_to_cap(&body, 100).unwrap_err();
    assert!(err.contains("no user message"), "got: {err}");
}

#[test]
fn truncate_returns_err_when_cap_too_small_for_marker() {
    let big = "X".repeat(10_000);
    let body = body_with_user_text(&big);
    // cap of 16 bytes is smaller than just the JSON envelope, let
    // alone the marker overhead.
    let err = truncate_last_user_message_to_cap(&body, 16).unwrap_err();
    assert!(err.contains("emergency body cap"), "got: {err}");
}

#[test]
fn truncate_keeps_the_truncation_marker_and_shrinks_body() {
    let big = "abcdefghij".repeat(10_000); // ~100KB of user text
    let body = body_with_user_text(&big);
    let original_len = body.len();
    let cap = original_len / 4; // force a meaningful truncation

    let new_body = truncate_last_user_message_to_cap(&body, cap)
        .expect("truncation should succeed when cap is reasonable");

    assert!(
        new_body.len() <= cap + TRUNCATION_MARKER_BUDGET,
        "new body ({} B) should be at or below cap+marker_budget ({} B)",
        new_body.len(),
        cap + TRUNCATION_MARKER_BUDGET
    );
    assert!(
        new_body.len() < original_len,
        "new body must be smaller than the original"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&new_body).unwrap();
    let last_user_text = parsed["messages"][2]["content"][0]["text"]
        .as_str()
        .expect("last user message text");
    assert!(
        last_user_text.starts_with(TRUNCATION_MARKER_PREFIX),
        "truncated text must start with the canonical marker; got: {}",
        &last_user_text[..last_user_text.len().min(80)]
    );
    assert!(
        last_user_text.contains(&format!("original_len={}", big.len())),
        "marker should record the original length"
    );

    // Earlier user message must be preserved verbatim (truncation
    // is targeted, not global).
    assert_eq!(
        parsed["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap(),
        "earlier turn"
    );
}

#[test]
fn truncate_preserves_waf_safe_unicode_escaping() {
    // Regression test for the silent WAF-bypass regression where
    // the emergency cap re-serializes the body via the default
    // `serde_json::to_vec` path, which decodes every `\u0026`,
    // `\u005b`, etc. back into a literal byte and exposes the
    // raw code-pattern characters to Cloudflare. The dev-loop
    // bootstrap ALWAYS hits the cap, so this regression made the
    // bypass useless on exactly the hot path it was meant to fix.
    // See: https://github.com/zeronetworking/aura — debug session
    // 95fd5c, hypothesis H_WAF_UNICODE_ESCAPE.
    let big = "if x[0] & y == 1 { return (a + b); }".repeat(800); // ~30 KB
    let body = body_with_user_text(&big);
    let cap = body.len() / 3;

    let new_body = truncate_last_user_message_to_cap(&body, cap)
        .expect("truncation should succeed when cap is reasonable");

    let new_bytes_str = String::from_utf8_lossy(&new_body);
    assert!(
        new_bytes_str.contains("\\u0026"),
        "truncated body must keep & escaped as \\u0026 on the wire"
    );
    assert!(
        new_bytes_str.contains("\\u003d\\u003d"),
        "truncated body must keep == escaped as \\u003d\\u003d on the wire"
    );
    assert!(
        !new_bytes_str.contains("x[0] & y"),
        "truncated body must not leak the literal `x[0] & y` substring on the wire: \
             this is the exact pattern the Cloudflare WAF was matching against. \
             Got fragment: {}",
        &new_bytes_str[..new_bytes_str.len().min(400)]
    );

    // Sanity: the body still parses back to the original content.
    let parsed: serde_json::Value = serde_json::from_slice(&new_body).unwrap();
    let last_user_text = parsed["messages"][2]["content"][0]["text"]
        .as_str()
        .expect("last user message text");
    assert!(
        last_user_text.contains("x[0] & y == 1"),
        "after JSON parsing the model must still see literal `x[0] & y == 1`; \
             escaping is a wire-only concern"
    );
}
