use super::body_cap::{
    collapse_messages_to_marker, fit_body_under_cap, truncate_last_user_message_to_cap,
    BodyFitMode, TRUNCATION_MARKER_BUDGET, TRUNCATION_MARKER_PREFIX,
};
use crate::anthropic::AnthropicProvider;
use crate::AnthropicConfig;
use serde_json::json;

fn body_with_user_text(text: &str) -> Vec<u8> {
    let body = json!({
        "model": "aura-claude-opus-4-7",
        "system": [{"type": "text", "text": "system prompt"}],
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "earlier turn"}
                ]
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "ok"}
                ]
            },
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text}
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    });
    serde_json::to_vec(&body).expect("serialize")
}

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

/// When the last user message alone is small but the *history* is
/// huge (long agent loops, many `tool_result`s), the ladder must
/// drop oldest message pairs rather than damaging the user's
/// latest turn.
#[test]
fn fit_body_under_cap_drops_oldest_pairs_when_history_is_the_bulk() {
    // Construct a body with ~50 KB of OLD history and a small new
    // last user message. The truncation step alone can't help
    // because the small last user message is already small.
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": [
                {"type": "text", "text": "Z".repeat(2_500)}
            ]
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "what's next?"}
        ]
    }));

    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-opus-4-7",
        "system": [{"type": "text", "text": "system prompt"}],
        "messages": messages,
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    let cap = body.len() / 3;
    let (capped, dropped, mode) = fit_body_under_cap(&body, cap);

    assert!(
        capped.len() <= cap + TRUNCATION_MARKER_BUDGET,
        "capped body ({}) must fit under cap+marker ({})",
        capped.len(),
        cap + TRUNCATION_MARKER_BUDGET
    );
    assert!(
        dropped > 0,
        "history-drop ladder must have dropped messages"
    );
    assert!(
        mode == BodyFitMode::DroppedOldestPairs || mode == BodyFitMode::Collapsed,
        "expected history-drop or collapse, got {mode:?}"
    );

    // The latest user message must still be present and untouched.
    let parsed: serde_json::Value = serde_json::from_slice(&capped).unwrap();
    let last = parsed["messages"]
        .as_array()
        .and_then(|a| a.last())
        .expect("at least one message after cap");
    assert_eq!(
        last["role"].as_str().unwrap(),
        "user",
        "the last message after cap must still be the user's latest turn"
    );
}

/// Image payloads are the dominant cause of cap overflow (the real
/// incident was a ~1.5 MB body from two attached images against a
/// 512 KB cap). Stubbing older images must fit the body without
/// dropping any history, and must leave the last message's image
/// intact so the current turn still reaches the model.
#[test]
fn fit_body_under_cap_stubs_older_images_before_dropping_history() {
    let body = serde_json::to_vec(&json!({
            "model": "aura-claude-opus-4-7",
            "system": [{"type": "text", "text": "system prompt"}],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "make the homepage look like this"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "A".repeat(200_000)}}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "got it, here is my plan"}]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "also use this reference"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "B".repeat(10_000)}}
                    ]
                }
            ],
            "max_tokens": 1024,
            "stream": true,
        }))
        .unwrap();

    let cap = 50 * 1024;
    let (capped, dropped, mode) = fit_body_under_cap(&body, cap);

    assert_eq!(mode, BodyFitMode::StubbedOlderImages, "got {mode:?}");
    assert_eq!(
        dropped, 0,
        "no history may be dropped when stubbing suffices"
    );
    assert!(
        capped.len() <= cap,
        "body ({}) must fit cap ({cap})",
        capped.len()
    );

    let parsed: serde_json::Value = serde_json::from_slice(&capped).unwrap();
    let msgs = parsed["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "full history preserved");

    let first_blocks = msgs[0]["content"].as_array().unwrap();
    assert!(
        first_blocks
            .iter()
            .all(|b| b["type"].as_str() != Some("image")),
        "older image must be stubbed out"
    );
    assert!(
        first_blocks
            .iter()
            .any(|b| b["type"].as_str() == Some("text")
                && b["text"].as_str().unwrap().contains("make the homepage")),
        "original text of the first turn must survive"
    );

    let last_blocks = msgs[2]["content"].as_array().unwrap();
    assert!(
        last_blocks
            .iter()
            .any(|b| b["type"].as_str() == Some("image")),
        "the current turn's image must be preserved"
    );
}

