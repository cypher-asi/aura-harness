use aura_agent::agent_runner::TaskExecutionResult;
use aura_reasoner::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::json;

use super::{forward_agent_event, validate_execution};
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;

#[test]
fn forwards_valid_tool_input_snapshot() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolInputSnapshot {
            id: "tool-1".to_string(),
            name: "run_command".to_string(),
            input: r#"{"command":"npm run build"}"#.to_string(),
        },
    );

    let event = rx.try_recv().expect("expected forwarded event");
    match event {
        AutomatonEvent::ToolCallSnapshot { id, name, input } => {
            assert_eq!(id, "tool-1");
            assert_eq!(name, "run_command");
            assert_eq!(input["command"], "npm run build");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn drops_invalid_tool_input_snapshot_json() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolInputSnapshot {
            id: "tool-1".to_string(),
            name: "run_command".to_string(),
            input: "{".to_string(),
        },
    );

    assert!(
        rx.try_recv().is_err(),
        "invalid JSON snapshot should be ignored"
    );
}

/// Build an assistant message containing a single `tool_use` block.
fn assistant_tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }],
    }
}

/// Build a user message containing a single `tool_result` block.
fn user_tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: ToolResultContent::Text(content.to_string()),
            is_error,
        }],
    }
}

#[test]
fn validate_execution_emits_needs_decomposition_when_reached_implementing_with_no_ops() {
    // Canned failing-run fixture: two write_file tool_uses with no
    // successful tool_result followed by a truncated third pending call.
    let messages = vec![
        assistant_tool_use(
            "call_1",
            "write_file",
            json!({ "path": "src/neural/key.rs", "content": "pub struct NeuralKey {}" }),
        ),
        // same path, second attempt (should dedupe).
        assistant_tool_use(
            "call_2",
            "write_file",
            json!({ "path": "src/neural/key.rs", "content": "pub struct NeuralKey {}" }),
        ),
        assistant_tool_use(
            "call_3",
            "write_file",
            json!({ "path": "src/neural/bundle.rs", "content": "pub fn bundle() {}" }),
        ),
    ];

    let exec = TaskExecutionResult {
        reached_implementing: true,
        no_changes_needed: false,
        messages,
        ..TaskExecutionResult::default()
    };

    let err = validate_execution(exec).expect_err("expected NeedsDecomposition");
    let AutomatonError::NeedsDecomposition { hint } = err else {
        panic!("expected NeedsDecomposition variant, got: {err:?}");
    };

    assert_eq!(
        hint.failed_paths,
        vec![
            "src/neural/key.rs".to_string(),
            "src/neural/bundle.rs".to_string(),
        ],
        "should collect unique paths from pending write_file tool_uses"
    );
    assert_eq!(hint.last_pending_tool_name.as_deref(), Some("write_file"));
    let summary = hint
        .last_pending_tool_input_summary
        .expect("expected summarized write_file input");
    assert!(
        summary.contains("src/neural/bundle.rs"),
        "summary should name the last pending path, got: {summary}"
    );
}

#[test]
fn validate_execution_drops_successful_paths_from_failed_paths() {
    // A write_file whose tool_use did receive a successful tool_result
    // must not appear in failed_paths.
    let messages = vec![
        assistant_tool_use(
            "call_ok",
            "write_file",
            json!({ "path": "src/done.rs", "content": "ok" }),
        ),
        user_tool_result("call_ok", "wrote 2 bytes", false),
        assistant_tool_use(
            "call_pending",
            "edit_file",
            json!({ "path": "src/pending.rs", "old_text": "a", "new_text": "b" }),
        ),
    ];

    let exec = TaskExecutionResult {
        reached_implementing: true,
        messages,
        ..TaskExecutionResult::default()
    };

    let err = validate_execution(exec).expect_err("expected NeedsDecomposition");
    let AutomatonError::NeedsDecomposition { hint } = err else {
        panic!("expected NeedsDecomposition variant, got: {err:?}");
    };

    assert_eq!(hint.failed_paths, vec!["src/pending.rs".to_string()]);
    assert_eq!(hint.last_pending_tool_name.as_deref(), Some("edit_file"));
}

#[test]
fn validate_execution_keeps_hard_error_when_not_reached_implementing() {
    // Classic "completed without any file operations" failure mode when
    // the agent never got past the exploration phase: the old
    // AgentExecution error must still fire so downstream callers keep
    // their current behavior.
    let exec = TaskExecutionResult {
        reached_implementing: false,
        no_changes_needed: false,
        messages: vec![],
        ..TaskExecutionResult::default()
    };

    let err = validate_execution(exec).expect_err("expected AgentExecution");
    match err {
        AutomatonError::AgentExecution(msg) => {
            assert!(
                msg.contains("completion not verified"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected AgentExecution variant, got: {other:?}"),
    }
}

#[test]
fn validate_execution_passes_through_when_no_changes_needed() {
    let exec = TaskExecutionResult {
        reached_implementing: true,
        no_changes_needed: true,
        ..TaskExecutionResult::default()
    };

    let ok = validate_execution(exec).expect("no_changes_needed must short-circuit");
    assert!(ok.no_changes_needed);
}
