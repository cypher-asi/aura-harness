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

use super::{Kernel, ReasonResult, ReasonStreamHandle};
use crate::context::hash_tx_with_window;
use aura_core::{RecordEntry, Transaction, TransactionType};
use aura_reasoner::{ModelRequest, StreamEventStream};

impl Kernel {
    /// Call the model provider and record the result.
    ///
    /// # Errors
    /// Returns error if the model call or storage fails.
    pub async fn reason(&self, request: ModelRequest) -> Result<ReasonResult, crate::KernelError> {
        let seq = self.next_seq();

        // Bound the reasoner call with the kernel-config timeout so a hung
        // provider cannot stall the agent indefinitely. (Wave 5 / T2.6 +
        // T7.3.)
        let timeout = std::time::Duration::from_millis(self.config.proposal_timeout_ms);
        let response = match tokio::time::timeout(timeout, self.provider.complete(request)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(crate::KernelError::Reasoner(e.to_string())),
            Err(_) => {
                return Err(crate::KernelError::Timeout(format!(
                    "model provider did not respond within {}ms",
                    self.config.proposal_timeout_ms
                )));
            }
        };

        let reasoning_payload = serde_json::json!({
            "model": response.trace.model,
            "stop_reason": format!("{:?}", response.stop_reason),
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
        });
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        let window = self.load_window(seq)?;
        let context_hash = hash_tx_with_window(&tx, &window)?;

        let entry = RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .build();

        self.store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;

        Ok(ReasonResult { entry, response })
    }

    /// Start a streaming reasoning call.
    ///
    /// Returns a handle for finalizing the record entry and the event stream.
    ///
    /// # Errors
    /// Returns error if the model call fails.
    pub async fn reason_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<(ReasonStreamHandle, StreamEventStream), crate::KernelError> {
        // Snapshot the record window *before* invoking the provider so
        // the context hash computed at finalization time depends solely
        // on the state observed at the start of the reasoning call
        // (Invariant §6). The sequence number itself is only reserved
        // atomically at `append` time so streaming reasoning interleaves
        // linearly with other kernel paths.
        let projected_seq = {
            let guard = self
                .seq
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
        };
        let window = self.load_window(projected_seq)?;

        // The streaming provider call only has to return the stream
        // handle inside the timeout — individual chunks are then governed
        // by downstream backpressure and cancellation. This still catches
        // the "provider hangs on handshake" failure mode. (Wave 5 /
        // T2.6 + T7.3.)
        let timeout = std::time::Duration::from_millis(self.config.proposal_timeout_ms);
        let stream =
            match tokio::time::timeout(timeout, self.provider.complete_streaming(request)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(crate::KernelError::Reasoner(e.to_string())),
                Err(_) => {
                    return Err(crate::KernelError::Timeout(format!(
                        "streaming model provider did not respond within {}ms",
                        self.config.proposal_timeout_ms
                    )));
                }
            };

        let handle = ReasonStreamHandle {
            kernel_store: self.store.clone(),
            agent_id: self.agent_id,
            seq_counter: self.seq.clone(),
            window,
        };

        Ok((handle, stream))
    }
}
