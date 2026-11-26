use crate::messages::pressure::DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES;
use crate::messages::{
    compact_messages, effective_pressure, select_tier, CompactionConfig, CompactionInput,
    CompactionPolicy,
};
use aura_model::{ContentBlock, Message, ModelRequestKind, Role, ToolResultContent};

#[test]
fn test_select_tier_85pct() {
    let tier = select_tier(0.85);
    assert!(tier.is_some());
    let config = tier.unwrap();
    assert_eq!(
        config.preserve_recent,
        CompactionConfig::emergency().preserve_recent
    );
    assert_eq!(
        config.tool_result_max_chars,
        CompactionConfig::emergency().tool_result_max_chars
    );
}

#[test]
fn test_select_tier_below_threshold() {
    let tier = select_tier(0.10);
    assert!(tier.is_none());
}

#[test]
fn test_5_tier_selection() {
    let t = select_tier(0.90).unwrap();
    assert_eq!(t.preserve_recent, 2);
    assert_eq!(t.tool_result_max_chars, 200);

    let t = select_tier(0.85).unwrap();
    assert_eq!(t.preserve_recent, 2);

    let t = select_tier(0.75).unwrap();
    assert_eq!(t.preserve_recent, 4);
    assert_eq!(t.tool_result_max_chars, 500);

    let t = select_tier(0.70).unwrap();
    assert_eq!(t.preserve_recent, 4);

    let t = select_tier(0.65).unwrap();
    assert_eq!(t.preserve_recent, 6);
    assert_eq!(t.tool_result_max_chars, 1000);
    assert_eq!(t.text_max_chars, 1500);

    let t = select_tier(0.60).unwrap();
    assert_eq!(t.preserve_recent, 6);

    let t = select_tier(0.45).unwrap();
    assert_eq!(t.preserve_recent, 6);
    assert_eq!(t.tool_result_max_chars, 1500);
    assert_eq!(t.text_max_chars, 2000);

    let t = select_tier(0.30).unwrap();
    assert_eq!(t.preserve_recent, 6);

    let t = select_tier(0.20).unwrap();
    assert_eq!(t.preserve_recent, 8);
    assert_eq!(t.tool_result_max_chars, 3000);
    assert_eq!(t.text_max_chars, 4000);

    let t = select_tier(0.15).unwrap();
    assert_eq!(t.preserve_recent, 8);

    assert!(select_tier(0.10).is_none());
    assert!(select_tier(0.0).is_none());
}

#[test]
fn each_tier_trims_at_least_as_hard_as_the_one_below_it() {
    let tiers: Vec<CompactionConfig> = [0.15, 0.30, 0.60, 0.70, 0.85]
        .into_iter()
        .map(|pressure| select_tier(pressure).unwrap())
        .collect();
    for pair in tiers.windows(2) {
        let (lower, higher) = (pair[0], pair[1]);
        assert!(higher.tool_result_max_chars <= lower.tool_result_max_chars);
        assert!(higher.text_max_chars <= lower.text_max_chars);
        assert!(higher.preserve_recent <= lower.preserve_recent);
        assert_ne!(higher, lower);
    }
}

#[test]
fn request_kind_body_cap_contributes_pressure() {
    // Without `absolute_byte_pressure`, the `DevLoopBootstrap` body
    // cap is the only thing that can push pressure above zero on a
    // tiny conversation. Set raw_message_bytes well above the
    // 24 KiB cap and assert pressure crosses into a tier purely on
    // the cap math (raw / cap), with `max_context_tokens` and
    // `current_context_tokens` left at their defaults.
    let mut messages = vec![Message::user("small")];
    let input = CompactionInput {
        messages: &mut messages,
        policy: CompactionPolicy {
            raw_message_bytes: Some(DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES + 1),
            request_kind: Some(ModelRequestKind::DevLoopBootstrap),
            ..CompactionPolicy::default()
        },
    };

    let pressure = effective_pressure(&input);
    assert!(
        pressure >= 1.0,
        "raw bytes above the cap should saturate request_cap_pressure to 1.0; got {pressure}"
    );
    assert!(
        select_tier(pressure).is_some(),
        "saturated pressure should pick a tier"
    );

    // Sanity: a Chat continuation has no body cap, so the same
    // raw_message_bytes should NOT trip pressure on its own.
    let mut messages2 = vec![Message::user("small")];
    let chat_input = CompactionInput {
        messages: &mut messages2,
        policy: CompactionPolicy {
            raw_message_bytes: Some(DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES + 1),
            request_kind: Some(ModelRequestKind::Chat),
            ..CompactionPolicy::default()
        },
    };
    assert_eq!(
        effective_pressure(&chat_input),
        0.0,
        "Chat continuations have no body cap; raw bytes alone must not produce pressure",
    );
}

