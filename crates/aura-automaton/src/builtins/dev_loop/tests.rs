use aura_agent::agent_runner::TaskExecutionResult;
use aura_reasoner::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    commit_and_push, forward_agent_event, validate_execution, TaskAggregate,
    COMMIT_SKIPPED_NO_CHANGES,
};
use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::state::AutomatonState;
use crate::types::AutomatonId;

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

// ---------------------------------------------------------------------------
// Commit-skip DoD precheck (Section 2 of fix_4.6-class_failures plan).
//
// These tests exercise `TaskAggregate::from_exec` and `commit_and_push`'s
// early-skip branch to ensure a task that produced no file changes and
// no verification evidence never dispatches `git_commit` /
// `git_commit_push`. See `TaskAggregate`'s docs for the chosen signal.
// ---------------------------------------------------------------------------

fn make_ctx() -> (TickContext, mpsc::Receiver<AutomatonEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let ctx = TickContext::new(
        AutomatonId::from_string("test-automaton"),
        AutomatonState::new(),
        tx,
        json!({}),
        None,
        CancellationToken::new(),
    );
    (ctx, rx)
}

fn drain(rx: &mut mpsc::Receiver<AutomatonEvent>) -> Vec<AutomatonEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[test]
fn task_aggregate_from_empty_exec_is_zero() {
    let agg = TaskAggregate::from_exec(&TaskExecutionResult::default());
    assert_eq!(agg.files_changed, 0);
    assert_eq!(agg.verification_steps, 0);
    assert!(agg.should_skip_commit());
}

#[test]
fn task_aggregate_counts_successful_write_file_tool_results() {
    // When the runner's `file_ops` is empty but the message log shows
    // a successful write_file tool_result, we should still treat the
    // task as having file changes. Guards against runners that only
    // populate `file_ops` in some code paths.
    let messages = vec![
        assistant_tool_use(
            "call-1",
            "write_file",
            json!({ "path": "src/foo.rs", "content": "pub fn foo() {}" }),
        ),
        user_tool_result("call-1", r#"{"bytes_written":16}"#, false),
    ];
    let exec = TaskExecutionResult {
        messages,
        ..TaskExecutionResult::default()
    };
    let agg = TaskAggregate::from_exec(&exec);
    assert_eq!(agg.files_changed, 1);
    assert_eq!(agg.verification_steps, 0);
    assert!(!agg.should_skip_commit());
}

#[test]
fn task_aggregate_dedupes_repeat_writes_to_same_path() {
    // Two successful write_file tool_results targeting the same path
    // should count as a single file change; otherwise the count inflates
    // and the DoD precheck could silently pass even when only one file
    // was ever touched.
    let messages = vec![
        assistant_tool_use(
            "call-1",
            "write_file",
            json!({ "path": "src/foo.rs", "content": "v1" }),
        ),
        user_tool_result("call-1", "ok", false),
        assistant_tool_use(
            "call-2",
            "write_file",
            json!({ "path": "src/foo.rs", "content": "v2" }),
        ),
        user_tool_result("call-2", "ok", false),
    ];
    let exec = TaskExecutionResult {
        messages,
        ..TaskExecutionResult::default()
    };
    let agg = TaskAggregate::from_exec(&exec);
    assert_eq!(agg.files_changed, 1);
}

#[test]
fn task_aggregate_ignores_errored_write_tool_results() {
    // tool_result with is_error=true must NOT count as a file change.
    let messages = vec![
        assistant_tool_use(
            "call-1",
            "write_file",
            json!({ "path": "src/foo.rs", "content": "x" }),
        ),
        user_tool_result("call-1", "permission denied", true),
    ];
    let exec = TaskExecutionResult {
        messages,
        ..TaskExecutionResult::default()
    };
    let agg = TaskAggregate::from_exec(&exec);
    assert_eq!(agg.files_changed, 0);
    assert!(agg.should_skip_commit());
}

#[test]
fn task_aggregate_counts_run_command_as_verification_evidence() {
    let messages = vec![
        assistant_tool_use("call-1", "run_command", json!({ "command": "cargo test" })),
        user_tool_result("call-1", "test result: ok. 42 passed", false),
    ];
    let exec = TaskExecutionResult {
        messages,
        ..TaskExecutionResult::default()
    };
    let agg = TaskAggregate::from_exec(&exec);
    assert_eq!(agg.files_changed, 0);
    assert_eq!(agg.verification_steps, 1);
    assert!(!agg.should_skip_commit());
}

#[tokio::test]
async fn commit_and_push_emits_commit_skipped_when_aggregate_is_empty() {
    // When the aggregate shows zero files_changed and zero
    // verification_steps, `commit_and_push` must emit `CommitSkipped`
    // WITHOUT consulting `tool_executor` (so `None` is fine) and
    // WITHOUT touching any workspace (so `workspace_root = None` is
    // fine). This guarantees the skip path is deterministic and
    // independent of whether the workspace happens to be a git repo.
    let (mut ctx, mut rx) = make_ctx();
    let aggregate = TaskAggregate::default();
    assert!(aggregate.should_skip_commit());

    commit_and_push(&mut ctx, None, "task-42", &aggregate).await;

    let events = drain(&mut rx);
    assert_eq!(
        events.len(),
        1,
        "expected exactly one event, got {events:?}"
    );
    match &events[0] {
        AutomatonEvent::CommitSkipped { task_id, reason } => {
            assert_eq!(task_id, "task-42");
            assert_eq!(reason, COMMIT_SKIPPED_NO_CHANGES);
        }
        other => panic!("expected CommitSkipped, got {other:?}"),
    }
}

#[tokio::test]
async fn commit_and_push_does_not_skip_when_aggregate_has_file_changes() {
    // When the aggregate carries at least one file change, the skip
    // precheck must NOT fire. We deliberately pass `workspace_root =
    // None` so the existing post-precheck path bails early with no
    // further events; the assertion is simply that no CommitSkipped
    // event was emitted, i.e. the precheck did not short-circuit.
    let (mut ctx, mut rx) = make_ctx();
    let aggregate = TaskAggregate {
        files_changed: 1,
        verification_steps: 0,
    };
    assert!(!aggregate.should_skip_commit());

    commit_and_push(&mut ctx, None, "task-42", &aggregate).await;

    let events = drain(&mut rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::CommitSkipped { .. })),
        "did not expect CommitSkipped, got {events:?}"
    );
}

#[tokio::test]
async fn commit_and_push_does_not_skip_when_aggregate_has_verification_only() {
    // A task with zero file changes but at least one verification step
    // (e.g. a shell-task that only ran `cargo test`) should still fall
    // through to the existing commit path; the skip is only for the
    // "nothing happened" case.
    let (mut ctx, mut rx) = make_ctx();
    let aggregate = TaskAggregate {
        files_changed: 0,
        verification_steps: 1,
    };
    assert!(!aggregate.should_skip_commit());

    commit_and_push(&mut ctx, None, "task-42", &aggregate).await;

    let events = drain(&mut rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::CommitSkipped { .. })),
        "did not expect CommitSkipped, got {events:?}"
    );
}
