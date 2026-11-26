use crate::messages::{
    apply_message_summary, compact_messages, estimate_message_chars, forced_summary_input,
    CompactionAction, CompactionInput, CompactionPolicy, SummaryOutput,
};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};

#[test]
fn forced_summary_covers_the_middle_of_any_size() {
    let mut messages = vec![Message::user("goal")];
    for i in 0..12 {
        messages.push(Message::assistant(format!("step {i}")));
        messages.push(Message::user(format!("ok {i}")));
    }

    let input = forced_summary_input(&messages).expect("long enough to summarize");

    assert_eq!(input.compact_start, 1);
    assert!(input.compact_end < messages.len());
    assert_eq!(input.compactable_messages.len(), input.compact_end - 1);
    assert_eq!(input.recent_tail.len(), messages.len() - input.compact_end);
    assert!(forced_summary_input(&messages[..3]).is_none());
}

fn tool_use_with_id(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            id,
            "read_file",
            serde_json::json!({"path": "big.rs"}),
        )],
    }
}

fn tool_result_with_id(id: &str, content: impl Into<String>) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::tool_result(
            id,
            ToolResultContent::Text(content.into()),
            false,
        )],
    }
}

fn summary_pressure_policy(before_chars: usize) -> CompactionPolicy {
    CompactionPolicy {
        raw_message_bytes: Some(before_chars),
        request_body_cap_bytes: Some(8_000),
        summary_at: 0.85,
        ..CompactionPolicy::default()
    }
}

fn long_summary_fixture() -> Vec<Message> {
    let mut messages = vec![Message::user("anchor")];
    for i in 0..20 {
        if i % 2 == 0 {
            messages.push(Message::assistant("A".repeat(10_000)));
        } else {
            messages.push(Message::user("B".repeat(10_000)));
        }
    }
    messages.push(Message::assistant("recent assistant tail"));
    messages.push(Message::user("recent user tail"));
    messages
}

#[test]
fn summary_action_preserves_recent_tail() {
    let mut messages = long_summary_fixture();
    let before_chars = estimate_message_chars(&messages);

    let report = compact_messages(CompactionInput {
        messages: &mut messages,
        policy: summary_pressure_policy(before_chars),
    });

    let CompactionAction::NeedsSummary(input) = report.action else {
        panic!("expected summary escalation, got {:?}", report.action);
    };
    assert_eq!(input.recent_tail.len(), 2);
    assert_eq!(input.recent_tail[0].text_content(), "recent assistant tail");
    assert_eq!(input.recent_tail[1].text_content(), "recent user tail");
    assert_eq!(
        messages[messages.len() - 2].text_content(),
        "recent assistant tail"
    );
    assert_eq!(
        messages[messages.len() - 1].text_content(),
        "recent user tail"
    );
}

#[test]
fn summary_action_preserves_tool_adjacency() {
    let mut messages = vec![Message::user("anchor")];
    for i in 0..16 {
        if i % 2 == 0 {
            messages.push(Message::assistant("A".repeat(10_000)));
        } else {
            messages.push(Message::user("B".repeat(10_000)));
        }
    }
    messages.push(tool_use_with_id("toolu_boundary"));
    messages.push(tool_result_with_id("toolu_boundary", "result tail"));
    messages.push(Message::assistant("latest"));

    let before_chars = estimate_message_chars(&messages);
    let report = compact_messages(CompactionInput {
        messages: &mut messages,
        policy: summary_pressure_policy(before_chars),
    });

    let CompactionAction::NeedsSummary(input) = report.action else {
        panic!("expected summary escalation, got {:?}", report.action);
    };
    assert!(
        matches!(
            input.recent_tail.first().and_then(|message| message.content.first()),
            Some(ContentBlock::ToolUse { id, .. }) if id == "toolu_boundary"
        ),
        "summary range must not split a tool_use/tool_result pair"
    );
    assert!(
        matches!(
            input.recent_tail.get(1).and_then(|message| message.content.first()),
            Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "toolu_boundary"
        ),
        "paired tool_result should stay adjacent to its tool_use"
    );
}

#[test]
fn apply_summary_reduces_message_bytes() {
    let mut messages = vec![
        Message::user("anchor"),
        Message::assistant("middle assistant ".repeat(2_000)),
        Message::user("middle user ".repeat(2_000)),
        Message::assistant("recent tail"),
    ];
    let before_chars = estimate_message_chars(&messages);

    let report = apply_message_summary(
        &mut messages,
        SummaryOutput::Messages {
            text: "The compacted middle history discussed prior implementation details."
                .to_string(),
            compact_start: 1,
            compact_end: 3,
        },
    );

    assert!(report.after_chars < before_chars);
    assert!(report.reduced());
    assert_eq!(messages.len(), 3);
    assert!(messages[1]
        .text_content()
        .contains("compacted middle history"));
    assert_eq!(messages[2].text_content(), "recent tail");
}

#[test]
fn summary_action_only_at_high_pressure() {
    // Build a fixture that's well below the summary_at threshold:
    // 200K context window, 80K context tokens (40% pressure) — far
    // below the 0.85 default. Compaction may still apply a tier,
    // but it must NOT escalate to NeedsSummary.
    let mut messages = long_summary_fixture();
    let report = compact_messages(CompactionInput {
        messages: &mut messages,
        policy: CompactionPolicy {
            max_context_tokens: Some(200_000),
            current_context_tokens: Some(80_000),
            summary_at: 0.85,
            ..CompactionPolicy::default()
        },
    });

    assert!(
        !matches!(report.action, CompactionAction::NeedsSummary(_)),
        "summary escalation should wait for policy.summary_at"
    );
}
