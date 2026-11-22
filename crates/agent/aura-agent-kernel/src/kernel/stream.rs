//! Streaming handle for reasoning calls.
//!
//! [`ReasonStreamHandle`] is returned alongside a streaming response so the
//! caller can finalize the record entry once the stream completes. Both
//! finalization methods consume `self`, making double-recording impossible
//! by construction (Invariant §3).

use super::RecordObserver;
use crate::context::hash_tx_with_window;
use aura_core_types::{AgentId, PlanUsage, RecordEntry, Transaction, TransactionType};
use aura_model::{StopReason, ThinkingEffort, Usage};
use aura_store_db::Store;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Handle returned alongside a streaming response so the caller can finalize
/// the record entry once the stream completes.
///
/// The handle reserves a sequence number and snapshots the record window at
/// construction time so that `record_completed`/`record_failed` can produce
/// a context hash deterministically from the state observed at the start of
/// the streaming call (Invariant §6).
///
/// Both finalization methods consume `self` so that Invariant §3 holds by
/// construction: a given handle can be finalized at most once, and the
/// `Drop` path on the caller side (e.g. `RecordingStream`) is responsible
/// for ensuring that a record is always appended exactly once.
pub struct ReasonStreamHandle {
    pub(super) kernel_store: Arc<dyn Store>,
    pub(super) agent_id: AgentId,
    pub(super) seq_counter: Arc<Mutex<u64>>,
    pub(super) window: Vec<RecordEntry>,
    pub(super) observer: Option<Arc<dyn RecordObserver>>,
    /// When the provider call started; the finalized entry records the
    /// elapsed time as `duration_ms`.
    pub(super) started: Instant,
    /// Plan usage the stream's response headers reported, recorded with
    /// the completed entry.
    pub(super) plan_usage: Vec<PlanUsage>,
    /// The thinking level the request asked for, recorded with the
    /// completed entry so the reply's footer can show it, live and when
    /// the chat is loaded again.
    pub(super) thinking_effort: Option<ThinkingEffort>,
    /// The model id the request asked for, route prefix included
    /// (`claude/…`, `openrouter/…`). Recorded as `route` on both
    /// finalizations, because the model the upstream echoes back
    /// (`model`) no longer says which sign-in served the call.
    pub(super) route: String,
}

impl ReasonStreamHandle {
    /// Record `plan_usage` with the completed entry.
    #[must_use]
    pub fn with_plan_usage(mut self, plan_usage: Vec<PlanUsage>) -> Self {
        self.plan_usage = plan_usage;
        self
    }

    fn next_seq(&self) -> Result<u64, crate::KernelError> {
        let head_seq = self
            .kernel_store
            .get_head_seq(self.agent_id)
            .map_err(|e| crate::KernelError::Store(format!("get_head_seq: {e}")))?;
        let next = head_seq + 1;
        let mut seq = self
            .seq_counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *seq = next + 1;
        Ok(next)
    }

    fn append(self, tx: Transaction) -> Result<RecordEntry, crate::KernelError> {
        // Allocate the sequence atomically with the append so streaming
        // reasoning interleaves cleanly with other kernel paths that
        // also draw from `Kernel::seq`. The context hash comes from the
        // *snapshot* window captured when `reason_streaming` started,
        // which is the state the model actually reasoned over
        // (Invariant §6).
        let seq = self.next_seq()?;
        let context_hash = hash_tx_with_window(&tx, &self.window)?;
        let entry = RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .build();

        self.kernel_store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;
        if let Some(observer) = self.observer.as_ref() {
            observer.on_entry(&entry);
        }

        Ok(entry)
    }

    /// Record a successfully completed streaming response.
    ///
    /// `thinking` is the model's extended thinking text (empty when it did
    /// not think) and `thinking_ms` how long it streamed; both are kept so
    /// the conversation can be shown again with its thinking.
    ///
    /// Consumes the handle so that double-finalization is impossible by
    /// construction.
    ///
    /// # Errors
    /// Returns error if serialization or store append fails.
    pub fn record_completed(
        self,
        model: &str,
        stop_reason: StopReason,
        usage: &Usage,
        tool_uses: &[String],
        thinking: &str,
        thinking_ms: Option<u64>,
    ) -> Result<RecordEntry, crate::KernelError> {
        let mut reasoning_payload = serde_json::json!({
            "model": model,
            "route": self.route,
            "stop_reason": format!("{stop_reason:?}"),
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "tool_uses": tool_uses,
            "duration_ms": elapsed_ms(self.started),
        });
        add_cache_usage(&mut reasoning_payload, usage);
        add_plan_usage(&mut reasoning_payload, &self.plan_usage);
        add_thinking_effort(&mut reasoning_payload, self.thinking_effort);
        add_thinking(&mut reasoning_payload, thinking, thinking_ms);
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        self.append(tx)
    }

    /// Record a failed streaming response.
    ///
    /// Consumes the handle so that double-finalization is impossible by
    /// construction.
    ///
    /// # Errors
    /// Returns error if serialization or store append fails.
    pub fn record_failed(self, error: &str) -> Result<RecordEntry, crate::KernelError> {
        let reasoning_payload = serde_json::json!({
            "route": self.route,
            "error": error,
            "status": "failed",
            "duration_ms": elapsed_ms(self.started),
        });
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        self.append(tx)
    }
}

pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Add the prompt-cache token counts to a `Reasoning` payload when the
/// provider reported them.
pub(crate) fn add_cache_usage(payload: &mut serde_json::Value, usage: &Usage) {
    if let Some(tokens) = usage.cache_creation_input_tokens {
        payload["cache_creation_input_tokens"] = serde_json::Value::from(tokens);
    }
    if let Some(tokens) = usage.cache_read_input_tokens {
        payload["cache_read_input_tokens"] = serde_json::Value::from(tokens);
    }
}

/// Add the plan usage the provider reported to a `Reasoning` payload,
/// under `plan_usage`, when it reported any.
pub(crate) fn add_plan_usage(payload: &mut serde_json::Value, plan_usage: &[PlanUsage]) {
    if plan_usage.is_empty() {
        return;
    }
    if let Ok(value) = serde_json::to_value(plan_usage) {
        payload["plan_usage"] = value;
    }
}

/// Add the thinking level the request asked for to a `Reasoning` payload,
/// under `thinking_effort`, when it asked for one. Requests that left the
/// level to the provider record nothing, so no level is shown for them.
pub(crate) fn add_thinking_effort(payload: &mut serde_json::Value, effort: Option<ThinkingEffort>) {
    if let Some(effort) = effort {
        payload["thinking_effort"] = serde_json::Value::from(effort.as_str());
    }
}

/// Add `thinking` (and `thinking_ms`) to a `Reasoning` payload when the
/// model thought.
pub(crate) fn add_thinking(payload: &mut serde_json::Value, thinking: &str, ms: Option<u64>) {
    if thinking.trim().is_empty() {
        return;
    }
    payload["thinking"] = serde_json::Value::from(thinking);
    if let Some(ms) = ms {
        payload["thinking_ms"] = serde_json::Value::from(ms);
    }
}
