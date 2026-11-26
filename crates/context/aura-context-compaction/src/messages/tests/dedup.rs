use crate::messages::{
    compact_messages, dedup_read_results_by_content_hash, supersede_older_reads, CompactionAction,
    CompactionInput, CompactionPolicy,
};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::Value;

// ------------------------------------------------------------------
// Phase 2: content_hash dedup of read-only tool results.
//
// The fixtures below build hand-crafted ToolUse/ToolResult message
// pairs (rather than going through the agent loop) so the dedup
// helper can be exercised in isolation. Each test pins down a
// single contract documented in the matching `assert!`.
// ------------------------------------------------------------------

fn read_tool_use(id: &str, path: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            id,
            "read_file",
            serde_json::json!({ "path": path }),
        )],
    }
}

fn read_tool_result(id: &str, body: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::tool_result(
            id,
            ToolResultContent::Text(body.to_string()),
            false,
        )],
    }
}

fn write_tool_use(id: &str, path: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            id,
            "write_file",
            serde_json::json!({ "path": path, "content": "..." }),
        )],
    }
}

fn write_tool_result(id: &str, body: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::tool_result(
            id,
            ToolResultContent::Text(body.to_string()),
            false,
        )],
    }
}

/// The `ToolUse` id of each message's first block, by position.
fn first_tool_use_ids(messages: &[Message]) -> Vec<Option<String>> {
    messages
        .iter()
        .map(|m| match m.content.first() {
            Some(ContentBlock::ToolUse { id, .. }) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// The `ToolResult` `tool_use_id` of each message's first block, by position.
fn first_tool_result_ids(messages: &[Message]) -> Vec<Option<String>> {
    messages
        .iter()
        .map(|m| match m.content.first() {
            Some(ContentBlock::ToolResult { tool_use_id, .. }) => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn dedup_collapses_older_identical_read_keeps_newest() {
    let body = "fn main() {\n    println!(\"hi\");\n}\n".repeat(20);
    let mut messages = vec![
        read_tool_use("tu_older", "src/main.rs"),
        read_tool_result("tu_older", &body),
        Message::assistant("intermediate step"),
        read_tool_use("tu_newer", "src/main.rs"),
        read_tool_result("tu_newer", &body),
    ];

    let folded = dedup_read_results_by_content_hash(&mut messages);
    assert_eq!(folded, 1, "exactly the older identical read should fold");

    // Newer (last) read stays verbatim.
    match &messages[4].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => assert_eq!(t, &body, "newest read must be verbatim"),
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }

    // Older read becomes the structured marker.
    match &messages[1].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => {
                let parsed: Value = serde_json::from_str(t)
                    .expect("marker must be valid JSON so downstream serde survives");
                assert_eq!(parsed["_redacted"], "read_file");
                assert_eq!(parsed["path"], "src/main.rs");
                assert_eq!(parsed["note"], "see later identical read");
                assert!(
                    parsed["content_hash"]
                        .as_str()
                        .is_some_and(|h| !h.is_empty()),
                    "marker must carry the content_hash of the folded read"
                );
                assert!(
                    t.len() < body.len() / 2,
                    "marker must be substantially shorter than the original"
                );
            }
            other => panic!("expected Text marker, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn dedup_does_not_touch_write_tool_results() {
    // Two write_file tool results with byte-identical text. Even
    // though their hashes match, `write_file` is not in the
    // read-only dedup set so neither result must be folded.
    let body = "write succeeded: src/lib.rs (32 bytes)";
    let mut messages = vec![
        write_tool_use("tu_w1", "src/lib.rs"),
        write_tool_result("tu_w1", body),
        Message::assistant("step"),
        write_tool_use("tu_w2", "src/lib.rs"),
        write_tool_result("tu_w2", body),
    ];

    let folded = dedup_read_results_by_content_hash(&mut messages);
    assert_eq!(folded, 0, "write_file results must never be folded");

    for idx in [1usize, 4] {
        match &messages[idx].content[0] {
            ContentBlock::ToolResult { content, .. } => match content {
                ToolResultContent::Text(t) => {
                    assert_eq!(t, body, "write_file result at idx {idx} must stay verbatim");
                }
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}

#[test]
fn dedup_preserves_message_count_and_tool_use_ids() {
    let body_a = "lots of bytes A".repeat(50);
    let body_b = "lots of bytes B".repeat(50);
    let mut messages = vec![
        read_tool_use("tu_a1", "a.rs"),
        read_tool_result("tu_a1", &body_a),
        read_tool_use("tu_b1", "b.rs"),
        read_tool_result("tu_b1", &body_b),
        Message::assistant("midway"),
        read_tool_use("tu_a2", "a.rs"),
        read_tool_result("tu_a2", &body_a),
        read_tool_use("tu_b2", "b.rs"),
        read_tool_result("tu_b2", &body_b),
    ];
    let before_len = messages.len();
    let before_use_ids = first_tool_use_ids(&messages);
    let before_result_ids = first_tool_result_ids(&messages);

    let folded = dedup_read_results_by_content_hash(&mut messages);
    assert_eq!(folded, 2, "two older reads (one per body) should fold");
    assert_eq!(messages.len(), before_len, "length must be preserved");

    let after_use_ids = first_tool_use_ids(&messages);
    let after_result_ids = first_tool_result_ids(&messages);
    assert_eq!(
        after_use_ids, before_use_ids,
        "tool_use ids must remain in the same positions"
    );
    assert_eq!(
        after_result_ids, before_result_ids,
        "tool_use_ids on ToolResult blocks must remain in the same positions"
    );
}

#[test]
fn dedup_respects_distinct_hashes() {
    // Two read_file results with *different* content. Neither
    // should be folded; both stay verbatim.
    let body_a = "alpha bytes".repeat(30);
    let body_b = "bravo bytes".repeat(30);
    let mut messages = vec![
        read_tool_use("tu_a", "a.rs"),
        read_tool_result("tu_a", &body_a),
        read_tool_use("tu_b", "b.rs"),
        read_tool_result("tu_b", &body_b),
    ];

    let folded = dedup_read_results_by_content_hash(&mut messages);
    assert_eq!(folded, 0, "distinct hashes must not be folded");

    match &messages[1].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => assert_eq!(t, &body_a),
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
    match &messages[3].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => assert_eq!(t, &body_b),
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

fn ranged_read_tool_use(id: &str, path: &str, start: u64, end: u64) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            id,
            "read_file",
            serde_json::json!({ "path": path, "start_line": start, "end_line": end }),
        )],
    }
}

fn result_text(message: &Message) -> &str {
    match &message.content[0] {
        ContentBlock::ToolResult {
            content: ToolResultContent::Text(t),
            ..
        } => t,
        other => panic!("expected a text ToolResult, got {other:?}"),
    }
}

#[test]
fn supersede_folds_an_edited_files_older_read() {
    let before = "1|fn a() {}\n2|fn b() {}".repeat(20);
    let after = "1|fn a() { edited }\n2|fn b() {}".repeat(20);
    let mut messages = vec![
        read_tool_use("tu_old", "src/lib.rs"),
        read_tool_result("tu_old", &before),
        write_tool_use("tu_w", "src/lib.rs"),
        write_tool_result("tu_w", "ok"),
        read_tool_use("tu_new", "./src/lib.rs"),
        read_tool_result("tu_new", &after),
    ];

    assert_eq!(dedup_read_results_by_content_hash(&mut messages), 0);
    assert_eq!(supersede_older_reads(&mut messages), 1);

    let marker: Value = serde_json::from_str(result_text(&messages[1])).unwrap();
    assert_eq!(marker["_redacted"], "read_file");
    assert_eq!(marker["path"], "src/lib.rs");
    assert_eq!(result_text(&messages[5]), after);
    assert_eq!(result_text(&messages[3]), "ok");
}

#[test]
fn supersede_needs_the_later_read_to_cover_the_older_window() {
    let body = "x".repeat(200);
    let mut messages = vec![
        read_tool_use("tu_whole", "a.rs"),
        read_tool_result("tu_whole", &body),
        ranged_read_tool_use("tu_part", "a.rs", 10, 20),
        read_tool_result("tu_part", &body),
    ];
    assert_eq!(supersede_older_reads(&mut messages), 0);
    assert_eq!(result_text(&messages[1]), body);

    let mut messages = vec![
        ranged_read_tool_use("tu_part", "a.rs", 10, 20),
        read_tool_result("tu_part", &body),
        ranged_read_tool_use("tu_wide", "a.rs", 1, 100),
        read_tool_result("tu_wide", &body),
    ];
    assert_eq!(supersede_older_reads(&mut messages), 1);
}

#[test]
fn supersede_ignores_cut_and_failed_later_reads() {
    let body = "x".repeat(200);
    let cut = format!(
        "{body}\n... [truncated: showing lines 1-2000 of 5000; continue with start_line=2001.]"
    );
    let mut messages = vec![
        read_tool_use("tu_old", "a.rs"),
        read_tool_result("tu_old", &body),
        read_tool_use("tu_cut", "a.rs"),
        read_tool_result("tu_cut", &cut),
        read_tool_use("tu_err", "a.rs"),
        Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result(
                "tu_err",
                ToolResultContent::Text("permission denied".into()),
                true,
            )],
        },
    ];
    assert_eq!(supersede_older_reads(&mut messages), 0);
    assert_eq!(result_text(&messages[1]), body);
}

#[test]
fn compaction_folds_superseded_reads() {
    let old = "old ".repeat(2_000);
    let new = "new ".repeat(2_000);
    let mut messages = vec![
        Message::user("task"),
        read_tool_use("tu_old", "a.rs"),
        read_tool_result("tu_old", &old),
        read_tool_use("tu_new", "a.rs"),
        read_tool_result("tu_new", &new),
    ];
    let report = compact_messages(CompactionInput {
        messages: &mut messages,
        policy: CompactionPolicy::new(Some(10_000), 9_000, 0),
    });
    assert!(!matches!(report.action, CompactionAction::None));
    assert!(result_text(&messages[2]).contains("superseded"));
}
