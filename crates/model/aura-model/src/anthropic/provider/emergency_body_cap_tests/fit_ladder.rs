use super::support::{
    assert_every_tool_result_follows_its_tool_use, body_with_image_then_tool_rounds,
    body_with_older_and_current_image,
};
use crate::anthropic::provider::body_cap::{
    fit_body_under_cap, BodyFitMode, TRUNCATION_MARKER_BUDGET,
};
use serde_json::json;

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
    let body = body_with_older_and_current_image();

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
    let body = body_with_image_then_tool_rounds();

    // Over the post-image-stub size (~180 KB of tool results) so the
    // ladder must proceed past image stubbing into pair drops.
    let cap = 100 * 1024;
    let (capped, dropped, mode) = fit_body_under_cap(&body, cap);
    assert!(dropped > 0, "must have dropped pairs, got mode {mode:?}");

    let parsed: serde_json::Value = serde_json::from_slice(&capped).unwrap();
    let msgs = parsed["messages"].as_array().expect("messages array");

    assert_every_tool_result_follows_its_tool_use(msgs);

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