/// Regression for the production 400 "messages.0.content.0:
/// unexpected `tool_use_id` found in `tool_result` blocks": the
/// pair-drop ladder removed the assistant message carrying the
/// `tool_use` while the following user message (now at the front)
/// still opened with its `tool_result`.
#[test]
fn fit_body_under_cap_never_leaves_orphan_tool_result_at_front() {
    // Image-heavy first user turn (mirrors the real incident where
    // two attached images pushed the body over the cap), then
    // several tool-call rounds.
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "make the homepage look like this"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "A".repeat(120_000)}}
        ]
    })];
    for i in 0..6 {
        let id = format!("toolu_{i}");
        messages.push(json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": id, "name": "get_project", "input": {}}
            ]
        }));
        messages.push(json!({
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": id, "content": "Z".repeat(30_000)}
            ]
        }));
    }

    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-opus-4-7",
        "system": [{"type": "text", "text": "system prompt"}],
        "messages": messages,
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    // Over the post-image-stub size (~180 KB of tool results) so the
    // ladder must proceed past image stubbing into pair drops.
    let cap = 100 * 1024;
    let (capped, dropped, mode) = fit_body_under_cap(&body, cap);
    assert!(dropped > 0, "must have dropped pairs, got mode {mode:?}");

    let parsed: serde_json::Value = serde_json::from_slice(&capped).unwrap();
    let msgs = parsed["messages"].as_array().expect("messages array");

    // Anthropic's positional rule: every tool_result in message i
    // must reference a tool_use in message i-1.
    for (i, msg) in msgs.iter().enumerate() {
        let Some(blocks) = msg["content"].as_array() else {
            continue;
        };
        for block in blocks {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            let id = block["tool_use_id"].as_str().unwrap();
            let paired = i > 0
                && msgs[i - 1]["content"].as_array().is_some_and(|prev| {
                    prev.iter().any(|b| {
                        b["type"].as_str() == Some("tool_use") && b["id"].as_str() == Some(id)
                    })
                });
            assert!(
                paired,
                "tool_result {id} at message {i} has no tool_use in the previous message"
            );
        }
    }

    // The first message must stay a non-empty user message.
    let first = msgs.first().expect("at least one message");
    assert_eq!(first["role"].as_str(), Some("user"));
    assert!(!first["content"].as_array().unwrap().is_empty());
}

#[test]
fn fit_body_under_cap_collapses_when_single_user_message_too_big() {
    // One enormous user message and no history — only fallback is
    // collapse. The ladder still must NEVER return an oversized
    // body.
    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-opus-4-7",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Q".repeat(80_000)}
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    let cap = 8 * 1024;
    let (capped, _dropped, mode) = fit_body_under_cap(&body, cap);
    assert!(
        capped.len() <= cap + TRUNCATION_MARKER_BUDGET * 4,
        "even the worst-case fallback must produce a body ({}) at or near cap ({})",
        capped.len(),
        cap
    );
    // Either the truncation marker or the collapse marker is fine
    // here — both keep the user's content recoverable.
    assert!(
        matches!(
            mode,
            BodyFitMode::TruncatedLastUser | BodyFitMode::Collapsed
        ),
        "expected truncated or collapsed; got {mode:?}"
    );
}

#[test]
fn collapse_salvages_instruction_and_tool_result_from_tool_loop() {
    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-fable-5-1",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Update the dashboard totals"}
                ]
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "read_file",
                        "input": {"path": "dashboard.tsx"}
                    }
                ]
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "The current total is computed on line 42"
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "A".repeat(20_000)
                        }
                    }
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    let (collapsed, _dropped, mode) = fit_body_under_cap(&body, 8 * 1024);
    assert_eq!(mode, BodyFitMode::Collapsed);
    let parsed: serde_json::Value = serde_json::from_slice(&collapsed).unwrap();
    let text = parsed["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("collapsed user text");

    assert!(text.starts_with(TRUNCATION_MARKER_PREFIX));
    assert!(text.contains("Update the dashboard totals"));
    assert!(text.contains("The current total is computed on line 42"));
    assert!(!text.contains("original_len=0,kept=0"));
}

#[test]
fn collapse_never_emits_an_empty_truncation_marker() {
    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-fable-5-1",
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    let collapsed = collapse_messages_to_marker(&body, 8 * 1024);
    let parsed: serde_json::Value = serde_json::from_slice(&collapsed).unwrap();
    let text = parsed["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("collapsed user text");

    assert!(text.starts_with(TRUNCATION_MARKER_PREFIX));
    assert!(text.contains("no recoverable text"));
    assert!(!text.contains("original_len=0,kept=0"));
}
