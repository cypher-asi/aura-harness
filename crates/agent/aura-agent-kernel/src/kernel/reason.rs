//! Reasoning entry points.
//!
//! The two public methods here bound provider calls with the
//! kernel-configured `proposal_timeout_ms` so a hung model cannot stall
//! the agent indefinitely (Wave 5 / T2.6 + T7.3):
//!
//! - `reason` drives a synchronous `ModelProvider::complete` call and
//!   appends a `Reasoning` record entry on success.
//! - `reason_streaming` returns a [`super::ReasonStreamHandle`] plus the
//!   event stream; the caller finalizes the record when the stream
//!   drains.

use super::seq::Claim;
use super::{Kernel, ReasonResult, ReasonStreamHandle};
use crate::context::hash_tx_with_hashes;
use aura_core_types::{ContextHash, RecordEntry, Transaction, TransactionType};
use aura_model::{
    ModelCallPurpose, ModelName, ModelRequest, ModelResponse, StreamEventStream, ThinkingEffort,
};
use tracing::error;

/// How a failed provider call is recorded and, if recording fails, logged.
struct CallFailure {
    kind: &'static str,
    stage: &'static str,
    log_message: &'static str,
}

impl Kernel {
    /// Call the model provider and record the result.
    ///
    /// # Errors
    /// Returns error if the model call or storage fails.
    pub async fn reason(
        &self,
        mut request: ModelRequest,
    ) -> Result<ReasonResult, crate::KernelError> {
        self.scope_request(&mut request);
        // Bound the reasoner call with the kernel-config timeout so a hung
        // provider cannot stall the agent indefinitely. (Wave 5 / T2.6 +
        // T7.3.)
        let timeout = std::time::Duration::from_millis(self.config.proposal_timeout_ms);
        let started = std::time::Instant::now();
        let purpose = request.metadata.purpose;
        let thinking_effort = request.thinking_effort;
        let model = request.model.clone();
        let response = match tokio::time::timeout(timeout, self.provider.complete(request)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                // Invariant §3 strict: every LLM call is recorded, even
                // on failure. Record a Reasoning entry with an error
                // indicator before bubbling the error up. The audit log
                // captures the formatted message; the typed
                // `ReasonerError` is then preserved in
                // `KernelError::Reasoner` so callers can branch on the
                // variant (rate limit, transient, bad request, …)
                // instead of string-matching.
                let failure = CallFailure {
                    kind: "reason_sync",
                    stage: "complete",
                    log_message: "failed to record reasoning failure entry",
                };
                self.record_call_failure(&failure, &e.to_string(), (&model, purpose))?;
                return Err(crate::KernelError::Reasoner(e));
            }
            Err(_) => {
                let reason_str = format!(
                    "model provider did not respond within {}ms",
                    self.config.proposal_timeout_ms
                );
                let failure = CallFailure {
                    kind: "reason_sync",
                    stage: "complete_timeout",
                    log_message: "failed to record reasoning timeout entry",
                };
                self.record_call_failure(&failure, &reason_str, (&model, purpose))?;
                return Err(crate::KernelError::Timeout(reason_str));
            }
        };

