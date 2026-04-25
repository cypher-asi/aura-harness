use chrono::{DateTime, TimeZone, Utc};

use super::DebugEvent;

fn fixed_ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 21, 12, 34, 56).unwrap()
}

#[test]
fn llm_call_serializes_with_debug_llm_call_type_and_counter_fields() {
    let ev = DebugEvent::LlmCall {
        timestamp: fixed_ts(),
        provider: "anthropic".into(),
        model: "claude-opus-4-6".into(),
        input_tokens: 1024,
        output_tokens: 512,
        duration_ms: 1_337,
        task_id: Some("task-42".into()),
        agent_instance_id: Some("inst-1".into()),
        provider_request_id: Some("req_abc".into()),
        message_id: Some("msg_42".into()),
    };
    let v = serde_json::to_value(&ev).expect("serialize");

    assert_eq!(v["type"], "debug.llm_call");
    assert_eq!(v["provider"], "anthropic");
    assert_eq!(v["model"], "claude-opus-4-6");
    // aura-os `update_counters` reads these exact field names off
    // `token_usage`/`assistant_message_end`, not `debug.llm_call`,
    // but we carry them here so the channel file is analytic-ready.
    assert_eq!(v["input_tokens"], 1024);
    assert_eq!(v["output_tokens"], 512);
    assert_eq!(v["duration_ms"], 1_337);
    assert_eq!(v["task_id"], "task-42");
    assert_eq!(v["agent_instance_id"], "inst-1");
    assert_eq!(v["provider_request_id"], "req_abc");
    assert_eq!(v["message_id"], "msg_42");
    assert!(
        v.get("timestamp").and_then(|t| t.as_str()).is_some(),
        "timestamp should serialize as a string (RFC3339)"
    );
}

#[test]
fn llm_call_omits_none_option_fields() {
    let ev = DebugEvent::LlmCall {
        timestamp: fixed_ts(),
        provider: "anthropic".into(),
        model: "claude-opus-4-6".into(),
        input_tokens: 0,
        output_tokens: 0,
        duration_ms: 0,
        task_id: None,
        agent_instance_id: None,
        provider_request_id: None,
        message_id: None,
    };
    let v = serde_json::to_value(&ev).expect("serialize");
    assert!(v.get("task_id").is_none());
    assert!(v.get("agent_instance_id").is_none());
    assert!(v.get("provider_request_id").is_none());
    assert!(v.get("message_id").is_none());
    assert!(
        v.get("request_id").is_none(),
        "request_id must not leak onto the wire after the split"
    );
}

#[test]
fn llm_call_accepts_legacy_request_id_alias_on_deserialize() {
    // Old run bundles (pre-`harden-llm-stream-retry-observability`)
    // serialized a single `request_id` field that was actually the
    // provider-internal message id. Accept it as a fallback for
    // `provider_request_id` so those bundles still parse.
    let raw = serde_json::json!({
        "type": "debug.llm_call",
        "timestamp": fixed_ts(),
        "provider": "anthropic",
        "model": "claude-opus-4-6",
        "input_tokens": 0,
        "output_tokens": 0,
        "duration_ms": 0,
        "request_id": "legacy_req_01"
    });
    let ev: DebugEvent = serde_json::from_value(raw).expect("parse legacy shape");
    match ev {
        DebugEvent::LlmCall {
            provider_request_id,
            message_id,
            ..
        } => {
            assert_eq!(provider_request_id.as_deref(), Some("legacy_req_01"));
            assert!(message_id.is_none());
        }
        other => panic!("expected LlmCall, got {other:?}"),
    }
}

#[test]
fn iteration_serializes_with_debug_iteration_type_and_fields() {
    let ev = DebugEvent::Iteration {
        timestamp: fixed_ts(),
        index: 3,
        tool_calls: 2,
        duration_ms: 4_200,
        task_id: Some("task-7".into()),
    };
    let v = serde_json::to_value(&ev).expect("serialize");
    assert_eq!(v["type"], "debug.iteration");
    assert_eq!(v["index"], 3);
    // `tool_calls` is an aura-os counter field. `update_counters`
    // bumps `tool_calls` only on `tool_call_snapshot` etc., not on
    // `debug.iteration`, but the analyzer reads it from this file.
    assert_eq!(v["tool_calls"], 2);
    assert_eq!(v["duration_ms"], 4_200);
    assert_eq!(v["task_id"], "task-7");
}

