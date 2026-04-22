//! Unified streaming events emitted during agent execution.
//!
//! `TurnEvent` is the event type for `AgentLoop`. Consumers subscribe
//! by passing an `mpsc::Sender<TurnEvent>` to the orchestrator.
//!
//! Debug observability (`debug.*`) events are carried through the same
//! channel via the [`TurnEvent::Debug`] variant, so a single consumer
//! sees both the live UI stream and the structured metrics stream in
//! arrival order. The [`DebugEvent`] type itself is JSON-tagged
//! (`{"type": "debug.llm_call", ...}`) to match the on-disk schema the
//! `aura-os` run-log consumer expects — see
//! `apps/aura-os-server/src/loop_log.rs::update_counters` in the
//! sibling repo.
//!
//! The [`mapper`] submodule provides a shared `TurnEventSink` trait +
//! [`map_agent_loop_event`] dispatcher used by both the TUI's
//! `UiCommandSink` and the headless WebSocket session's
//! `OutboundMessageSink` so adding a new `TurnEvent` variant is a
//! compile error until every consumer handles it.

pub mod mapper;
pub use mapper::{map_agent_loop_event, TurnEventSink};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unified events emitted during agent/turn execution.
///
/// Covers all events emitted during agent execution.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Incremental text content from the model.
    TextDelta(String),

    /// Incremental thinking/reasoning content from the model.
    ThinkingDelta(String),

    /// Thinking block completed (end of extended-thinking content).
    ThinkingComplete,

    /// A tool use block started streaming.
    ToolStart {
        /// Tool use ID from the model.
        id: String,
        /// Tool name.
        name: String,
    },

    /// Incremental snapshot of tool input JSON as it streams in.
    ToolInputSnapshot {
        /// Tool use ID.
        id: String,
        /// Tool name.
        name: String,
        /// Accumulated input JSON so far (may be partial/incomplete).
        input: String,
    },

    /// A tool execution completed (with full result).
    ToolComplete {
        /// Tool name.
        name: String,
        /// Tool arguments (JSON), if available.
        args: Option<serde_json::Value>,
        /// Result content (text).
        result: String,
        /// Whether the tool execution failed.
        is_error: bool,
    },

    /// Tool result that will be appended to context.
    ToolResult {
        /// Tool use ID.
        tool_use_id: String,
        /// Tool name.
        tool_name: String,
        /// Result content.
        content: String,
        /// Whether the result is an error.
        is_error: bool,
    },

    /// One iteration (model call + tool execution) completed.
    IterationComplete {
        /// Zero-based iteration index.
        iteration: usize,
        /// Input tokens used in this iteration.
        input_tokens: u64,
        /// Output tokens used in this iteration.
        output_tokens: u64,
    },

    /// Streaming is complete for the current step.
    StepComplete,

    /// Streaming was interrupted and restarted. Consumers must discard
    /// any buffered partial content for the current iteration and treat
    /// subsequent events as the authoritative source.
    StreamReset {
        /// Human-readable reason for the reset.
        reason: String,
    },

    /// A warning was injected into the context.
    Warning(String),

    /// An error occurred during execution.
    Error {
        /// Machine-readable error code.
        code: String,
        /// Human-readable description.
        message: String,
        /// Whether the loop can continue after this error.
        recoverable: bool,
    },

    /// Structured observability frame for the `aura-os` run bundle.
    /// Flows through the same channel as the UI-facing variants so
    /// that downstream forwarders preserve ordering.
    Debug(DebugEvent),
}

/// Backward-compatible alias. Prefer [`TurnEvent`] for new code.
pub type AgentLoopEvent = TurnEvent;

/// Observability frames routed into the `aura-os` per-run bundle
/// (`llm_calls.jsonl`, `iterations.jsonl`, `blockers.jsonl`,
/// `retries.jsonl`). Each variant serializes as a flat JSON object with
/// a `type: "debug.<kind>"` discriminator — the format the
/// `aura-os-server` forwarder routes on. See
/// `apps/aura-os-server/src/loop_log.rs` for the consumer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DebugEvent {
    /// A single completed LLM call (success path). Token counts come
    /// from the provider response; `duration_ms` is measured from the
    /// moment the request is issued to the moment the stream
    /// terminates (or non-streaming response is parsed).
    #[serde(rename = "debug.llm_call")]
    LlmCall {
        timestamp: DateTime<Utc>,
        provider: String,
        model: String,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_instance_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    /// A completed agent iteration (one model call + the tool calls it
    /// drove). `index` is 0-based within the current turn.
    #[serde(rename = "debug.iteration")]
    Iteration {
        timestamp: DateTime<Utc>,
        index: u32,
        tool_calls: u32,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },

    /// A tool call that the harness refused to execute and instead
    /// replaced with a `[BLOCKED] ...` tool result.
    #[serde(rename = "debug.blocker")]
    Blocker {
        timestamp: DateTime<Utc>,
        /// Short discriminator: `"duplicate_write"`, `"read_required"`,
        /// `"policy"`, `"tool_error"`, etc.
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The human-readable blocker message the model would see.
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },

    /// A retryable provider error that is about to trigger a backoff +
    /// retry. Emitted *before* the provider sleeps, with
    /// `attempt` = the 1-based attempt number that will now occur.
    #[serde(rename = "debug.retry")]
    Retry {
        timestamp: DateTime<Utc>,
        /// Short error class: `"rate_limited_429"`, `"transient_5xx"`,
        /// `"timeout"`, `"stream_interrupted"`, etc.
        reason: String,
        /// 1-based attempt number that WILL now occur (first retry = 2).
        attempt: u32,
        /// Delay the provider is about to sleep, in milliseconds.
        wait_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
}

impl DebugEvent {
    /// Return the `type` string this variant serializes with. Useful
    /// for log-level filtering without going through `serde_json`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::LlmCall { .. } => "debug.llm_call",
            Self::Iteration { .. } => "debug.iteration",
            Self::Blocker { .. } => "debug.blocker",
            Self::Retry { .. } => "debug.retry",
        }
    }
}

#[cfg(test)]
mod debug_event_tests {
    use super::*;
    use chrono::TimeZone;

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
            request_id: Some("req_abc".into()),
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
        assert_eq!(v["request_id"], "req_abc");
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
            request_id: None,
        };
        let v = serde_json::to_value(&ev).expect("serialize");
        assert!(v.get("task_id").is_none());
        assert!(v.get("agent_instance_id").is_none());
        assert!(v.get("request_id").is_none());
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
                    request_id: None,
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
}
