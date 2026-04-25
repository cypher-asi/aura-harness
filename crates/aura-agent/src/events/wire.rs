//! On-the-wire shapes for `debug.*` observability frames.
//!
//! Each [`DebugEvent`] variant serializes as a flat JSON object with a
//! `type: "debug.<kind>"` discriminator — the format the
//! `aura-os-server` forwarder routes on. See
//! `apps/aura-os-server/src/loop_log.rs` for the consumer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Observability frames routed into the `aura-os` per-run bundle
/// (`llm_calls.jsonl`, `iterations.jsonl`, `blockers.jsonl`,
/// `retries.jsonl`).
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
        /// HTTP `x-request-id` captured from the provider response. This
        /// is the correlation key against aura-router / provider logs.
        /// Populated from `ProviderTrace.provider_request_id`.
        #[serde(default, alias = "request_id", skip_serializing_if = "Option::is_none")]
        provider_request_id: Option<String>,
        /// Provider-internal message id (Anthropic `message_start.message.id`).
        /// Useful for correlating with a specific assistant turn, but
        /// NOT the same as `provider_request_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
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
