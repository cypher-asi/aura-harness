use crate::messages::{
    truncate_messages_for_storage, RedactionMarker, SESSION_TOOL_BLOB_MAX_BYTES,
};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::Value;

#[test]
fn truncate_messages_for_storage_caps_oversized_tool_result_text() {
    let big = "Z".repeat(SESSION_TOOL_BLOB_MAX_BYTES + 1_000);
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text(big),
            is_error: false,
        }],
    }];
    truncate_messages_for_storage(&mut messages);
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
    let small = "ok".to_string();
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text(small.clone()),
            is_error: false,
        }],
    }];
    truncate_messages_for_storage(&mut messages);
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => assert_eq!(t, &small),
            other => panic!("expected Text, got {other:?}"),
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn truncate_messages_for_storage_keeps_oversized_tool_use_input_as_object() {
    // Regression: previously this branch wrote `Value::String(truncated)`
    // back into `tool_use.input`, which Anthropic rejects on replay with
    // 400 `messages.N.content.M.tool_use.input: Input should be an
    // object`. With the RedactionMarker fix, the input must remain a
    // JSON object and the oversized string field is summarized.
    let big_markdown = "M".repeat(SESSION_TOOL_BLOB_MAX_BYTES + 4_000);
    let mut messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            "tu_create_spec",
            "create_spec",
            serde_json::json!({
                "title": "Phase 06: MLS GroupService",
                "markdown_contents": big_markdown,
            }),
        )],
    }];
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input, .. } = &messages[0].content[0] else {
        panic!("expected ToolUse, got {:?}", messages[0].content[0]);
    };
    assert!(input.is_object(), "input must remain a JSON object");
    assert!(
        RedactionMarker::is_marker(input),
        "oversized input should carry a redaction marker"
    );
    assert_eq!(
        input.get("title").and_then(Value::as_str),
        Some("Phase 06: MLS GroupService"),
        "small fields should survive"
    );
    assert!(
        input.get("markdown_contents").is_none(),
        "the oversized field should be removed"
    );
    let serialized = serde_json::to_string(input).expect("serialize");
    assert!(
        serialized.len() <= SESSION_TOOL_BLOB_MAX_BYTES,
        "redacted input ({} bytes) must fit under storage cap ({} bytes)",
        serialized.len(),
        SESSION_TOOL_BLOB_MAX_BYTES
    );
}

#[test]
fn truncate_messages_for_storage_idempotent_for_already_redacted_tool_use() {
    // Running compaction twice should be a no-op once the input has
    // already been marked; otherwise we'd keep stacking markers.
    let big = "X".repeat(SESSION_TOOL_BLOB_MAX_BYTES + 2_000);
    let mut messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            "tu_1",
            "create_spec",
            serde_json::json!({
                "title": "ok",
                "markdown_contents": big,
            }),
        )],
    }];
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input: first, .. } = messages[0].content[0].clone() else {
        panic!("expected ToolUse after first pass");
    };
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input: second, .. } = &messages[0].content[0] else {
        panic!("expected ToolUse after second pass");
    };
    assert_eq!(&first, second, "second pass must be a no-op");
}

#[test]
fn truncate_messages_for_storage_preserves_small_tool_use_input() {
    let input_value = serde_json::json!({
        "title": "Tiny",
        "markdown_contents": "# Hello\n\nworld",
    });
    let mut messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            "tu_small",
            "create_spec",
            input_value.clone(),
        )],
    }];
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input, .. } = &messages[0].content[0] else {
        panic!("expected ToolUse");
    };
    assert_eq!(input, &input_value, "below-cap input must be untouched");
}

#[test]
fn truncate_messages_for_storage_redacts_multiple_oversized_string_fields() {
    // `edit_file` carries both `old_text` and `new_text`; if a single
    // field redaction isn't enough we walk to the next-largest field.
    let big = "Q".repeat(SESSION_TOOL_BLOB_MAX_BYTES);
    let mut messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            "tu_edit",
            "edit_file",
            serde_json::json!({
                "path": "src/big.rs",
                "old_text": big.clone(),
                "new_text": big,
            }),
        )],
    }];
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input, .. } = &messages[0].content[0] else {
        panic!("expected ToolUse");
    };
    assert!(input.is_object());
    assert!(RedactionMarker::is_marker(input));
    assert_eq!(
        input.get("path").and_then(Value::as_str),
        Some("src/big.rs")
    );
    let serialized = serde_json::to_string(input).expect("serialize");
    assert!(
        serialized.len() <= SESSION_TOOL_BLOB_MAX_BYTES,
        "redacted input ({} bytes) must fit under storage cap",
        serialized.len()
    );
}

#[test]
fn truncate_messages_for_storage_leaves_non_object_tool_use_input_untouched() {
    // Defensive: non-object inputs are already invalid upstream. We
    // intentionally don't mutate them so the upstream bug is visible
    // (and the aura-os persistence guard surfaces it).
    let original = Value::String("oops not an object".repeat(SESSION_TOOL_BLOB_MAX_BYTES / 4));
    let mut messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::tool_use(
            "tu_corrupt",
            "create_spec",
            original.clone(),
        )],
    }];
    truncate_messages_for_storage(&mut messages);
    let ContentBlock::ToolUse { input, .. } = &messages[0].content[0] else {
        panic!("expected ToolUse");
    };
    assert_eq!(input, &original);
}

#[test]
fn truncate_messages_for_storage_caps_oversized_tool_result_json() {
    let items: Vec<Value> = (0..500)
        .map(|i| serde_json::json!({ "id": format!("agent-{i}"), "pad": "X".repeat(200) }))
        .collect();
    let big = Value::Array(items);
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tu_list_agents".into(),
            content: ToolResultContent::Json(big),
            is_error: false,
        }],
    }];
    truncate_messages_for_storage(&mut messages);
    match &messages[0].content[0] {
        ContentBlock::ToolResult { content, .. } => match content {
            ToolResultContent::Text(t) => {
                assert!(t.len() < SESSION_TOOL_BLOB_MAX_BYTES + 200);
                assert!(t.contains("[truncated"));
            }
            other => {
                panic!("oversized Json should be collapsed to truncated Text, got {other:?}");
            }
        },
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
