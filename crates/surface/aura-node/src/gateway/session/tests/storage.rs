//! Storage-time truncation of oversized tool-result blobs.

use aura_context_compaction::{compact_for_storage, SESSION_TOOL_BLOB_MAX_BYTES};
use aura_model::Message;

#[test]
fn truncate_messages_for_storage_caps_oversized_tool_result_text() {
    use aura_model::{ContentBlock, Role, ToolResultContent};
    let big = "Z".repeat(SESSION_TOOL_BLOB_MAX_BYTES + 1_000);
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text(big.clone()),
            is_error: false,
        }],
    }];
    compact_for_storage(&mut messages);
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => {
                assert!(t.len() < SESSION_TOOL_BLOB_MAX_BYTES + 200);
                assert!(t.contains("[truncated"));
            }
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn truncate_messages_for_storage_is_noop_for_small_blobs() {
    use aura_model::{ContentBlock, Role, ToolResultContent};
    let small = "ok".to_string();
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text(small.clone()),
            is_error: false,
        }],
    }];
    compact_for_storage(&mut messages);
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => assert_eq!(t, &small),
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn truncate_messages_for_storage_caps_oversized_tool_result_json() {
    use aura_model::{ContentBlock, Role, ToolResultContent};
    let items: Vec<serde_json::Value> = (0..500)
        .map(|i| serde_json::json!({ "id": format!("agent-{i}"), "pad": "X".repeat(200) }))
        .collect();
    let big = serde_json::Value::Array(items);
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_list_agents".into(),
            content: ToolResultContent::Json(big.clone()),
            is_error: false,
        }],
    }];
    compact_for_storage(&mut messages);
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => {
                assert!(t.len() < SESSION_TOOL_BLOB_MAX_BYTES + 200);
                assert!(t.contains("[truncated"));
            }
            other => {
                panic!("oversized Json should be collapsed to truncated Text, got {other:?}")
            }
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
