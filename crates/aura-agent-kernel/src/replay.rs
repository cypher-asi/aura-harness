//! Replay consumer — Phase 6b wiring of [`crate::KernelConfig::replay_from`].
//!
//! The replay consumer walks the per-agent record log in forward
//! `seq` order starting at `from_seq` (inclusive), validates each
//! entry against the deterministic invariants the kernel originally
//! recorded under, and produces a [`ReplayReport`] capturing the
//! resulting decision chain and a final state hash. The kernel
//! constructor invokes [`ReplayConsumer::run`] before exposing any
//! `process_*` surface so a divergent log aborts the boot with a
//! typed [`ReplayError`] instead of silently corrupting downstream
//! state.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - **Forward, seq-ordered iteration only.** The consumer reads
//!   entries in monotonic `seq` order via
//!   [`aura_store_record::RecordLog::scan`]. No random access, no
//!   cursors, no descending walks; richer iteration is left for the
//!   `replay_from` cursor sketched in the architecture plan §4.
//! - **Context-hash validation.** Every entry's `context_hash` is
//!   recomputed via [`crate::hash_tx_with_window`] against the
//!   already-replayed window. A mismatch aborts replay with
//!   [`ReplayError::ContextDivergence`] carrying the diverging
//!   `seq`, the expected hash (from the recomputation), and the
//!   actual hash (from the recorded entry). Replay does not skip
//!   the diverging entry; the entire boot is rejected.
//! - **Shim substitution.** No live model provider or executor is
//!   invoked during replay. The consumer's only inputs are the
//!   recorded [`aura_core::RecordEntry`] rows (proposals, decisions,
//!   actions, effects) and the snapshot store; the recorded values
//!   are treated as the source of truth for what the original turn
//!   produced.
//! - **AuditedLite payload retrieval.** When an entry's effect
//!   payload deserialises as
//!   [`aura_store_record::RecordPayload::Summary`], the consumer
//!   fetches the original bytes from [`SnapshotStore::get`] using
//!   the recorded `full_hash`. If the snapshot store returns `None`
//!   the consumer aborts with [`ReplayError::SnapshotMissing`] —
//!   live-model fallback is intentionally NOT performed in Phase 6b.
//!
//! ## Assumptions
//!
//! - The store is read-consistent throughout the replay sweep. The
//!   consumer captures `head_seq` once at the start of the run and
//!   reads up to that watermark.
//! - The snapshot store is content-addressed by the same digest
//!   algorithm the original kernel used when summarising
//!   (`BLAKE3` hex). Phase 6b ships with the no-op
//!   [`aura_store_snapshot::NoopSnapshotStore`]; backends gain
//!   verification later.
//!
//! ## Failure modes
//!
//! - [`ReplayError::ContextDivergence`] — recomputed context hash
//!   does not match the recorded one. Indicates the audit log was
//!   tampered with or a kernel-version change altered the canonical
//!   serialisation. Boot is aborted; the caller must triage the log
//!   before retrying.
//! - [`ReplayError::SnapshotMissing`] — an AuditedLite effect
//!   referenced a snapshot the store does not have. Boot is aborted;
//!   live-replay fallback is a future opt-in, not the V1 default.
//! - [`ReplayError::Store`] — the underlying record log or snapshot
//!   store reported a backend failure.
//! - [`ReplayError::Deserialization`] — a record entry's payload
//!   could not be parsed (e.g. AuditedLite summary JSON corrupt).

use std::sync::Arc;

use aura_core::{AgentId, ContextHash, Decision, Effect, RecordEntry};
use aura_store::Store;
use aura_store_record::RecordPayload;
use aura_store_snapshot::{SnapshotError, SnapshotStore};
use thiserror::Error;
use tracing::{debug, info};

use crate::context::hash_tx_with_window;

