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

use crate::cf;
use crate::error::StoreError;
use crate::keys::{AgentMetaKey, InboxKey, KeyCodec, RecordKey};
use crate::store::Store;
use aura_core::AgentStatus;
use aura_core::{AgentId, RecordEntry, Transaction};
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBWithThreadMode, IteratorMode, MultiThreaded,
    Options, WriteBatch, WriteOptions,
};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, instrument};

/// `RocksDB`-based store implementation.
pub struct RocksStore {
    db: Arc<DBWithThreadMode<MultiThreaded>>,
    sync_writes: bool,
}

impl RocksStore {
    /// Open or create a `RocksDB` store at the given path.
    ///
    /// # Errors
    /// Returns error if the database cannot be opened.
    pub fn open(path: impl AsRef<Path>, sync_writes: bool) -> Result<Self, StoreError> {
        let path = path.as_ref();
        debug!(?path, "Opening RocksDB store");

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        // Define column families
        let cf_names = [
            cf::RECORD,
            cf::AGENT_META,
            cf::INBOX,
            cf::MEMORY_FACTS,
            cf::MEMORY_EVENTS,
            cf::MEMORY_PROCEDURES,
            cf::AGENT_SKILLS,
        ];
        let cf_descriptors: Vec<_> = cf_names
            .iter()
            .map(|name| {
                let cf_opts = Options::default();
                ColumnFamilyDescriptor::new(*name, cf_opts)
            })
            .collect();

        let db =
            DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, path, cf_descriptors)?;

        Ok(Self {
            db: Arc::new(db),
            sync_writes,
        })
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

    /// Create write options based on `sync_writes` setting.
    fn write_opts(&self) -> WriteOptions {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync_writes);
        opts
    }

    /// Read a u64 value from agent metadata.
    fn read_meta_u64(&self, key: &AgentMetaKey) -> Result<u64, StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let encoded_key = key.encode();

        match self.db.get_cf(&cf, &encoded_key)? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| StoreError::Deserialization("invalid u64 bytes".to_string()))?;
                Ok(u64::from_be_bytes(arr))
            }
            None => Ok(0), // Default to 0 if not set
        }
    }
}

