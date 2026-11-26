//! `dedupe_tool_results` and its use from `convert_messages_to_api`.

use super::support::*;
use crate::anthropic::api_types::{ApiContent, ApiImageSource, ApiMessage};
use crate::anthropic::convert::{convert_messages_to_api, dedupe_tool_results};
use crate::anthropic::ReasoningOrigin;
use crate::{ContentBlock, Message, Role, ToolResultContent};

#[test]
fn dedupe_keeps_last_tool_result_for_same_id() {
    let mut api_messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_X", "stale body", true)],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_X", "fresh body", false)],
        },
    ];

    dedupe_tool_results(&mut api_messages);

    let survivors = count_tool_results_with_id(&api_messages, "toolu_X");
    assert_eq!(
        survivors.len(),
        1,
        "exactly one ToolResult for toolu_X should survive"
    );
    assert_eq!(survivors[0].0, "fresh body", "last-write-wins on body");
    assert_eq!(
        survivors[0].1,
        Some(false),
        "last-write-wins on is_error too"
    );
}

#[test]
fn dedupe_preserves_position_of_first_occurrence() {
    let mut api_messages = vec![ApiMessage {
        role: "user".to_string(),
        content: vec![
            text_block("before"),
            tool_result("toolu_X", "old", false),
            text_block("between"),
            tool_result("toolu_X", "new", false),
            text_block("after"),
        ],
    }];

    dedupe_tool_results(&mut api_messages);

    assert_eq!(api_messages.len(), 1);
    let msg = &api_messages[0];
    assert_eq!(
        msg.content.len(),
        4,
        "three text blocks + one surviving ToolResult"
    );

    match &msg.content[1] {
        ApiContent::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(
                tool_use_id, "toolu_X",
                "ToolResult sits at the FIRST occurrence index (1)"
            );
            assert_eq!(
                content, "new",
                "body still comes from the LAST occurrence (last-write-wins)"
            );
        }
        other => panic!("expected ToolResult at index 1, got {other:?}"),
    }

    assert!(
        matches!(&msg.content[0], ApiContent::Text { text, .. } if text == "before"),
        "leading Text block untouched"
    );
    assert!(
        matches!(&msg.content[2], ApiContent::Text { text, .. } if text == "between"),
        "middle Text block (between the two original ToolResults) shifts up by one"
    );
    assert!(
        matches!(&msg.content[3], ApiContent::Text { text, .. } if text == "after"),
        "trailing Text block survives"
    );
}

#[test]
fn dedupe_across_messages() {
    let mut api_messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![
                tool_result("toolu_A", "a-old", false),
                text_block("more user context"),
            ],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![text_block("ack")],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_A", "a-new", false)],
        },
    ];

    dedupe_tool_results(&mut api_messages);

    assert_eq!(
        api_messages.len(),
        2,
        "third user msg was emptied by dedupe and should be dropped"
    );
    assert_eq!(api_messages[0].role, "user");
    assert_eq!(api_messages[1].role, "assistant");

    let survivors = count_tool_results_with_id(&api_messages, "toolu_A");
    assert_eq!(survivors.len(), 1);
    assert_eq!(survivors[0].0, "a-new");

    assert!(
        matches!(&api_messages[0].content[1], ApiContent::Text { text, .. } if text == "more user context"),
        "the unrelated Text block in the kept message is untouched"
    );
}

#[test]
fn dedupe_no_op_when_unique() {
    let mut api_messages = vec![ApiMessage {
        role: "user".to_string(),
        content: vec![
            tool_result("id_A", "a", false),
            tool_result("id_B", "b", false),
            tool_result("id_C", "c", false),
        ],
    }];

    dedupe_tool_results(&mut api_messages);

    assert_eq!(api_messages.len(), 1);
    assert_eq!(api_messages[0].content.len(), 3);
    let ids: Vec<&str> = api_messages[0]
        .content
        .iter()
        .filter_map(|b| match b {
            ApiContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["id_A", "id_B", "id_C"]);
}

#[test]
fn dedupe_drops_emptied_message() {
    let mut api_messages = vec![
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_X", "first body", false)],
        },
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![text_block("intermediate")],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_X", "duplicate body", false)],
        },
    ];

    dedupe_tool_results(&mut api_messages);

    assert_eq!(
        api_messages.len(),
        2,
        "third user message contained ONLY a duplicate ToolResult and must be dropped"
    );
    assert_eq!(api_messages[0].role, "user");
    assert_eq!(api_messages[1].role, "assistant");
    match &api_messages[0].content[0] {
        ApiContent::ToolResult { content, .. } => {
            assert_eq!(
                content, "duplicate body",
                "kept block carries the LAST body even though it sits at the first position"
            );
        }
        other => panic!("expected surviving ToolResult, got {other:?}"),
    }
}

