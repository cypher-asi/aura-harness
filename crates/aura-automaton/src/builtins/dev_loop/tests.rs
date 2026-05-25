//! Forward-event translation tests for the dev-loop.
//!
//! The simplification removed `TaskAggregate`, `validate_execution`,
//! and `commit_and_push` along with the tests that covered them. The
//! `forward_agent_event` translation layer is still load-bearing for
//! the WS event stream consumed by chat, dev-loop, and task_run, so
//! those tests stay.

use super::forward_agent_event;
use crate::events::AutomatonEvent;

#[test]
fn forwards_text_delta_with_task_id() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("hello".to_string()),
        Some("task-1"),
    );

    let event = rx.try_recv().expect("expected forwarded text delta");
    match event {
        AutomatonEvent::TextDelta { task_id, text } => {
            assert_eq!(task_id.as_deref(), Some("task-1"));
            assert_eq!(text, "hello");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn forwards_chat_text_delta_without_task_id() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::TextDelta("hello".to_string()),
        None,
    );

    let event = rx.try_recv().expect("expected forwarded text delta");
    match event {
        AutomatonEvent::TextDelta { task_id, text } => {
            assert!(task_id.is_none());
            assert_eq!(text, "hello");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn forwards_tool_start_with_task_id() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "run_command".to_string(),
        },
        Some("task-1"),
    );

    let event = rx.try_recv().expect("expected forwarded tool start");
    match event {
        AutomatonEvent::ToolCallStarted { task_id, id, name } => {
            assert_eq!(task_id.as_deref(), Some("task-1"));
            assert_eq!(id, "tool-1");
            assert_eq!(name, "run_command");
            let wire = serde_json::to_value(AutomatonEvent::ToolCallStarted { task_id, id, name })
                .expect("serialize tool start");
            assert_eq!(wire["type"], "tool_use_start");
            assert_eq!(wire["task_id"], "task-1");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

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
        Some("task-1"),
    );

    let event = rx.try_recv().expect("expected forwarded event");
    match event {
        AutomatonEvent::ToolCallSnapshot {
            task_id,
            id,
            name,
            input,
            snapshot_partial,
        } => {
            assert_eq!(task_id.as_deref(), Some("task-1"));
            assert_eq!(id, "tool-1");
            assert_eq!(name, "run_command");
            assert_eq!(input["command"], "npm run build");
            assert!(
                !snapshot_partial,
                "parseable JSON must surface as a complete snapshot"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn forwards_partial_tool_input_snapshot_with_flag() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolInputSnapshot {
            id: "tool-1".to_string(),
            name: "write_file".to_string(),
            input: "{\"path\":\"src/".to_string(),
        },
        Some("task-1"),
    );

    let event = rx
        .try_recv()
        .expect("partial snapshot must still be forwarded");
    match event {
        AutomatonEvent::ToolCallSnapshot {
            task_id,
            id,
            name,
            input,
            snapshot_partial,
        } => {
            assert_eq!(task_id.as_deref(), Some("task-1"));
            assert_eq!(id, "tool-1");
            assert_eq!(name, "write_file");
            assert!(
                snapshot_partial,
                "unparseable JSON must set snapshot_partial"
            );
            assert_eq!(
                input,
                serde_json::Value::String("{\"path\":\"src/".to_string())
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn forwards_tool_call_retrying_event() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolCallRetrying {
            tool_use_id: "toolu_1".to_string(),
            tool_name: "write_file".to_string(),
            attempt: 2,
            max_attempts: 8,
            delay_ms: 500,
            reason: "overloaded_error".to_string(),
        },
        Some("task-1"),
    );

    let event = rx.try_recv().expect("ToolCallRetrying must forward");
    match event {
        AutomatonEvent::ToolCallRetrying {
            task_id,
            tool_use_id,
            tool_name,
            attempt,
            max_attempts,
            delay_ms,
            reason,
        } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(tool_use_id, "toolu_1");
            assert_eq!(tool_name, "write_file");
            assert_eq!(attempt, 2);
            assert_eq!(max_attempts, 8);
            assert_eq!(delay_ms, 500);
            assert_eq!(reason, "overloaded_error");
        }
        other => panic!("expected ToolCallRetrying, got: {other:?}"),
    }
}

#[test]
fn forwards_tool_call_failed_event() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolCallFailed {
            tool_use_id: "toolu_1".to_string(),
            tool_name: "write_file".to_string(),
            reason: "retries exhausted".to_string(),
        },
        Some("task-1"),
    );

    let event = rx.try_recv().expect("ToolCallFailed must forward");
    match event {
        AutomatonEvent::ToolCallFailed {
            task_id,
            tool_use_id,
            tool_name,
            reason,
        } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(tool_use_id, "toolu_1");
            assert_eq!(tool_name, "write_file");
            assert_eq!(reason, "retries exhausted");
        }
        other => panic!("expected ToolCallFailed, got: {other:?}"),
    }
}