/// Index of the aged ToolResult in [`read_file_history`]: `1 + 4*2`.
const AGED_RESULT_IDX: usize = 9;

/// ~6 KB of Rust-shaped content. The fixture deliberately contains the
/// structural markers `try_signature_compact` used to look for, so any
/// future regression that re-introduces signature collapse will produce a
/// `body omitted` marker that the tests assert is absent.
fn rust_shaped_body() -> String {
    let snippet = "pub fn foo(x: u64) -> u64 {\n    let y = x + 1;\n    y * 2\n}\n\n";
    let body = snippet.repeat(120);
    assert!(body.len() >= 6_000, "fixture should be at least 6 KB");
    body
}

fn read_file_history(body: &str) -> Vec<Message> {
    let mut messages = Vec::new();
    messages.push(Message::user("anchor"));
    // A few small messages so the ToolResult is *not* in the
    // preserve_recent tail.
    for i in 0..4 {
        messages.push(Message::assistant(format!("filler-{i}")));
        messages.push(Message::user(format!("ack-{i}")));
    }
    // The aged ToolResult under test.
    messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::tool_result(
            "tu_read_publisher",
            ToolResultContent::Text(body.to_string()),
            false,
        )],
    });
    // Pad the tail to push the ToolResult above out of
    // preserve_recent.
    for i in 0..12 {
        messages.push(Message::assistant(format!("tail-asst-{i}")));
        messages.push(Message::user(format!("tail-user-{i}")));
    }
    messages
}

/// Compact `messages` against a 200K-token window holding `current_tokens`.
fn compact_at_context_tokens(messages: &mut [Message], current_tokens: u64) {
    compact_messages(CompactionInput {
        messages,
        policy: CompactionPolicy {
            max_context_tokens: Some(200_000),
            current_context_tokens: Some(current_tokens),
            ..CompactionPolicy::default()
        },
    });
}

fn aged_result_text(messages: &[Message]) -> &str {
    match &messages[AGED_RESULT_IDX].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => t,
            other => panic!("expected Text content, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// At 10% pressure, below `select_tier`'s 15% floor, no compaction tier
/// fires: a 6 KB `read_file` ToolResult must be byte-for-byte identical
/// to the original.
#[test]
fn read_file_result_survives_compaction_without_a_tier() {
    let body = rust_shaped_body();
    let mut messages = read_file_history(&body);
    assert!(matches!(
        &messages[AGED_RESULT_IDX].content[0],
        ContentBlock::ToolResult { .. }
    ));

    compact_at_context_tokens(&mut messages, 20_000);

    let t = aged_result_text(&messages);
    assert_eq!(
        t.len(),
        body.len(),
        "at 10% pressure the ToolResult must survive verbatim"
    );
    assert_eq!(t, body);
}

/// Regression: at every pressure below `summary_at` (default 0.85) the
/// worst case for a Rust-shaped 6 KB `read_file` ToolResult must be a
/// *visible* `truncate_content` marker — never a silent signature-compact
/// skeleton with `// ... body omitted ...`.
///
/// (The plan's wording said "verbatim at 40% pressure", but the
/// existing tier matrix selects the `light` tier at >= 30% with
/// `tool_result_max_chars = 3000`, so a 6 KB ToolResult is
/// always truncated above the 30% threshold. The regression we
/// actually care about is *signature-compact never fires at any
/// non-summary pressure*; that's what this test pins down.)
#[test]
fn read_file_result_survives_compaction_below_summary_pressure() {
    let body = rust_shaped_body();

    // 40%, 65%, 80% pressure: compaction may shrink the result,
    // but the worst case must be a visible `truncate_content`
    // marker — never the old signature-compact
    // `// ... body omitted ...` skeleton, and never the old
    // cached-result-shaping "Cached result reused" preamble.
    for pct in [0.40_f64, 0.65_f64, 0.80_f64] {
        let mut messages = read_file_history(&body);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let current_tokens = (200_000.0_f64 * pct) as u64;

        compact_at_context_tokens(&mut messages, current_tokens);

        let t = aged_result_text(&messages);
        if t.len() == body.len() {
            assert_eq!(t, body, "untouched results must be byte-identical");
            continue;
        }
        let prefix: String = t.chars().take(200).collect();
        assert!(
            t.contains("content truncated"),
            "shrunk ToolResult at {pct} pressure must carry a visible \
                 truncate_content marker; got prefix: {prefix}"
        );
        assert!(
            !t.contains("body omitted"),
            "shrunk ToolResult must NOT use signature-compact skeleton; \
                 got at {pct} pressure prefix: {prefix}"
        );
        assert!(
            !t.contains("Cached result reused"),
            "shrunk ToolResult must NOT use cached-result-shaping prefix; \
                 got at {pct} pressure prefix: {prefix}"
        );
    }
}