        let payload_bytes =
            Self::reasoning_payload(&response, &model, purpose, thinking_effort, started)?;
        let entry = self.append_reasoning_entry(payload_bytes)?;
        Ok(ReasonResult { entry, response })
    }

    /// Serialize the `Reasoning` record payload for a successful call.
    fn reasoning_payload(
        response: &ModelResponse,
        model: &ModelName,
        purpose: Option<ModelCallPurpose>,
        thinking_effort: Option<ThinkingEffort>,
        started: std::time::Instant,
    ) -> Result<Vec<u8>, crate::KernelError> {
        let mut reasoning_payload = serde_json::json!({
            "model": response.trace.model,
            "route": model,
            "stop_reason": format!("{:?}", response.stop_reason),
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
            "duration_ms": super::stream::elapsed_ms(started),
        });
        super::stream::add_cache_usage(&mut reasoning_payload, &response.usage);
        super::stream::add_plan_usage(&mut reasoning_payload, &response.trace.plan_usage);
        if let Some(purpose) = purpose {
            reasoning_payload["purpose"] = serde_json::json!(purpose);
        }
        super::stream::add_thinking_effort(&mut reasoning_payload, thinking_effort);
        let thinking: String = response
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                aura_model::ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect();
        super::stream::add_thinking(&mut reasoning_payload, &thinking, None);
        serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))
    }

    /// Claim the next number and append a `Reasoning` entry carrying
    /// `payload_bytes`.
    fn append_reasoning_entry(
        &self,
        payload_bytes: Vec<u8>,
    ) -> Result<RecordEntry, crate::KernelError> {
        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        let claim = self.claim_seq()?;
        let seq = claim.seq;
        let window = self.window_hashes(seq)?;
        let context_hash = hash_tx_with_hashes(&tx, &window)?;

        let entry = RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .build();

        self.store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;
        self.notify_entry(claim, &entry);
        Ok(entry)
    }

    /// Start a streaming reasoning call.
    ///
    /// Returns a handle for finalizing the record entry and the event stream.
    ///
    /// # Errors
    /// Returns error if the model call fails.
    pub async fn reason_streaming(
        &self,
        mut request: ModelRequest,
    ) -> Result<(ReasonStreamHandle, StreamEventStream), crate::KernelError> {
        self.scope_request(&mut request);
        // Snapshot the record window *before* invoking the provider so
        // the context hash computed at finalization time depends solely
        // on the state observed at the start of the reasoning call
        // (Invariant §6). The sequence number itself is only reserved
        // atomically at `append` time so streaming reasoning interleaves
        // linearly with other kernel paths.
        let window = self.projected_window()?;

        // The streaming provider call only has to return the stream
        // handle inside the timeout — individual chunks are then governed
        // by downstream backpressure and cancellation. This still catches
        // the "provider hangs on handshake" failure mode. (Wave 5 /
        // T2.6 + T7.3.)
        let timeout = std::time::Duration::from_millis(self.config.proposal_timeout_ms);
        let started = std::time::Instant::now();
        let (model, purpose) = (request.model.clone(), request.metadata.purpose);
        let thinking_effort = request.thinking_effort;
        let stream =
            match tokio::time::timeout(timeout, self.provider.complete_streaming(request)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    // A stream never materialized, so there is no
                    // `ReasonStreamHandle::finalize` seam to catch this.
                    // Invariant §3 strict: record the handshake failure
                    // directly. The audit log keeps the formatted message;
                    // the typed `ReasonerError` is preserved through
                    // `KernelError::Reasoner` so the agent loop can keep
                    // its variant-based retry classification.
                    let failure = CallFailure {
                        kind: "reason_streaming",
                        stage: "streaming_handshake",
                        log_message: "failed to record streaming handshake failure entry",
                    };
                    self.record_call_failure(&failure, &e.to_string(), (&model, purpose))?;
                    return Err(crate::KernelError::Reasoner(e));
                }
                Err(_) => {
                    let reason_str = format!(
                        "streaming model provider did not respond within {}ms",
                        self.config.proposal_timeout_ms
                    );
                    let failure = CallFailure {
                        kind: "reason_streaming",
                        stage: "streaming_handshake_timeout",
                        log_message: "failed to record streaming handshake timeout entry",
                    };
                    self.record_call_failure(&failure, &reason_str, (&model, purpose))?;
                    return Err(crate::KernelError::Timeout(reason_str));
                }
            };

        let handle = ReasonStreamHandle {
            kernel_store: self.store.clone(),
            agent_id: self.agent_id,
            seq_counter: self.seq.clone(),
            window,
            recent: self.recent.clone(),
            observer: self.config.record_observer.clone(),
            started,
            plan_usage: Vec::new(),
            thinking_effort,
            route: model.to_string(),
        };

        Ok((handle, stream))
    }

    /// The record window as of the next unclaimed number, without
    /// claiming it.
    fn projected_window(&self) -> Result<Vec<ContextHash>, crate::KernelError> {
        let projected_seq = {
            let guard = self
                .seq
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
        };
        self.window_hashes(projected_seq)
    }

    /// Claim a number and record a failed provider call; a failure to
    /// record is logged, not returned (only the claim can fail the call).
    fn record_call_failure(
        &self,
        failure: &CallFailure,
        reason_str: &str,
        call: (&ModelName, Option<ModelCallPurpose>),
    ) -> Result<(), crate::KernelError> {
        let claim = self.claim_seq()?;
        if let Err(record_err) =
            self.record_reasoning_failure(claim, failure.stage, reason_str, call)
        {
            error!(
                kind = failure.kind,
                error = %reason_str,
                record_error = %record_err,
                "{}",
                failure.log_message
            );
        }
        Ok(())
    }

    /// Append a `Reasoning` record entry describing a failed provider
    /// call to `model` made for `purpose`, at the claimed number. Used by
    /// both `reason` and `reason_streaming`'s handshake error paths to
    /// keep Invariant §3 strict: every LLM call attempt produces a record
    /// entry.
    fn record_reasoning_failure(
        &self,
        claim: Claim<'_>,
        stage: &str,
        error_reason: &str,
        (model, purpose): (&ModelName, Option<ModelCallPurpose>),
    ) -> Result<(), crate::KernelError> {
        let mut payload = serde_json::json!({
            "model": model,
            "route": model,
            "stop_reason": "Error",
            "stage": stage,
            "error": error_reason,
        });
        if let Some(purpose) = purpose {
            payload["purpose"] = serde_json::json!(purpose);
        }
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );
        let seq = claim.seq;
        let window = self.window_hashes(seq)?;
        let context_hash = hash_tx_with_hashes(&tx, &window)?;
        let entry = RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .build();
        self.store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;
        self.notify_entry(claim, &entry);
        Ok(())
    }
}
