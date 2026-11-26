//! Positional `tool_use` / `tool_result` pairing guards and tool id
//! sanitizing.

use super::support::*;
use crate::anthropic::api_types::{ApiContent, ApiMessage};
use crate::anthropic::convert::{
    api_tool_id, convert_messages_to_api, drop_misplaced_tool_results,
};
use crate::anthropic::ReasoningOrigin;
use crate::{ContentBlock, Message, Role, ToolResultContent};

// ============================================================================
// drop_misplaced_tool_results / inject_missing_tool_results — wire-level
// guards for Anthropic's positional rule: "Each `tool_result` block must
// have a corresponding `tool_use` block in the previous message."
// Regression for the production 400 at messages.0.content.0.
// ============================================================================

fn tool_use(id: &str) -> ApiContent {
    ApiContent::ToolUse {
        id: id.to_string(),
        name: "fs.read".to_string(),
        input: serde_json::json!({"path": "a.rs"}),
    }
}

/// Verify the rule Anthropic enforces server-side.
fn positional_pairing_holds(api_messages: &[ApiMessage]) -> bool {
    api_messages.iter().enumerate().all(|(i, msg)| {
        msg.content.iter().all(|block| {
            let ApiContent::ToolResult { tool_use_id, .. } = block else {
                return true;
            };
            i > 0
                && api_messages[i - 1]
                    .content
                    .iter()
                    .any(|b| matches!(b, ApiContent::ToolUse { id, .. } if id == tool_use_id))
        })
    })
}

#[test]
fn guard_drops_tool_result_leading_first_message() {
    let mut api_messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_front", "stranded", false)],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![text_block("hello")],
        },
    ];

    drop_misplaced_tool_results(&mut api_messages);

    assert!(positional_pairing_holds(&api_messages));
    assert_eq!(
        api_messages.len(),
        1,
        "the emptied leading user message must be removed entirely"
    );
    assert_eq!(api_messages[0].role, "assistant");
}

#[test]
fn guard_drops_non_adjacent_tool_result_but_keeps_valid_one() {
    let mut api_messages = vec![
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![tool_use("toolu_A")],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![
                tool_result("toolu_A", "valid", false),
                tool_result("toolu_B", "misplaced: use is in a LATER message", false),
            ],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![tool_use("toolu_B")],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_B", "valid too", false)],
        },
    ];

    drop_misplaced_tool_results(&mut api_messages);

    assert!(positional_pairing_holds(&api_messages));
    assert_eq!(
        count_tool_results_with_id(&api_messages, "toolu_A").len(),
        1
    );
    let b_survivors = count_tool_results_with_id(&api_messages, "toolu_B");
    assert_eq!(
        b_survivors.len(),
        1,
        "only the adjacent toolu_B result survives"
    );
    assert_eq!(b_survivors[0].0, "valid too");
}

#[test]
fn guard_noop_on_valid_transcript() {
    let mut api_messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![text_block("task")],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![
                text_block("on it"),
                tool_use("toolu_1"),
                tool_use("toolu_2"),
            ],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![
                tool_result("toolu_1", "a", false),
                tool_result("toolu_2", "b", false),
            ],
        },
    ];
    let before = serde_json::to_value(&api_messages).unwrap();

    drop_misplaced_tool_results(&mut api_messages);

    assert_eq!(serde_json::to_value(&api_messages).unwrap(), before);
}

/// End-to-end through `convert_messages_to_api`: a transcript whose first
/// message starts with a stranded `tool_result` (the exact shape behind the
/// production "messages.0.content.0: unexpected `tool_use_id`" 400) must be
/// repaired before hitting the wire — the orphan is dropped and the
/// later `tool_use` it pointed at gets a synthetic result injected.
#[test]
fn convert_repairs_leading_orphan_tool_result_end_to_end() {
    let messages = vec![
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_019R".to_string(),
                content: ToolResultContent::Text("stranded result".to_string()),
                is_error: false,
            }],
        ),
        Message::new(Role::Assistant, vec![ContentBlock::text("hi")]),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_019R".to_string(),
                name: "list_files".to_string(),
                input: serde_json::json!({}),
            }],
        ),
    ];

    let api_messages = convert_messages_to_api(&messages, true, ReasoningOrigin::Anthropic);

    assert!(
        positional_pairing_holds(&api_messages),
        "outbound request must satisfy the positional rule: {api_messages:?}"
    );
    let survivors = count_tool_results_with_id(&api_messages, "toolu_019R");
    assert_eq!(
        survivors.len(),
        1,
        "the trailing tool_use must get a synthetic result so it isn't unpaired"
    );
    assert_eq!(survivors[0].1, Some(true), "synthetic result is an error");
}

#[test]
fn invalid_tool_ids_are_sanitized_on_both_sides_of_the_pair() {
    let messages = vec![
        Message::user("go"),
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "call.1/x".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "toolu_01-Ab".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                },
            ],
        ),
        Message::tool_results(vec![
            (
                "call.1/x".to_string(),
                ToolResultContent::Text("a".to_string()),
                false,
            ),
            (
                "toolu_01-Ab".to_string(),
                ToolResultContent::Text("b".to_string()),
                false,
            ),
        ]),
    ];
    let api = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);
    let use_ids: Vec<&str> = api[1]
        .content
        .iter()
        .filter_map(|b| match b {
            ApiContent::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<&str> = api[2]
        .content
        .iter()
        .filter_map(|b| match b {
            ApiContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(use_ids, ["call_1_x", "toolu_01-Ab"]);
    assert_eq!(result_ids, ["call_1_x", "toolu_01-Ab"]);
}

#[test]
fn api_tool_id_maps_only_invalid_characters() {
    assert_eq!(api_tool_id("toolu_01ABC-xyz"), "toolu_01ABC-xyz");
    assert_eq!(api_tool_id("call.1/x"), "call_1_x");
    assert_eq!(api_tool_id("fc:é"), "fc__");
    assert_eq!(api_tool_id(""), "toolu_empty");
}
