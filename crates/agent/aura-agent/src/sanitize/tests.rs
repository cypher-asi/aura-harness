
use super::*;

#[test]
fn test_remove_empty_messages() {
    let mut messages = vec![
        Message::user("hello"),
        Message::new(Role::User, vec![ContentBlock::text("")]),
        Message::assistant("world"),
    ];
    remove_empty_messages(&mut messages);
    assert_eq!(messages.len(), 2);
}

#[test]
fn test_merge_consecutive_same_role() {
    let mut messages = vec![
        Message::user("hello"),
        Message::user("world"),
        Message::assistant("hi"),
    ];
    merge_consecutive_same_role(&mut messages);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content.len(), 2);
}

#[test]
fn test_ensure_starts_with_user() {
    let mut messages = vec![Message::assistant("hi")];
    ensure_starts_with_user(&mut messages);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages.len(), 2);
}

#[test]
fn test_fix_orphan_tool_results() {
    let mut messages = vec![
        Message::user("go"),
        Message::new(
            Role::User,
            vec![ContentBlock::tool_result(
                "orphan_id",
                ToolResultContent::text("result"),
                false,
            )],
        ),
    ];
    fix_orphan_tool_results(&mut messages);
    assert_eq!(messages.len(), 1);
}

#[test]
fn test_validate_and_repair_full_pipeline() {
    let mut messages = vec![
        Message::assistant("oops first"),
        Message::user(""),
        Message::user("hello"),
        Message::user("world"),
    ];
    validate_and_repair(&mut messages);
    assert_eq!(messages[0].role, Role::User);
    assert!(messages.len() >= 2);
}

fn make_assistant_with_tool_use(tool_ids: &[&str]) -> Message {
    let mut blocks = vec![ContentBlock::text("I'll help")];
    for id in tool_ids {
        blocks.push(ContentBlock::ToolUse {
            id: id.to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "a.rs"}),
        });
    }
    Message::new(Role::Assistant, blocks)
}

fn make_tool_result_msg(tool_ids: &[&str]) -> Message {
    let results: Vec<(String, ToolResultContent, bool)> = tool_ids
        .iter()
        .map(|id| (id.to_string(), ToolResultContent::text("ok"), false))
        .collect();
    Message::tool_results(results)
}

#[test]
fn test_warning_between_tool_use_and_result_gets_merged() {
    let mut messages = vec![
        Message::user("do something"),
        make_assistant_with_tool_use(&["t1", "t2"]),
        Message::user("WARNING: blocked"),
        make_tool_result_msg(&["t1", "t2"]),
    ];
    validate_and_repair(&mut messages);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[2].role, Role::User);

    let result_ids: HashSet<String> = messages[2]
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                Some(tool_use_id.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(result_ids.contains("t1"));
    assert!(result_ids.contains("t2"));
}

#[test]
fn test_tool_use_without_any_result_gets_synthetic() {
    let mut messages = vec![
        Message::user("do something"),
        make_assistant_with_tool_use(&["t1"]),
    ];
    validate_and_repair(&mut messages);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].role, Role::User);

    let has_result = messages[2]
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t1"));
    assert!(has_result, "should have synthetic tool_result for t1");
}

#[test]
fn test_multiple_tool_uses_partial_results_get_synthetic() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1", "t2", "t3"]),
        make_tool_result_msg(&["t1"]),
    ];
    validate_and_repair(&mut messages);

    let result_ids: HashSet<String> = messages[2]
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                Some(tool_use_id.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(result_ids.contains("t1"));
    assert!(result_ids.contains("t2"), "synthetic result for t2");
    assert!(result_ids.contains("t3"), "synthetic result for t3");
}

#[test]
fn test_build_warning_between_tool_use_and_result() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1"]),
        Message::user("Build check failed with 3 error(s)"),
        Message::user("NOTE: first write checkpoint"),
        make_tool_result_msg(&["t1"]),
    ];
    validate_and_repair(&mut messages);

    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[2].role, Role::User);

    let has_result = messages[2]
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t1"));
    assert!(
        has_result,
        "tool_result should be in the merged user message"
    );
}

#[test]
fn test_multi_turn_tool_use_all_paired() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1"]),
        make_tool_result_msg(&["t1"]),
        make_assistant_with_tool_use(&["t2"]),
        make_tool_result_msg(&["t2"]),
    ];
    validate_and_repair(&mut messages);

    assert_eq!(messages.len(), 5);
    for i in [1, 3] {
        assert_eq!(messages[i].role, Role::Assistant);
        assert_eq!(messages[i + 1].role, Role::User);
    }
}

/// Returns true when every `tool_result` in `messages` references a
/// `tool_use` in the immediately previous message — the exact rule the
/// Anthropic API enforces with a 400 otherwise.
fn positional_pairing_holds(messages: &[Message]) -> bool {
    messages.iter().enumerate().all(|(i, msg)| {
        msg.content.iter().all(|block| {
            let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                return true;
            };
            i > 0
                && messages[i - 1]
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id))
        })
    })
}

/// Regression for the production 400: "messages.0.content.0: unexpected
/// `tool_use_id` found in `tool_result` blocks". A transcript whose FIRST
/// message starts with a `tool_result` used to survive sanitization as
/// long as the matching `tool_use` id existed anywhere else (e.g. later,
/// after compaction rearranged history).
#[test]
fn test_leading_tool_result_with_use_later_is_repaired() {
    let mut messages = vec![
        make_tool_result_msg(&["t1"]),
        make_assistant_with_tool_use(&["t1"]),
        make_tool_result_msg(&["t1"]),
    ];
    validate_and_repair(&mut messages);

    assert!(
        positional_pairing_holds(&messages),
        "repaired transcript must satisfy the positional rule: {messages:?}"
    );
    assert_eq!(messages[0].role, Role::User);
    assert!(
        !matches!(messages[0].content[0], ContentBlock::ToolResult { .. }),
        "first message must not start with a tool_result"
    );
}

/// A `tool_result` whose `tool_use` sits two messages back (non-adjacent)
/// must be dropped, and the now-unpaired `tool_use` gets a synthetic
/// result from pass 4.
#[test]
fn test_non_adjacent_tool_result_is_repaired() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1"]),
        make_tool_result_msg(&["other"]),
        make_assistant_with_tool_use(&["t2"]),
        make_tool_result_msg(&["t1"]),
    ];
    validate_and_repair(&mut messages);

    assert!(
        positional_pairing_holds(&messages),
        "repaired transcript must satisfy the positional rule: {messages:?}"
    );
}

/// A fully valid transcript (including parallel tool calls) must pass
/// through sanitization untouched.
#[test]
fn test_valid_transcript_untouched() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1", "t2"]),
        make_tool_result_msg(&["t1", "t2"]),
        Message::assistant("done"),
    ];
    let before = serde_json::to_value(&messages).unwrap();
    validate_and_repair(&mut messages);
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
}

#[test]
fn test_trailing_assistant_tool_use_gets_result() {
    let mut messages = vec![
        Message::user("task"),
        make_assistant_with_tool_use(&["t1"]),
        make_tool_result_msg(&["t1"]),
        make_assistant_with_tool_use(&["t2"]),
    ];
    validate_and_repair(&mut messages);

    assert!(messages.len() >= 5);
    let last_user = &messages[4];
    assert_eq!(last_user.role, Role::User);
    let has_t2 = last_user
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t2"));
    assert!(has_t2, "trailing tool_use should get synthetic result");
}
