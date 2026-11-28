//! `RocksDB` implementation of the Store trait.
//!
//! # Atomic Commit Protocol
//!
//! All mutations that involve more than one column family use [`WriteBatch`] to
//! guarantee **all-or-nothing** semantics.  `RocksDB` applies a `WriteBatch` as a
//! single atomic unit: either every put/delete in the batch is durably written,
//! or none of them are.
//!
//! The key multi-step operation is [`Store::append_entry_atomic`], which
//! performs four writes in one batch:
//!
//! 1. **Put** the serialised [`RecordEntry`] into the `record` column family.
//! 2. **Put** the updated `head_seq` into `agent_meta`.
//! 3. **Delete** the consumed inbox entry from the `inbox` column family.
//! 4. **Put** the advanced `inbox_head` cursor into `agent_meta`.
//!
//! Because these four operations share one `WriteBatch`, it is impossible to
//! observe a state where the record was written but the inbox was not advanced,
//! or vice-versa.  Transaction enqueue ([`Store::enqueue_tx`]) likewise batches
//! the inbox entry write with the tail-cursor update.
//!
//! # Failure Modes
//!
//! * **Partial writes are impossible** – the `WriteBatch` contract prevents
//!   them at the `RocksDB` level.
//! * **Sequence mismatch** – `append_entry_atomic` validates that `next_seq ==
//!   current_head + 1` before writing; a mismatch returns
//!   [`StoreError::SequenceMismatch`] without mutating state.
//! * **Disk-level failures** (e.g. full disk, storage corruption) may leave the
//!   WAL or SST files in an inconsistent state. `RocksDB`'s WAL replay can
//!   recover from crashes mid-write, but hardware-level corruption (bit-rot,
//!   torn sectors) may require restoring from backup.
//! * **`sync_writes`** controls whether each `WriteBatch` issues an `fsync`.
//!   When disabled, a process crash can lose committed batches that haven't
//!   been flushed to disk yet.
//!
//! # Layout
//!
//! * `open`: opening the database, column families and their options.
//! * `read_store`: the [`crate::ReadStore`] impl (record reads, inbox,
//!   agent status, runtime capabilities, tool defaults, processing claims).
//! * `append`: record appends and the [`crate::WriteStore`] impl.
//! * `fault`: fault-injection hooks for the atomicity tests.

#[cfg(doc)]
use aura_core_types::RecordEntry;
#[cfg(doc)]
use rocksdb::WriteBatch;

use crate::error::StoreError;
use crate::seal::SealCipher;
use rocksdb::{BoundColumnFamily, DBWithThreadMode, MultiThreaded, WriteOptions};
use std::sync::{Arc, Mutex};

mod append;
mod fault;
mod open;
mod read_store;

pub use fault::FaultAt;
pub(crate) use fault::{write_batch_with_fault, FaultWriteError};

/// How many locks record appends are spread over. An agent always takes
/// the same one, so different agents mostly commit at once.
const APPEND_SHARDS: u64 = 64;

/// `RocksDB`-based store implementation.
pub struct RocksStore {
    db: Arc<DBWithThreadMode<MultiThreaded>>,
    sync_writes: bool,
    processing_claim_lock: Mutex<()>,
    /// Serializes the record-append read-modify-write so the
    /// `get_head_seq` / `assert_next_seq` pre-flight and the paired
    /// `WriteBatch` commit are atomic with respect to each other.
    ///
    /// Without this, two concurrent appenders for the same agent can
    /// both read head `N`, both pass the `next_seq == N + 1` check, and
    /// both commit `seq = N + 1` — the second silently overwrites the
    /// first (a lost update) and leaves `head = N + 1`. The scheduler's
    /// per-agent processing claim prevents most contention, but
    /// out-of-band system appends (e.g. an agent-host fallback record
    /// written while the target's turn is in flight) are
    /// not covered by that claim. Holding this lock across the
    /// check+commit turns the only observable concurrency failure into
    /// a clean [`StoreError::SequenceMismatch`] the caller can retry.
    ///
    /// The race is between appends for one agent, so each agent's
    /// appends meet at one of [`APPEND_SHARDS`] locks
    /// ([`Self::append_lock`]): with many agents running, one agent's
    /// commit (an fsync with `sync_writes`) no longer holds up the rest.
    append_locks: Box<[Mutex<()>]>,
    /// Optional value-sealing cipher (Swarm TEE phase 5).
    ///
    /// `Some` when the harness booted in `AURA_STATE_ENCRYPTION=sealed`
    /// mode: every content-bearing value (record entries, inbox
    /// transactions, runtime-capability snapshots, user tool defaults)
    /// is AES-256-GCM sealed before the `WriteBatch` and opened after
    /// reads. `None` is the legacy plaintext path, byte-for-byte
    /// identical to the historical on-disk format. Keys and pure-counter
    /// metadata (head/tail cursors, status byte, processing claims)
    /// stay plaintext in both modes — see [`crate::seal`] module docs.
    cipher: Option<Arc<SealCipher>>,
}

impl RocksStore {
    /// Seal a serialized value when sealed mode is on; identity otherwise.
    fn seal_value(&self, plain: Vec<u8>) -> Result<Vec<u8>, StoreError> {
        match &self.cipher {
            Some(cipher) => cipher
                .seal(&plain)
                .map_err(|e| StoreError::Serialization(format!("sealing value: {e}"))),
            None => Ok(plain),
        }
    }

    /// Open a sealed value when sealed mode is on; borrow-through otherwise.
    fn open_value<'a>(&self, bytes: &'a [u8]) -> Result<std::borrow::Cow<'a, [u8]>, StoreError> {
        match &self.cipher {
            Some(cipher) => cipher
                .open(bytes)
                .map(std::borrow::Cow::Owned)
                .map_err(|e| StoreError::Deserialization(format!("opening sealed value: {e}"))),
            None => Ok(std::borrow::Cow::Borrowed(bytes)),
        }
    }

    /// Get a column family handle.
    fn cf(&self, name: &str) -> Result<Arc<BoundColumnFamily<'_>>, StoreError> {
        self.db
            .cf_handle(name)
            .ok_or_else(|| StoreError::ColumnFamilyNotFound(name.to_string()))
    }

    /// Expose the underlying `RocksDB` handle for subsystems (e.g. memory store)
    /// that share the same database instance.
    #[must_use]
    pub const fn db_handle(&self) -> &Arc<DBWithThreadMode<MultiThreaded>> {
        &self.db
    }

    /// The value-sealing cipher, in sealed mode, for subsystems that
    /// share the database and must seal the same way.
    #[must_use]
    pub const fn cipher(&self) -> Option<&Arc<SealCipher>> {
        self.cipher.as_ref()
    }

    /// Whether writes `fsync`, for subsystems that share the database
    /// and must be as durable.
    pub(crate) const fn sync_writes(&self) -> bool {
        self.sync_writes
    }

    /// Create write options based on `sync_writes` setting.
    fn write_opts(&self) -> WriteOptions {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync_writes);
        opts
    }
}

// Wave 2 T3: the legacy `impl Store for RocksStore` is split into the two
// narrower traits + the sealed marker so the type system enforces who is
// allowed to append to the record log (Invariant §10).
impl crate::store::sealed::Sealed for RocksStore {}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_concurrent;
