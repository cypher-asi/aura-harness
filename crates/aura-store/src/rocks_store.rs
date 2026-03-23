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
        let cf_names = [cf::RECORD, cf::AGENT_META, cf::INBOX];
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
    fn dequeue_tx(&self, agent_id: AgentId) -> Result<Option<(u64, Transaction)>, StoreError> {
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
            Ok(Some((head, tx)))
        } else {
            // This shouldn't happen if head < tail, but handle gracefully
            debug!("Inbox entry missing at head");
            Ok(None)
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

        // 1. Put record entry
        batch.put_cf(&cf_record, record_key.encode(), entry_bytes);

        // 2. Update head_seq
        batch.put_cf(&cf_meta, head_seq_key.encode(), next_seq.to_be_bytes());

        // 3. Delete inbox entry
        batch.delete_cf(&cf_inbox, inbox_key.encode());

        // 4. Update inbox_head cursor
        batch.put_cf(
            &cf_meta,
            inbox_head_key.encode(),
            (dequeued_inbox_seq + 1).to_be_bytes(),
        );

        self.db.write_opt(batch, &self.write_opts())?;

        debug!("Record entry committed atomically");
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

            // Check if we're still within the agent's range
            if key.as_ref() >= end_key.as_slice() {
                break;
            }

            // Decode and verify key
            let record_key = RecordKey::decode(&key)?;

            if record_key.agent_id != agent_id {
                break;
            }

            // Deserialize entry - skip records that don't match current schema
            match serde_json::from_slice::<RecordEntry>(&value) {
                Ok(entry) => {
                    entries.push(entry);
                }
                Err(_) => {
                    debug!(
                        seq = record_key.seq,
                        error_kind = "deserialization",
                        "skipping malformed record"
                    );
                    continue;
                }
            }

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
mod tests {
    use super::*;
    use aura_core::{Decision, Hash, ProposalSet, TransactionType};
    use bytes::Bytes;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_store() -> (RocksStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = RocksStore::open(dir.path(), false).unwrap();
        (store, dir)
    }

    fn create_test_tx(agent_id: AgentId) -> Transaction {
        Transaction::new(
            Hash::from_content(b"test"),
            agent_id,
            1000,
            TransactionType::UserPrompt,
            Bytes::from_static(b"test payload"),
        )
    }

    #[test]
    fn test_enqueue_dequeue() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();
        let tx = create_test_tx(agent_id);

        // Enqueue
        store.enqueue_tx(&tx).unwrap();

        // Dequeue
        let result = store.dequeue_tx(agent_id).unwrap();
        assert!(result.is_some());

        let (inbox_seq, dequeued_tx) = result.unwrap();
        assert_eq!(inbox_seq, 0);
        assert_eq!(dequeued_tx.tx_id(), tx.tx_id());
    }

    #[test]
    fn test_inbox_empty() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let result = store.dequeue_tx(agent_id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_atomic_commit() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();
        let tx = create_test_tx(agent_id);

        // Enqueue transaction
        store.enqueue_tx(&tx).unwrap();

        // Get head_seq (should be 0)
        let head_seq = store.get_head_seq(agent_id).unwrap();
        assert_eq!(head_seq, 0);

        // Dequeue
        let (inbox_seq, _) = store.dequeue_tx(agent_id).unwrap().unwrap();

        // Create record entry
        let entry = RecordEntry::builder(1, tx)
            .context_hash([0u8; 32])
            .proposals(ProposalSet::new())
            .decision(Decision::new())
            .build();

        // Atomic commit
        store
            .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
            .unwrap();

        // Verify head_seq updated
        let new_head = store.get_head_seq(agent_id).unwrap();
        assert_eq!(new_head, 1);

        // Verify inbox is empty
        assert!(!store.has_pending_tx(agent_id).unwrap());

        // Verify record entry exists
        let retrieved = store.get_record_entry(agent_id, 1).unwrap();
        assert_eq!(retrieved.seq, 1);
    }

    #[test]
    fn test_scan_record() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Create and commit 5 entries
        for i in 1..=5 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

            #[allow(clippy::cast_possible_truncation)] // i is always 1-5 in test
            let entry = RecordEntry::builder(i, tx)
                .context_hash([i as u8; 32])
                .build();

            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        // Scan all
        let entries = store.scan_record(agent_id, 1, 10).unwrap();
        assert_eq!(entries.len(), 5);

        // Scan from seq 3
        let entries = store.scan_record(agent_id, 3, 10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 3);

        // Scan with limit
        let entries = store.scan_record(agent_id, 1, 2).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_agent_status() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Default status
        let status = store.get_agent_status(agent_id).unwrap();
        assert_eq!(status, AgentStatus::Active);

        // Set paused
        store
            .set_agent_status(agent_id, AgentStatus::Paused)
            .unwrap();
        let status = store.get_agent_status(agent_id).unwrap();
        assert_eq!(status, AgentStatus::Paused);
    }

    #[test]
    fn test_sequence_mismatch() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();
        let tx = create_test_tx(agent_id);

        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

        let entry = RecordEntry::builder(5, tx) // Wrong seq - should be 1
            .build();

        let result = store.append_entry_atomic(agent_id, 5, &entry, inbox_seq);
        assert!(matches!(result, Err(StoreError::SequenceMismatch { .. })));
    }

    // ========================================================================
    // Edge Case Tests
    // ========================================================================

    #[test]
    fn test_empty_agent_state() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // New agent should have head_seq = 0
        assert_eq!(store.get_head_seq(agent_id).unwrap(), 0);

        // Inbox should be empty
        assert!(!store.has_pending_tx(agent_id).unwrap());
        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);

        // Dequeue should return None
        assert!(store.dequeue_tx(agent_id).unwrap().is_none());
    }

    #[test]
    fn test_multiple_agents_isolated() {
        let (store, _dir) = create_test_store();

        let agent1 = AgentId::generate();
        let agent2 = AgentId::generate();

        // Enqueue to both agents
        let tx1 = create_test_tx(agent1);
        let tx2 = create_test_tx(agent2);

        store.enqueue_tx(&tx1).unwrap();
        store.enqueue_tx(&tx2).unwrap();

        // Each agent should have exactly 1 pending tx
        assert_eq!(store.get_inbox_depth(agent1).unwrap(), 1);
        assert_eq!(store.get_inbox_depth(agent2).unwrap(), 1);

        // Process agent1
        let (inbox_seq, tx) = store.dequeue_tx(agent1).unwrap().unwrap();
        let entry = RecordEntry::builder(1, tx).build();
        store
            .append_entry_atomic(agent1, 1, &entry, inbox_seq)
            .unwrap();

        // Agent1 should have head_seq=1, agent2 should still have head_seq=0
        assert_eq!(store.get_head_seq(agent1).unwrap(), 1);
        assert_eq!(store.get_head_seq(agent2).unwrap(), 0);
    }

    #[test]
    fn test_large_inbox_depth() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Enqueue 100 transactions
        for _ in 0..100 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
        }

        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 100);

        // Process them all
        for seq in 1..=100 {
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(seq, tx).build();
            store
                .append_entry_atomic(agent_id, seq, &entry, inbox_seq)
                .unwrap();
        }

        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 0);
        assert_eq!(store.get_head_seq(agent_id).unwrap(), 100);
    }

    #[test]
    fn test_scan_empty_record() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let entries = store.scan_record(agent_id, 1, 10).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_scan_partial_range() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Create 10 entries
        for i in 1..=10 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        // Scan from middle
        let entries = store.scan_record(agent_id, 5, 3).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 5);
        assert_eq!(entries[1].seq, 6);
        assert_eq!(entries[2].seq, 7);
    }

    #[test]
    fn test_scan_beyond_end() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Create 5 entries
        for i in 1..=5 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        // Scan with limit beyond end
        let entries = store.scan_record(agent_id, 3, 100).unwrap();
        assert_eq!(entries.len(), 3); // Only entries 3, 4, 5
    }

    #[test]
    fn test_get_nonexistent_entry() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let result = store.get_record_entry(agent_id, 999);
        assert!(matches!(
            result,
            Err(StoreError::RecordEntryNotFound(_, 999))
        ));
    }

    #[test]
    fn test_agent_status_transitions() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        // Default should be Active
        assert_eq!(
            store.get_agent_status(agent_id).unwrap(),
            AgentStatus::Active
        );

        // Transition to Paused
        store
            .set_agent_status(agent_id, AgentStatus::Paused)
            .unwrap();
        assert_eq!(
            store.get_agent_status(agent_id).unwrap(),
            AgentStatus::Paused
        );

        // Transition to Dead
        store.set_agent_status(agent_id, AgentStatus::Dead).unwrap();
        assert_eq!(store.get_agent_status(agent_id).unwrap(), AgentStatus::Dead);

        // Can go back to Active
        store
            .set_agent_status(agent_id, AgentStatus::Active)
            .unwrap();
        assert_eq!(
            store.get_agent_status(agent_id).unwrap(),
            AgentStatus::Active
        );
    }

    #[test]
    fn test_transaction_payload_preserved() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let payload = b"complex payload with \x00 null bytes and unicode: \xC3\xA9";
        let tx = Transaction::new(
            Hash::from_content(payload),
            agent_id,
            1000,
            TransactionType::UserPrompt,
            Bytes::from(payload.to_vec()),
        );

        store.enqueue_tx(&tx).unwrap();
        let (_, dequeued_tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

        assert_eq!(dequeued_tx.payload.as_ref(), payload);
    }

    #[test]
    fn test_record_entry_with_complex_data() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();

        // Create entry with proposals, decisions, actions, effects
        let mut decision = Decision::new();
        let action_id = aura_core::ActionId::generate();
        decision.accept(action_id);
        decision.reject(0, "test rejection");

        let entry = RecordEntry::builder(1, tx)
            .context_hash([42u8; 32])
            .proposals(ProposalSet::new())
            .decision(decision)
            .build();

        store
            .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
            .unwrap();

        // Retrieve and verify
        let retrieved = store.get_record_entry(agent_id, 1).unwrap();
        assert_eq!(retrieved.context_hash, [42u8; 32]);
        assert_eq!(retrieved.decision.accepted_action_ids.len(), 1);
        assert_eq!(retrieved.decision.rejected.len(), 1);
        assert_eq!(retrieved.decision.rejected[0].reason, "test rejection");
    }

    // ========================================================================
    // Concurrency Tests (single-threaded simulation)
    // ========================================================================

    #[test]
    fn test_interleaved_agent_operations() {
        let (store, _dir) = create_test_store();

        let agents: Vec<AgentId> = (0..5).map(|_| AgentId::generate()).collect();

        // Interleave enqueue operations
        for round in 0..3 {
            for agent in &agents {
                let tx = create_test_tx(*agent);
                store.enqueue_tx(&tx).unwrap();
            }

            // Process one transaction per agent
            for agent in &agents {
                let (inbox_seq, tx) = store.dequeue_tx(*agent).unwrap().unwrap();
                let seq = round as u64 + 1; // Each agent at their own sequence
                let entry = RecordEntry::builder(seq, tx).build();
                store
                    .append_entry_atomic(*agent, seq, &entry, inbox_seq)
                    .unwrap();
            }
        }

        // Verify each agent processed 3 entries
        for agent in &agents {
            assert_eq!(store.get_head_seq(*agent).unwrap(), 3);
        }
    }

    #[test]
    fn test_reopen_store() {
        let dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();

        // First session: create and populate
        {
            let store = RocksStore::open(dir.path(), false).unwrap();

            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(1, tx).build();
            store
                .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
                .unwrap();

            store
                .set_agent_status(agent_id, AgentStatus::Paused)
                .unwrap();
        }

        // Second session: verify persistence
        {
            let store = RocksStore::open(dir.path(), false).unwrap();

            assert_eq!(store.get_head_seq(agent_id).unwrap(), 1);
            assert_eq!(
                store.get_agent_status(agent_id).unwrap(),
                AgentStatus::Paused
            );

            let entry = store.get_record_entry(agent_id, 1).unwrap();
            assert_eq!(entry.seq, 1);
        }
    }

    // ========================================================================
    // Concurrent Read/Write Tests
    // ========================================================================

    #[tokio::test]
    async fn test_concurrent_writes_different_agents() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());

        let mut handles = Vec::new();
        for _ in 0..10 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let agent_id = AgentId::generate();
                let tx = Transaction::new(
                    Hash::from_content(b"concurrent"),
                    agent_id,
                    1000,
                    TransactionType::UserPrompt,
                    Bytes::from_static(b"test payload"),
                );
                store.enqueue_tx(&tx).unwrap();
                let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
                let entry = RecordEntry::builder(1, tx).build();
                store
                    .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
                    .unwrap();
                agent_id
            }));
        }

        let agent_ids: Vec<AgentId> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        for agent_id in &agent_ids {
            assert_eq!(store.get_head_seq(*agent_id).unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn test_concurrent_reads_and_writes() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());
        let agent_id = AgentId::generate();

        // Pre-populate some entries
        for i in 1..=5 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        let mut handles = Vec::new();

        // Spawn readers
        for _ in 0..5 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let entries = store.scan_record(agent_id, 1, 10).unwrap();
                assert_eq!(entries.len(), 5);
                let head = store.get_head_seq(agent_id).unwrap();
                assert_eq!(head, 5);
            }));
        }

        // Spawn a writer for a different agent concurrently
        let store_w = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let other_agent = AgentId::generate();
            for i in 1..=3 {
                let tx = Transaction::new(
                    Hash::from_content(format!("other-{i}").as_bytes()),
                    other_agent,
                    1000,
                    TransactionType::UserPrompt,
                    Bytes::from(format!("payload-{i}")),
                );
                store_w.enqueue_tx(&tx).unwrap();
                let (inbox_seq, tx) = store_w.dequeue_tx(other_agent).unwrap().unwrap();
                let entry = RecordEntry::builder(i, tx).build();
                store_w
                    .append_entry_atomic(other_agent, i, &entry, inbox_seq)
                    .unwrap();
            }
        }));

        futures_util::future::join_all(handles)
            .await
            .into_iter()
            .for_each(|r| r.unwrap());
    }

    #[tokio::test]
    async fn test_concurrent_enqueue_same_agent() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksStore::open(dir.path(), false).unwrap());
        let agent_id = AgentId::generate();

        let mut handles = Vec::new();
        for i in 0..10u64 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let tx = Transaction::new(
                    Hash::from_content(format!("tx-{i}").as_bytes()),
                    agent_id,
                    1000 + i,
                    TransactionType::UserPrompt,
                    Bytes::from(format!("payload-{i}")),
                );
                store.enqueue_tx(&tx).unwrap();
            }));
        }

        futures_util::future::join_all(handles)
            .await
            .into_iter()
            .for_each(|r| r.unwrap());

        assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 10);
    }

    // ========================================================================
    // Crash-Recovery Simulation Tests
    // ========================================================================

    #[test]
    fn test_crash_recovery_inbox_persists() {
        let dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            for _ in 0..3 {
                let tx = create_test_tx(agent_id);
                store.enqueue_tx(&tx).unwrap();
            }
            assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 3);
        }

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            assert_eq!(store.get_inbox_depth(agent_id).unwrap(), 3);

            let (inbox_seq, _) = store.dequeue_tx(agent_id).unwrap().unwrap();
            assert_eq!(inbox_seq, 0);
        }
    }

    #[test]
    fn test_crash_recovery_record_entries_persist() {
        let dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            for i in 1..=5 {
                let tx = create_test_tx(agent_id);
                store.enqueue_tx(&tx).unwrap();
                let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
                let entry = RecordEntry::builder(i, tx)
                    .context_hash([i as u8; 32])
                    .build();
                store
                    .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                    .unwrap();
            }
        }

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            assert_eq!(store.get_head_seq(agent_id).unwrap(), 5);

            let entries = store.scan_record(agent_id, 1, 10).unwrap();
            assert_eq!(entries.len(), 5);
            for (i, entry) in entries.iter().enumerate() {
                assert_eq!(entry.seq, (i + 1) as u64);
                assert_eq!(entry.context_hash, [(i + 1) as u8; 32]);
            }
        }
    }

    #[test]
    fn test_crash_recovery_multiple_agents() {
        let dir = TempDir::new().unwrap();
        let agents: Vec<AgentId> = (0..3).map(|_| AgentId::generate()).collect();

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            for (idx, agent_id) in agents.iter().enumerate() {
                for i in 1..=((idx + 1) as u64) {
                    let tx = create_test_tx(*agent_id);
                    store.enqueue_tx(&tx).unwrap();
                    let (inbox_seq, tx) = store.dequeue_tx(*agent_id).unwrap().unwrap();
                    let entry = RecordEntry::builder(i, tx).build();
                    store
                        .append_entry_atomic(*agent_id, i, &entry, inbox_seq)
                        .unwrap();
                }
                store
                    .set_agent_status(*agent_id, AgentStatus::Paused)
                    .unwrap();
            }
        }

        {
            let store = RocksStore::open(dir.path(), false).unwrap();
            for (idx, agent_id) in agents.iter().enumerate() {
                let expected_seq = (idx + 1) as u64;
                assert_eq!(store.get_head_seq(*agent_id).unwrap(), expected_seq);
                assert_eq!(
                    store.get_agent_status(*agent_id).unwrap(),
                    AgentStatus::Paused
                );
            }
        }
    }

    // ========================================================================
    // Additional Scan Edge Case Tests
    // ========================================================================

    #[test]
    fn test_scan_single_entry() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        let tx = create_test_tx(agent_id);
        store.enqueue_tx(&tx).unwrap();
        let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
        let entry = RecordEntry::builder(1, tx).build();
        store
            .append_entry_atomic(agent_id, 1, &entry, inbox_seq)
            .unwrap();

        let entries = store.scan_record(agent_id, 1, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, 1);
    }

    #[test]
    fn test_scan_with_large_limit() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        for i in 1..=20 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        let entries = store.scan_record(agent_id, 1, 100_000).unwrap();
        assert_eq!(entries.len(), 20);
    }

    #[test]
    fn test_scan_from_seq_zero() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        for i in 1..=3 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        let entries = store.scan_record(agent_id, 0, 100).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_scan_from_nonexistent_seq() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        for i in 1..=3 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        let entries = store.scan_record(agent_id, 999, 100).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_scan_limit_one_returns_single_entry() {
        let (store, _dir) = create_test_store();
        let agent_id = AgentId::generate();

        for i in 1..=5 {
            let tx = create_test_tx(agent_id);
            store.enqueue_tx(&tx).unwrap();
            let (inbox_seq, tx) = store.dequeue_tx(agent_id).unwrap().unwrap();
            let entry = RecordEntry::builder(i, tx).build();
            store
                .append_entry_atomic(agent_id, i, &entry, inbox_seq)
                .unwrap();
        }

        let entries = store.scan_record(agent_id, 1, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, 1);
    }
}