impl Store for RocksStore {
    #[instrument(skip(self, tx), fields(agent_id = %tx.agent_id, hash = %tx.hash))]
    fn enqueue_tx(&self, tx: &Transaction) -> Result<(), StoreError> {
        let cf_inbox = self.cf(cf::INBOX)?;
        let cf_meta = self.cf(cf::AGENT_META)?;

        // Get current inbox tail
        let tail_key = AgentMetaKey::inbox_tail(tx.agent_id);
        let tail = self.read_meta_u64(&tail_key)?;

        // Create inbox key
        let inbox_key = InboxKey::new(tx.agent_id, tail);

        // Serialize transaction
        let tx_bytes = serde_json::to_vec(tx)?;

        // Write batch: inbox entry + update tail
        let mut batch = WriteBatch::default();
        batch.put_cf(&cf_inbox, inbox_key.encode(), tx_bytes);
        batch.put_cf(&cf_meta, tail_key.encode(), (tail + 1).to_be_bytes());

        self.db.write_opt(batch, &self.write_opts())?;

        debug!(inbox_seq = tail, "Transaction enqueued");
        Ok(())
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn dequeue_tx(&self, agent_id: AgentId) -> Result<Option<(crate::store::DequeueToken, Transaction)>, StoreError> {
        let cf_inbox = self.cf(cf::INBOX)?;

        // Get current inbox head and tail
        let head_key = AgentMetaKey::inbox_head(agent_id);
        let tail_key = AgentMetaKey::inbox_tail(agent_id);
        let head = self.read_meta_u64(&head_key)?;
        let tail = self.read_meta_u64(&tail_key)?;

        // Check if inbox is empty
        if head >= tail {
            debug!("Inbox empty");
            return Ok(None);
        }

        // Read the transaction at head
        let inbox_key = InboxKey::new(agent_id, head);
        let encoded_key = inbox_key.encode();

        if let Some(bytes) = self.db.get_cf(&cf_inbox, &encoded_key)? {
            let tx: Transaction = serde_json::from_slice(&bytes)
                .map_err(|e| StoreError::Deserialization(e.to_string()))?;
            debug!(inbox_seq = head, "Transaction dequeued");
            let token = crate::store::DequeueToken { inbox_seq: head };
            Ok(Some((token, tx)))
        } else {
            Err(StoreError::InboxCorruption {
                agent_id,
                expected_seq: head,
            })
        }
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn get_head_seq(&self, agent_id: AgentId) -> Result<u64, StoreError> {
        let key = AgentMetaKey::head_seq(agent_id);
        self.read_meta_u64(&key)
    }

    #[instrument(skip(self, entry), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_atomic(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        dequeued_inbox_seq: u64,
    ) -> Result<(), StoreError> {
        let cf_record = self.cf(cf::RECORD)?;
        let cf_meta = self.cf(cf::AGENT_META)?;
        let cf_inbox = self.cf(cf::INBOX)?;

        // Verify sequence
        let current_head = self.get_head_seq(agent_id)?;
        if next_seq != current_head + 1 {
            return Err(StoreError::SequenceMismatch {
                expected: current_head + 1,
                actual: next_seq,
            });
        }

        // Serialize entry
        let entry_bytes = serde_json::to_vec(entry)?;

        // Create keys
        let record_key = RecordKey::new(agent_id, next_seq);
        let head_seq_key = AgentMetaKey::head_seq(agent_id);
        let inbox_key = InboxKey::new(agent_id, dequeued_inbox_seq);
        let inbox_head_key = AgentMetaKey::inbox_head(agent_id);

        // Atomic batch write
        let mut batch = WriteBatch::default();

        batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
        batch.put_cf(&cf_meta, head_seq_key.encode(), next_seq.to_be_bytes());
        batch.delete_cf(&cf_inbox, inbox_key.encode());
        batch.put_cf(
            &cf_meta,
            inbox_head_key.encode(),
            (dequeued_inbox_seq + 1).to_be_bytes(),
        );

        self.db.write_opt(batch, &self.write_opts())?;

        debug!("Record entry committed atomically");
        Ok(())
    }

    #[instrument(skip(self, entry), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_direct(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
    ) -> Result<(), StoreError> {
        let cf_record = self.cf(cf::RECORD)?;
        let cf_meta = self.cf(cf::AGENT_META)?;

        let current_head = self.get_head_seq(agent_id)?;
        if next_seq != current_head + 1 {
            return Err(StoreError::SequenceMismatch {
                expected: current_head + 1,
                actual: next_seq,
            });
        }

        let entry_bytes = serde_json::to_vec(entry)?;
        let record_key = RecordKey::new(agent_id, next_seq);
        let head_seq_key = AgentMetaKey::head_seq(agent_id);

        let mut batch = WriteBatch::default();
        batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
        batch.put_cf(&cf_meta, head_seq_key.encode(), next_seq.to_be_bytes());

        self.db.write_opt(batch, &self.write_opts())?;

        debug!("Record entry committed (direct)");
        Ok(())
    }