/// An assistant turn (thinking, text, tool use) followed by two user turns
/// that both answer `toolu_X`, the first wrapped in an image and a text note.
fn transcript_with_mixed_blocks_and_duplicate_result() -> Vec<ApiMessage> {
    vec![
        ApiMessage {
            role: "assistant".to_string(),
            content: vec![
                ApiContent::Thinking {
                    thinking: "deliberating".to_string(),
                    signature: Some("sig-1".to_string()),
                },
                text_block("here's my plan"),
                ApiContent::ToolUse {
                    id: "toolu_X".to_string(),
                    name: "fs.write".to_string(),
                    input: serde_json::json!({"path": "out.txt"}),
                },
            ],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![
                ApiContent::Image {
                    source: ApiImageSource {
                        source_type: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: "AAA".to_string(),
                    },
                },
                tool_result("toolu_X", "first body", false),
                text_block("follow-up note"),
            ],
        },
        ApiMessage {
            role: "user".to_string(),
            content: vec![tool_result("toolu_X", "second body", false)],
        },
    ]
}

#[test]
fn dedupe_leaves_non_tool_result_blocks_alone() {
    let mut api_messages = transcript_with_mixed_blocks_and_duplicate_result();

    dedupe_tool_results(&mut api_messages);

    assert_eq!(api_messages.len(), 2);

    // Assistant message: Thinking + Text + ToolUse all preserved, in order.
    assert_eq!(api_messages[0].content.len(), 3);
    assert!(matches!(
        &api_messages[0].content[0],
        ApiContent::Thinking { .. }
    ));
    assert!(matches!(
        &api_messages[0].content[1],
        ApiContent::Text { .. }
    ));
    assert!(matches!(
        &api_messages[0].content[2],
        ApiContent::ToolUse { .. }
    ));

    // User message: Image + ToolResult(LAST body) + Text — order preserved,
    // ToolResult held in place at index 1.
    assert_eq!(api_messages[1].content.len(), 3);
    assert!(matches!(
        &api_messages[1].content[0],
        ApiContent::Image { .. }
    ));
    match &api_messages[1].content[1] {
        ApiContent::ToolResult { content, .. } => {
            assert_eq!(content, "second body", "last-write-wins survived dedupe");
        }
        other => panic!("expected ToolResult at index 1, got {other:?}"),
    }
    assert!(
        matches!(&api_messages[1].content[2], ApiContent::Text { text, .. } if text == "follow-up note"),
    );
}

#[test]
fn convert_messages_to_api_invokes_dedupe() {
    let messages = vec![
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_dup".to_string(),
                name: "fs.read".to_string(),
                input: serde_json::json!({"path": "a.rs"}),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_dup".to_string(),
                content: ToolResultContent::Text("old".to_string()),
                is_error: true,
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_dup".to_string(),
                content: ToolResultContent::Text("new".to_string()),
                is_error: false,
            }],
        ),
    ];

    let api_messages = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);

    let survivors = count_tool_results_with_id(&api_messages, "toolu_dup");
    assert_eq!(
        survivors.len(),
        1,
        "convert_messages_to_api must invoke dedupe_tool_results internally"
    );
    assert_eq!(survivors[0].0, "new");
    assert_eq!(survivors[0].1, Some(false));
}

#[test]
fn dedupe_regression_synthetic_max_tokens_placeholder() {
    // Mirrors the exact production failure: an assistant ToolUse produces a
    // `tool_use_id`, then `handle_max_tokens` (or similar truncation path)
    // synthesizes a fake `tool_result` placeholder for that id, and later
    // the real tool result for the same id lands in the conversation. The
    // pre-fix harness shipped both `tool_result` blocks to Anthropic and
    // got
    //   "messages.4.content.1: each tool_use must have a single result.
    //    Found multiple tool_result blocks with id: toolu_X"
    // back as a 400. The safety net must keep only the real (later) one.
    let messages = vec![
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_X".to_string(),
                name: "fs.write".to_string(),
                input: serde_json::json!({"path": "out.txt"}),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_X".to_string(),
                content: ToolResultContent::Text("synthetic max_tokens placeholder".to_string()),
                is_error: true,
            }],
        ),
        Message::assistant("Let me retry the write..."),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_X".to_string(),
                content: ToolResultContent::Text("Wrote 10011 bytes to out.txt".to_string()),
                is_error: false,
            }],
        ),
    ];

    let api_messages = convert_messages_to_api(&messages, true, ReasoningOrigin::Anthropic);

    let survivors = count_tool_results_with_id(&api_messages, "toolu_X");
    assert_eq!(
        survivors.len(),
        1,
        "exactly one ToolResult for toolu_X should survive the safety-net sweep"
    );
    assert_eq!(
        survivors[0].0, "Wrote 10011 bytes to out.txt",
        "the REAL tool result must win over the synthetic max_tokens placeholder"
    );
    assert_eq!(
        survivors[0].1,
        Some(false),
        "real result is non-error; placeholder's is_error=true must not leak through"
    );
}
