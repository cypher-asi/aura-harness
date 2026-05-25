//! Forward-event translation tests for the dev-loop.
//!
//! The simplification removed `TaskAggregate`, `validate_execution`,
//! and `commit_and_push` along with the tests that covered them. The
//! `forward_agent_event` translation layer is still load-bearing for
//! the WS event stream consumed by chat, dev-loop, and task_run, so
//! those tests stay.
//!
//! `AgentIdentityEnvelope` wire-roundtrip tests live here too: they
//! lock the JSON shape aura-os populates → harness parses → rendered
//! system prompt tags so a future schema drift on either side
//! triggers a compile / test failure rather than a silent
//! cross-repo break.

use super::{forward_agent_event, AgentIdentityEnvelope};
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
fn envelope_from_json_parses_full_payload() {
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "Atlas",
            "role": "Engineer",
            "personality": "Precise and methodical.",
        },
        "agent_skills": ["Rust", "TypeScript"],
        "agent_system_prompt": "Use TDD on every change.",
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);

    assert!(
        !envelope.is_empty(),
        "populated payload must not collapse to empty"
    );
    let info = envelope
        .as_agent_info()
        .expect("populated envelope must yield AgentInfo");
    let identity = info.identity.expect("identity present");
    assert_eq!(identity.name, "Atlas");
    assert_eq!(identity.role, "Engineer");
    assert_eq!(identity.personality, "Precise and methodical.");
    assert_eq!(info.skills, &["Rust".to_string(), "TypeScript".to_string()]);
    assert_eq!(info.system_prompt, Some("Use TDD on every change."));
}

#[test]
fn envelope_from_json_handles_missing_fields() {
    let envelope = AgentIdentityEnvelope::from_json(&serde_json::json!({}));
    assert!(
        envelope.is_empty(),
        "empty JSON object must produce an empty envelope"
    );
    assert!(
        envelope.as_agent_info().is_none(),
        "empty envelope must yield no AgentInfo so identity sections drop"
    );
}

#[test]
fn envelope_from_json_treats_blank_strings_as_empty() {
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "   ",
            "role": "",
            "personality": "\n\t",
        },
        "agent_skills": [],
        "agent_system_prompt": "   ",
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    assert!(
        envelope.is_empty(),
        "blank-string fields must collapse to empty so no <agent_*> tags render"
    );
    assert!(envelope.as_agent_info().is_none());
}

#[test]
fn envelope_skills_only_still_renders_as_populated() {
    // Skills-only payloads should still render an <agent_skills> tag
    // even when identity / system prompt are absent.
    let cfg = serde_json::json!({
        "agent_skills": ["Rust"],
    });

    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    assert!(!envelope.is_empty());
    let info = envelope.as_agent_info().expect("skills-only is populated");
    assert!(
        info.identity.is_none(),
        "missing identity object must leave AgentInfo.identity = None"
    );
    assert_eq!(info.skills, &["Rust".to_string()]);
    assert!(info.system_prompt.is_none());
}

#[test]
fn envelope_roundtrips_into_system_prompt_tags() {
    // End-to-end roundtrip: aura-os-shaped JSON → envelope →
    // AgentInfo → agentic_execution_system_prompt → rendered tags.
    let cfg = serde_json::json!({
        "agent_identity": {
            "name": "Atlas",
            "role": "Engineer",
            "personality": "Precise and methodical.",
        },
        "agent_skills": ["Rust", "TypeScript"],
        "agent_system_prompt": "Use TDD on every change.",
    });
    let envelope = AgentIdentityEnvelope::from_json(&cfg);
    let info = envelope.as_agent_info().expect("populated");

    let project = aura_agent::prompts::ProjectInfo {
        project_id: None,
        name: "Demo",
        description: "A demo project.",
        folder_path: "/nonexistent",
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt = aura_agent::prompts::agentic_execution_system_prompt(&project, Some(&info));

    for tag in [
        "<agent_identity>",
        "</agent_identity>",
        "<agent_skills>",
        "- Rust",
        "- TypeScript",
        "</agent_skills>",
        "<agent_system_prompt>",
        "Use TDD on every change.",
        "</agent_system_prompt>",
        "<project_context>",
    ] {
        assert!(
            prompt.contains(tag),
            "expected {tag} in the rendered roundtrip prompt; got:\n{prompt}",
        );
    }
}

#[test]
fn empty_envelope_keeps_identity_sections_off() {
    let envelope = AgentIdentityEnvelope::from_json(&serde_json::json!({}));
    let info = envelope.as_agent_info();
    assert!(info.is_none());

    let project = aura_agent::prompts::ProjectInfo {
        project_id: None,
        name: "Demo",
        description: "A demo project.",
        folder_path: "/nonexistent",
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    };
    let prompt = aura_agent::prompts::agentic_execution_system_prompt(&project, info.as_ref());

    for tag in [
        "<agent_identity>",
        "<agent_skills>",
        "<agent_system_prompt>",
    ] {
        assert!(
            !prompt.contains(tag),
            "empty envelope must NOT render {tag}; got:\n{prompt}",
        );
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