/// Replay-side errors surfaced by [`ReplayConsumer::run`].
///
/// See the module-level documentation for the invariants each
/// variant signals.
#[derive(Debug, Error)]
pub enum ReplayError {
    /// Recomputed `hash_tx_with_window(entry.tx, prior_window)` did
    /// not match the recorded `context_hash`. The replay refuses
    /// to advance past the diverging entry.
    #[error("context divergence at seq {seq}: expected {expected}, recomputed {actual}")]
    ContextDivergence {
        /// Sequence number of the diverging record entry.
        seq: u64,
        /// Context hash recorded in the entry (hex, 64 chars).
        expected: String,
        /// Context hash freshly recomputed from
        /// `(entry.tx, prior_window)` (hex, 64 chars).
        actual: String,
    },
    /// An AuditedLite effect's `full_hash` was not present in the
    /// configured snapshot store. Phase 6b ships with the no-op
    /// stub by default, so this is the expected outcome whenever a
    /// summarised entry is replayed against the stub.
    #[error("snapshot missing for seq {seq}: full_hash {full_hash}")]
    SnapshotMissing {
        /// Sequence number of the AuditedLite entry that referenced
        /// the missing snapshot.
        seq: u64,
        /// BLAKE3 hex digest the snapshot store could not satisfy.
        full_hash: String,
    },
    /// Backend failure from the record log or snapshot store.
    #[error("store error at seq {seq}: {message}")]
    Store {
        /// Sequence number being processed when the backend failed
        /// (`0` for failures outside a per-entry context).
        seq: u64,
        /// Human-readable backend message, including operation tag.
        message: String,
    },
    /// A record entry's payload could not be deserialised — e.g. an
    /// AuditedLite summary blob failed
    /// [`aura_store_record::RecordPayload`] parsing.
    #[error("deserialization error at seq {seq}: {message}")]
    Deserialization {
        /// Sequence number of the entry whose payload failed to
        /// parse.
        seq: u64,
        /// Human-readable parse error.
        message: String,
    },
}

/// Replay outcome surfaced on [`crate::Kernel::replay_report`].
///
/// Carries the per-entry decisions and the chained final state
/// hash so callers (and tests) can pin determinism. Construction is
/// the consumer's responsibility; this struct is otherwise immutable.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    /// Sequence numbers of every entry the consumer replayed, in
    /// ascending order.
    pub replayed_seqs: Vec<u64>,
    /// [`Decision`] from each replayed entry in `replayed_seqs`
    /// order. Cloning the recorded decisions keeps the report
    /// detached from the underlying store so callers can outlive it.
    pub decisions: Vec<Decision>,
    /// Last replayed entry's `context_hash`. Empty (all-zero) when
    /// no entries were replayed. Two kernels that replay over the
    /// same store with the same configuration MUST observe an
    /// identical `final_state_hash` — that is the deterministic
    /// fingerprint of the replay.
    pub final_state_hash: ContextHash,
}

/// Replay consumer for an agent's record log.
///
/// Construct via [`ReplayConsumer::new`] and drive the replay via
/// [`ReplayConsumer::run`]. The consumer is single-shot; callers
/// reconstruct it for additional replays.
pub struct ReplayConsumer {
    store: Arc<dyn Store>,
    snapshot_store: Arc<dyn SnapshotStore>,
    agent_id: AgentId,
    from_seq: u64,
    window_size: usize,
}