    #[instrument(skip(self, entries), fields(agent_id = %agent_id, base_seq, count = entries.len()))]
    fn append_entries_batch(
        &self,
        agent_id: AgentId,
        base_seq: u64,
        entries: &[RecordEntry],
    ) -> Result<(), StoreError> {
        if entries.is_empty() {
            return Ok(());
        }

        let cf_record = self.cf(cf::RECORD)?;
        let cf_meta = self.cf(cf::AGENT_META)?;

        let current_head = self.get_head_seq(agent_id)?;
        if base_seq != current_head + 1 {
            return Err(StoreError::SequenceMismatch {
                expected: current_head + 1,
                actual: base_seq,
            });
        }

        let mut batch = WriteBatch::default();
        let head_seq_key = AgentMetaKey::head_seq(agent_id);

        for (i, entry) in entries.iter().enumerate() {
            let seq = base_seq + i as u64;
            let entry_bytes = serde_json::to_vec(entry)?;
            let record_key = RecordKey::new(agent_id, seq);
            batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
        }

        let last_seq = base_seq + entries.len() as u64 - 1;
        batch.put_cf(&cf_meta, head_seq_key.encode(), last_seq.to_be_bytes());

        self.db.write_opt(batch, &self.write_opts())?;

        debug!(last_seq, "Batch record entries committed");
        Ok(())
    }

    #[instrument(skip(self), fields(agent_id = %agent_id, from_seq, limit))]
    fn scan_record(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, StoreError> {
        let cf = self.cf(cf::RECORD)?;

        let start_key = RecordKey::scan_from(agent_id, from_seq);
        let end_key = RecordKey::scan_end(agent_id);

        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );

        let mut entries = Vec::with_capacity(limit);

        for item in iter {
            let (key, value) = item?;

            if key.as_ref() >= end_key.as_slice() {
                break;
            }

            let record_key = RecordKey::decode(&key)?;

            if record_key.agent_id != agent_id {
                break;
            }

            let entry = serde_json::from_slice::<RecordEntry>(&value).map_err(|e| {
                StoreError::Deserialization(format!("record seq={}: {e}", record_key.seq))
            })?;
            entries.push(entry);

            if entries.len() >= limit {
                break;
            }
        }

        debug!(count = entries.len(), "Record scan complete");
        Ok(entries)
    }

    #[instrument(skip(self), fields(agent_id = %agent_id, seq))]
    fn get_record_entry(&self, agent_id: AgentId, seq: u64) -> Result<RecordEntry, StoreError> {
        let cf = self.cf(cf::RECORD)?;
        let key = RecordKey::new(agent_id, seq);

        match self.db.get_cf(&cf, key.encode())? {
            Some(bytes) => {
                let entry: RecordEntry = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Deserialization(e.to_string()))?;
                Ok(entry)
            }
            None => Err(StoreError::RecordEntryNotFound(agent_id, seq)),
        }
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn get_agent_status(&self, agent_id: AgentId) -> Result<AgentStatus, StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let key = AgentMetaKey::status(agent_id);

        match self.db.get_cf(&cf, key.encode())? {
            Some(bytes) => {
                if bytes.is_empty() {
                    return Ok(AgentStatus::default());
                }
                AgentStatus::from_byte(bytes[0])
                    .ok_or_else(|| StoreError::Deserialization("invalid agent status".to_string()))
            }
            None => Ok(AgentStatus::default()),
        }
    }

    #[instrument(skip(self), fields(agent_id = %agent_id, ?status))]
    fn set_agent_status(&self, agent_id: AgentId, status: AgentStatus) -> Result<(), StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let key = AgentMetaKey::status(agent_id);

        self.db
            .put_cf_opt(&cf, key.encode(), [status.as_byte()], &self.write_opts())?;
        Ok(())
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn has_pending_tx(&self, agent_id: AgentId) -> Result<bool, StoreError> {
        let head = self.read_meta_u64(&AgentMetaKey::inbox_head(agent_id))?;
        let tail = self.read_meta_u64(&AgentMetaKey::inbox_tail(agent_id))?;
        Ok(tail > head)
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn get_inbox_depth(&self, agent_id: AgentId) -> Result<u64, StoreError> {
        let head = self.read_meta_u64(&AgentMetaKey::inbox_head(agent_id))?;
        let tail = self.read_meta_u64(&AgentMetaKey::inbox_tail(agent_id))?;
        Ok(tail.saturating_sub(head))
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_concurrent;
