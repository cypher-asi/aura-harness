//! Fault-injection helpers (Wave 7 / T2 — invariant_atomicity suite).
//!
//! Only tests call these. They are in every build, hidden from the docs,
//! so the app and the tests of every crate share one build of this crate
//! instead of a feature that rebuilds it. They exist solely to
//! exercise Invariant §10 against `RocksStore::append_entry_atomic` —
//! specifically to prove that a record-write and the paired inbox-delete
//! either both commit or both roll back, under every simulated fault
//! point.

use super::RocksStore;
use crate::cf;
use crate::error::{backend, StoreError};
use crate::keys::{AgentMetaKey, KeyCodec, RecordKey};
use crate::store::ReadStore;
use aura_core_types::{AgentId, RecordEntry};
use rocksdb::{DBWithThreadMode, MultiThreaded, WriteBatch, WriteOptions};

/// Fault point for [`RocksStore::append_entry_atomic_with_fault`].
///
/// See [`RocksStore::append_entry_atomic_with_fault`] for semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAt {
    /// Fail the call before issuing the `WriteBatch`. Expected outcome:
    /// no record is appended and no inbox slot is deleted.
    BeforeBatchWrite,
    /// Fail the call **after** the `WriteBatch` commits. Expected
    /// outcome: the record is appended **and** the inbox slot is
    /// deleted (the atomic commit is durable even though the caller
    /// observes an error).
    AfterBatchWrite,
    /// Deliberately write the record entry in one `WriteBatch`, fail
    /// before queueing the paired inbox delete, and return an error.
    /// This is the "broken non-atomic path" the test uses to prove
    /// that the real atomic path is strictly required by Invariant §10.
    /// Observing partial state here is expected and documents the
    /// failure mode that `append_entry_atomic` prevents.
    InsideBatch,
}

/// Why [`write_batch_with_fault`] failed.
#[derive(Debug)]
pub(crate) enum FaultWriteError {
    /// The writes the fault mode calls for went through; this is the
    /// injected failure.
    Injected(FaultAt),
    /// A real write failed before the fault was reached.
    Write(rocksdb::Error),
}

/// Write `full` or `partial` the way `fault` says, then fail. Shared by
/// the fault hooks of stores that commit through one `WriteBatch`:
///
/// * [`FaultAt::BeforeBatchWrite`]: writes nothing.
/// * [`FaultAt::AfterBatchWrite`]: commits `full`.
/// * [`FaultAt::InsideBatch`]: commits only `partial`, the part of the
///   change a non-atomic writer would have got onto disk.
///
/// It never succeeds, so the return value is the error.
pub(crate) fn write_batch_with_fault(
    db: &DBWithThreadMode<MultiThreaded>,
    opts: &WriteOptions,
    full: WriteBatch,
    partial: WriteBatch,
    fault: FaultAt,
) -> FaultWriteError {
    let written = match fault {
        FaultAt::BeforeBatchWrite => Ok(()),
        FaultAt::AfterBatchWrite => db.write_opt(full, opts),
        FaultAt::InsideBatch => db.write_opt(partial, opts),
    };
    match written {
        Ok(()) => FaultWriteError::Injected(fault),
        Err(error) => FaultWriteError::Write(error),
    }
}

#[doc(hidden)]
impl RocksStore {
    /// Fault-injected variant of `append_entry_atomic`. See [`FaultAt`].
    ///
    /// # Errors
    /// Always returns an error in every fault mode so callers can assert
    /// both "the call failed" and the resulting store state in a single
    /// shot.
    pub fn append_entry_atomic_with_fault(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        dequeued_inbox_seq: u64,
        fault: FaultAt,
    ) -> Result<(), StoreError> {
        // Fault helper reuses `StoreError::InvalidKey` as a general-purpose
        // error carrier for injected faults so we don't have to grow the
        // public `StoreError` enum for test-only purposes. Callers assert
        // on the `injected fault:` prefix.
        let fault_err =
            |tag: &str| -> StoreError { StoreError::InvalidKey(format!("injected fault: {tag}")) };
        match fault {
            FaultAt::BeforeBatchWrite => Err(fault_err("BeforeBatchWrite")),
            FaultAt::AfterBatchWrite => {
                // Commit the full atomic batch first, then synthesize a
                // caller-side failure. This simulates e.g. a network
                // error after RocksDB has already fsynced.
                self.append_entry_atomic_internal(
                    agent_id,
                    next_seq,
                    entry,
                    dequeued_inbox_seq,
                    None,
                    false,
                )?;
                Err(fault_err("AfterBatchWrite"))
            }
            FaultAt::InsideBatch => {
                // Deliberately non-atomic: write the record entry in
                // one batch, then bail before queueing the inbox
                // delete. The assertion in the paired test is that
                // this path produces *partial* state — which is why
                // the real `append_entry_atomic` uses a single
                // `WriteBatch` containing both the record put and the
                // inbox delete.
                let cf_record = self.cf(cf::RECORD)?;
                let cf_meta = self.cf(cf::AGENT_META)?;

                let current_head = self.get_head_seq(agent_id)?;
                if next_seq != current_head + 1 {
                    return Err(StoreError::SequenceMismatch {
                        expected: current_head + 1,
                        actual: next_seq,
                    });
                }

                let entry_bytes = self.seal_value(serde_json::to_vec(entry)?)?;
                let record_key = RecordKey::new(agent_id, next_seq);
                let head_seq_key = AgentMetaKey::head_seq(agent_id);

                let mut batch = WriteBatch::default();
                batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
                batch.put_cf(&cf_meta, head_seq_key.encode(), next_seq.to_be_bytes());
                self.db
                    .write_opt(batch, &self.write_opts())
                    .map_err(backend)?;

                // Intentionally no inbox delete — this is the bug.
                Err(fault_err("InsideBatch (broken non-atomic path)"))
            }
        }
    }
}