#[test]
fn blocker_serializes_with_debug_blocker_type_and_fields() {
    let ev = DebugEvent::Blocker {
        timestamp: fixed_ts(),
        kind: "duplicate_write".into(),
        path: Some("src/lib.rs".into()),
        message: "[BLOCKED] You already wrote to src/lib.rs".into(),
        task_id: None,
    };
    let v = serde_json::to_value(&ev).expect("serialize");
    assert_eq!(v["type"], "debug.blocker");
    assert_eq!(v["kind"], "duplicate_write");
    assert_eq!(v["path"], "src/lib.rs");
    assert_eq!(v["message"], "[BLOCKED] You already wrote to src/lib.rs");
    assert!(v.get("task_id").is_none());
}

#[test]
fn retry_serializes_with_debug_retry_type_and_counter_fields() {
    let ev = DebugEvent::Retry {
        timestamp: fixed_ts(),
        reason: "rate_limited_429".into(),
        attempt: 2,
        wait_ms: 7_500,
        provider: Some("anthropic".into()),
        model: Some("claude-opus-4-6".into()),
        task_id: Some("t-1".into()),
    };
    let v = serde_json::to_value(&ev).expect("serialize");
    assert_eq!(v["type"], "debug.retry");
    assert_eq!(v["reason"], "rate_limited_429");
    // `attempt` / `wait_ms` are the exact field names aura-os
    // writes into retries.jsonl; if these drift, downstream
    // analytics silently lose the fields.
    assert_eq!(v["attempt"], 2);
    assert_eq!(v["wait_ms"], 7_500);
    assert_eq!(v["provider"], "anthropic");
    assert_eq!(v["model"], "claude-opus-4-6");
    assert_eq!(v["task_id"], "t-1");
}

#[test]
fn serialized_type_strings_match_aura_os_counter_constants() {
    // These constants live in
    // `apps/aura-os-server/src/loop_log.rs` as
    //   DEBUG_EVENT_LLM_CALL = "debug.llm_call"
    //   DEBUG_EVENT_ITERATION = "debug.iteration"
    //   DEBUG_EVENT_BLOCKER = "debug.blocker"
    //   DEBUG_EVENT_RETRY = "debug.retry"
    // and drive both `classify_debug_file` and `update_counters`.
    // A string mismatch here silently demotes these frames to
    // "uncategorized events" on disk, so assert it explicitly.
    let ts = fixed_ts();
    let samples = [
        (
            DebugEvent::LlmCall {
                timestamp: ts,
                provider: "p".into(),
                model: "m".into(),
                input_tokens: 0,
                output_tokens: 0,
                duration_ms: 0,
                task_id: None,
                agent_instance_id: None,
                provider_request_id: None,
                message_id: None,
            },
            "debug.llm_call",
        ),
        (
            DebugEvent::Iteration {
                timestamp: ts,
                index: 0,
                tool_calls: 0,
                duration_ms: 0,
                task_id: None,
            },
            "debug.iteration",
        ),
        (
            DebugEvent::Blocker {
                timestamp: ts,
                kind: "k".into(),
                path: None,
                message: "m".into(),
                task_id: None,
            },
            "debug.blocker",
        ),
        (
            DebugEvent::Retry {
                timestamp: ts,
                reason: "r".into(),
                attempt: 2,
                wait_ms: 1,
                provider: None,
                model: None,
                task_id: None,
            },
            "debug.retry",
        ),
    ];
    for (ev, expected) in samples {
        assert_eq!(ev.kind(), expected);
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(
            json.contains(&format!("\"type\":\"{expected}\"")),
            "expected `\"type\":\"{expected}\"` in serialized form, got: {json}"
        );
    }
}

#[test]
fn round_trips_through_serde_json() {
    let ev = DebugEvent::Blocker {
        timestamp: fixed_ts(),
        kind: "duplicate_write".into(),
        path: Some("a/b.rs".into()),
        message: "[BLOCKED] ...".into(),
        task_id: Some("t".into()),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let back: DebugEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(ev, back);
}
