//! Bridge from [`RocksStore`] to the
//! [`aura_store_record::RecordLog`] trait.
//!
//! Phase 2 carve-out: layered consumers (kernel, fleet) bind to a
//! small `Arc<dyn RecordLog>` instead of the full
//! `WriteStore`/`ReadStore` surface. The bridge here delegates to the
//! existing `append_entry_direct` and `get_head_seq` paths and maps
//! their [`StoreError`](crate::StoreError) results into the
//! trait-level [`RecordLogError`].
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - `append` delegates to [`crate::WriteStore::append_entry_direct`],
//!   which itself enforces the Invariant §10 atomic-commit protocol
//!   inside a single `RocksDB` `WriteBatch`. The trait's
//!   strict-monotone-seq invariant is therefore preserved by the
//!   underlying implementation, with [`StoreError::SequenceMismatch`]
//!   mapped to [`RecordLogError::SeqOutOfOrder`] so callers can react
//!   without parsing the backend error text.
//! - Callers wanting a trait-object handle (`Arc<dyn RecordLog>`)
//!   can rely on Rust's built-in DST-unsizing coercion from
//!   `Arc<RocksStore>` — no per-`Arc` blanket impl is required and
//!   the orphan rule rules out adding one here anyway.
//!
//! ## Failure modes
//!
//! - [`StoreError::SequenceMismatch`] → [`RecordLogError::SeqOutOfOrder`].
//! - Every other [`StoreError`] is wrapped as
//!   [`RecordLogError::Backend`] with a short operation tag plus the
//!   original error's `Display` form.

use crate::store::WriteStore;
use crate::{RocksStore, StoreError};
use aura_core::AgentId;
use aura_store_record::{RecordEntry, RecordLog, RecordLogError};

fn map_append_error(agent_id: &AgentId, entry_seq: u64, err: &StoreError) -> RecordLogError {
    if let StoreError::SequenceMismatch { expected, actual } = err {
        return RecordLogError::SeqOutOfOrder {
            agent_id: *agent_id,
            expected: *expected,
            actual: *actual,
        };
    }
    RecordLogError::Backend(format!(
        "RocksStore::append_entry_direct(agent_id={agent_id}, seq={entry_seq}): {err}"
    ))
}

fn map_head_seq_error(agent_id: &AgentId, err: &StoreError) -> RecordLogError {
    RecordLogError::Backend(format!(
        "RocksStore::get_head_seq(agent_id={agent_id}): {err}"
    ))
}

fn map_scan_error(agent_id: &AgentId, from_seq: u64, err: &StoreError) -> RecordLogError {
    RecordLogError::Backend(format!(
        "RocksStore::scan_record(agent_id={agent_id}, from_seq={from_seq}): {err}"
    ))
}

impl RecordLog for RocksStore {
    fn append(&self, agent_id: &AgentId, entry: &RecordEntry) -> Result<(), RecordLogError> {
        WriteStore::append_entry_direct(self, *agent_id, entry.seq, entry)
            .map_err(|err| map_append_error(agent_id, entry.seq, &err))
    }

    fn head_seq(&self, agent_id: &AgentId) -> Result<u64, RecordLogError> {
        crate::store::ReadStore::get_head_seq(self, *agent_id)
            .map_err(|err| map_head_seq_error(agent_id, &err))
    }

    fn scan(
        &self,
        agent_id: &AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, RecordLogError> {
        crate::store::ReadStore::scan_record(self, *agent_id, from_seq, limit)
            .map_err(|err| map_scan_error(agent_id, from_seq, &err))
    }
}