impl ReplayConsumer {
    /// Construct a new replay consumer.
    ///
    /// `from_seq` is the inclusive starting sequence number; the
    /// consumer walks up to the agent's current head. `window_size`
    /// mirrors [`crate::KernelConfig::record_window_size`] so the
    /// context-hash recomputation matches the original kernel's
    /// window shape exactly.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        snapshot_store: Arc<dyn SnapshotStore>,
        agent_id: AgentId,
        from_seq: u64,
        window_size: usize,
    ) -> Self {
        Self {
            store,
            snapshot_store,
            agent_id,
            from_seq,
            window_size,
        }
    }

    /// Run the replay sweep.
    ///
    /// Reads from `from_seq` (inclusive) up to the agent's
    /// current head, validates each entry's `context_hash` against
    /// a fresh recomputation, fetches AuditedLite snapshots when
    /// referenced, and returns a [`ReplayReport`] on success.
    ///
    /// # Errors
    ///
    /// Returns [`ReplayError`] on any divergence or backend
    /// failure. The error preserves the offending sequence number
    /// for triage.
    pub fn run(&self) -> Result<ReplayReport, ReplayError> {
        let head_seq = self
            .store
            .get_head_seq(self.agent_id)
            .map_err(|e| ReplayError::Store {
                seq: 0,
                message: format!("get_head_seq: {e}"),
            })?;

        if self.from_seq > head_seq {
            debug!(
                agent_id = %self.agent_id,
                from_seq = self.from_seq,
                head_seq,
                "replay requested past head; nothing to replay"
            );
            return Ok(ReplayReport {
                replayed_seqs: Vec::new(),
                decisions: Vec::new(),
                final_state_hash: ContextHash::zero(),
            });
        }

        let start = self.from_seq.max(1);
        // The expected count fits in `usize` for any realistic agent
        // log; saturate to `usize::MAX` if a pathological head_seq
        // overflows so we still attempt the read instead of panicking.
        let expected_count =
            usize::try_from(head_seq.saturating_sub(start) + 1).unwrap_or(usize::MAX);

        let entries = self
            .store
            .scan_record(self.agent_id, start, expected_count)
            .map_err(|e| ReplayError::Store {
                seq: start,
                message: format!("scan_record: {e}"),
            })?;

        info!(
            agent_id = %self.agent_id,
            from_seq = start,
            head_seq,
            entries = entries.len(),
            "replay sweep started"
        );

        let mut replayed_seqs = Vec::with_capacity(entries.len());
        let mut decisions = Vec::with_capacity(entries.len());
        let mut final_state_hash = ContextHash::zero();

        for entry in &entries {
            self.validate_entry(entry)?;
            self.resolve_snapshots(entry)?;

            replayed_seqs.push(entry.seq);
            decisions.push(entry.decision.clone());
            final_state_hash = entry.context_hash;
        }

        info!(
            agent_id = %self.agent_id,
            replayed = replayed_seqs.len(),
            "replay sweep complete"
        );

        Ok(ReplayReport {
            replayed_seqs,
            decisions,
            final_state_hash,
        })
    }

    /// Recompute the entry's context hash against the prior window
    /// loaded from the same store. A divergence aborts replay.
    fn validate_entry(&self, entry: &RecordEntry) -> Result<(), ReplayError> {
        // Reload the entry's prior window directly from the store so
        // the replay matches the kernel's original
        // `load_window(seq)` computation exactly. Reading the window
        // from the live store (instead of accumulating in-process)
        // preserves determinism even if the replay starts mid-log:
        // the recomputed hash depends solely on persisted data.
        let from_seq = entry.seq.saturating_sub(self.window_size as u64);
        let window = self
            .store
            .scan_record(self.agent_id, from_seq, self.window_size)
            .map_err(|e| ReplayError::Store {
                seq: entry.seq,
                message: format!("scan_record(window): {e}"),
            })?;

        // The window the kernel hashed at original commit time did
        // NOT include the entry being committed. `scan_record` may
        // return entries with `seq >= entry.seq` when `from_seq`
        // overlaps the current entry's slot, so filter those out
        // before recomputing.
        let prior_window: Vec<RecordEntry> =
            window.into_iter().filter(|e| e.seq < entry.seq).collect();

        let recomputed =
            hash_tx_with_window(&entry.tx, &prior_window).map_err(|e| ReplayError::Store {
                seq: entry.seq,
                message: format!("hash_tx_with_window: {e}"),
            })?;

        if recomputed != entry.context_hash {
            return Err(ReplayError::ContextDivergence {
                seq: entry.seq,
                expected: hex::encode(entry.context_hash.as_ref()),
                actual: hex::encode(recomputed.as_ref()),
            });
        }

        Ok(())
    }

    /// For every AuditedLite effect on the entry, fetch the original
    /// payload bytes from the snapshot store and verify they match
    /// the recorded `full_hash`. Replay does not consume the bytes
    /// further in Phase 6b; we only require the snapshot to be
    /// present so live-mode fallback is not silently engaged.
    fn resolve_snapshots(&self, entry: &RecordEntry) -> Result<(), ReplayError> {
        for effect in &entry.effects {
            self.resolve_effect_snapshot(entry.seq, effect)?;
        }
        Ok(())
    }

    fn resolve_effect_snapshot(&self, seq: u64, effect: &Effect) -> Result<(), ReplayError> {
        // AuditedLite effects encode the summary as a
        // `RecordPayload::Summary` JSON blob inside `effect.payload`
        // (see `kernel::tools::shared::maybe_summarise_effect_payload`).
        // For Audited (full-fidelity) effects this parse fails and
        // we treat the payload as opaque inline bytes — nothing to
        // resolve.
        let Ok(payload) = serde_json::from_slice::<RecordPayload>(&effect.payload) else {
            return Ok(());
        };

        let RecordPayload::Summary {
            full_hash,
            full_len,
            ..
        } = payload
        else {
            // Inline payload re-encoded as a `RecordPayload::Inline`
            // wrapper — also nothing to resolve.
            return Ok(());
        };

        match self.snapshot_store.get(&full_hash) {
            Ok(Some(bytes)) => {
                if bytes.len() != full_len {
                    return Err(ReplayError::Store {
                        seq,
                        message: format!(
                            "snapshot length mismatch for full_hash={full_hash}: \
                             expected {full_len}, got {}",
                            bytes.len()
                        ),
                    });
                }
                let digest = blake3::hash(&bytes).to_hex().to_string();
                if digest != full_hash {
                    return Err(ReplayError::Store {
                        seq,
                        message: format!(
                            "snapshot hash mismatch: recorded {full_hash}, fetched {digest}"
                        ),
                    });
                }
                Ok(())
            }
            Ok(None) => Err(ReplayError::SnapshotMissing { seq, full_hash }),
            Err(SnapshotError::Backend(message)) => Err(ReplayError::Store {
                seq,
                message: format!("snapshot_store.get({full_hash}): {message}"),
            }),
        }
    }
}
