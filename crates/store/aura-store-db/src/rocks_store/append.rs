//! Record appends: the pre-flight sequence check, the per-agent append
//! locks, the batches each append path commits, and the [`WriteStore`]
//! impl.

use super::{RocksStore, APPEND_SHARDS};
use crate::cf;
use crate::error::{backend, StoreError};
use crate::keys::{AgentMetaKey, InboxKey, KeyCodec, RecordKey};
use crate::store::{ReadStore, WriteStore};
use aura_core_types::{AgentId, RecordEntry, RuntimeCapabilityInstall};
use rocksdb::WriteBatch;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{MutexGuard, PoisonError};
use tracing::{debug, instrument};

impl RocksStore {
    /// Assert that `next_seq` is exactly `current_head + 1` for
    /// `agent_id`, returning [`StoreError::SequenceMismatch`] if not.
    ///
    /// Extracted so both `append_entry_direct_internal` and
    /// `append_entry_atomic_internal` use the same pre-flight check.
    fn assert_next_seq(&self, agent_id: AgentId, next_seq: u64) -> Result<(), StoreError> {
        let current_head = self.get_head_seq(agent_id)?;
        if next_seq != current_head + 1 {
            return Err(StoreError::SequenceMismatch {
                expected: current_head + 1,
                actual: next_seq,
            });
        }
        Ok(())
    }

    /// Add the serialized record-entry write and the head-sequence bump
    /// to `batch` for `(agent_id, next_seq)`.
    ///
    /// Extracted so both append paths produce bit-identical byte
    /// streams for the record + head-seq pair.
    fn append_record_and_head_to_batch(
        &self,
        batch: &mut WriteBatch,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
    ) -> Result<(), StoreError> {
        let cf_record = self.cf(cf::RECORD)?;
        let cf_meta = self.cf(cf::AGENT_META)?;

        let entry_bytes = self.seal_value(serde_json::to_vec(entry)?)?;
        let record_key = RecordKey::new(agent_id, next_seq);
        let head_seq_key = AgentMetaKey::head_seq(agent_id);

        batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
        batch.put_cf(&cf_meta, head_seq_key.encode(), next_seq.to_be_bytes());
        Ok(())
    }

    /// Apply runtime-capability ledger mutations onto `batch`.
    ///
    /// When `clear_runtime_capabilities` is `true`, an explicit delete
    /// is queued. When `runtime_capabilities` is `Some`, the fresh
    /// payload is serialized and `put`. Ordering matches the original
    /// inline code: `clear` is added before `put`, so a simultaneous
    /// clear + set reduces to a single effective `put` (the new value
    /// wins within the batch — semantics preserved from the original
    /// `append_entry_*_internal` bodies).
    fn apply_runtime_capability_ops_to_batch(
        &self,
        batch: &mut WriteBatch,
        agent_id: AgentId,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError> {
        let cf_runtime_capabilities = self.cf(cf::RUNTIME_CAPABILITIES)?;
        let capability_key = Self::runtime_capability_key(agent_id);

        if clear_runtime_capabilities {
            batch.delete_cf(&cf_runtime_capabilities, capability_key);
        }

        if let Some(runtime_capabilities) = runtime_capabilities {
            let capability_bytes = self.seal_value(serde_json::to_vec(runtime_capabilities)?)?;
            batch.put_cf(
                &cf_runtime_capabilities,
                Self::runtime_capability_key(agent_id),
                capability_bytes,
            );
        }
        Ok(())
    }

    /// The lock `agent_id`'s appends hold across the head check and the
    /// commit. The batch is serialized and sealed before it is taken.
    fn append_lock(&self, agent_id: AgentId) -> MutexGuard<'_, ()> {
        let mut hasher = DefaultHasher::new();
        agent_id.as_bytes().hash(&mut hasher);
        let shard = usize::try_from(hasher.finish() % APPEND_SHARDS).unwrap_or(0);
        self.append_locks[shard]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn append_entry_direct_internal(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError> {
        let mut batch = WriteBatch::default();
        self.append_record_and_head_to_batch(&mut batch, agent_id, next_seq, entry)?;
        self.apply_runtime_capability_ops_to_batch(
            &mut batch,
            agent_id,
            runtime_capabilities,
            clear_runtime_capabilities,
        )?;

        let _append_guard = self.append_lock(agent_id);
        self.assert_next_seq(agent_id, next_seq)?;
        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(backend)?;
        debug!("Record entry committed (direct)");
        Ok(())
    }

    pub(super) fn append_entry_atomic_internal(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        dequeued_inbox_seq: u64,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError> {
        let cf_meta = self.cf(cf::AGENT_META)?;
        let cf_inbox = self.cf(cf::INBOX)?;

        let inbox_key = InboxKey::new(agent_id, dequeued_inbox_seq);
        let inbox_head_key = AgentMetaKey::inbox_head(agent_id);

        let mut batch = WriteBatch::default();
        self.append_record_and_head_to_batch(&mut batch, agent_id, next_seq, entry)?;
        // Inbox dequeue is the only operation that differs between the
        // direct and atomic append paths.
        batch.delete_cf(&cf_inbox, inbox_key.encode());
        batch.put_cf(
            &cf_meta,
            inbox_head_key.encode(),
            (dequeued_inbox_seq + 1).to_be_bytes(),
        );
        self.apply_runtime_capability_ops_to_batch(
            &mut batch,
            agent_id,
            runtime_capabilities,
            clear_runtime_capabilities,
        )?;

        let _append_guard = self.append_lock(agent_id);
        self.assert_next_seq(agent_id, next_seq)?;
        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(backend)?;

        debug!("Record entry committed atomically");
        Ok(())
    }
}

impl WriteStore for RocksStore {
    #[instrument(skip(self, entry), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_atomic(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        dequeued_inbox_seq: u64,
    ) -> Result<(), StoreError> {
        self.append_entry_atomic_internal(
            agent_id,
            next_seq,
            entry,
            dequeued_inbox_seq,
            None,
            false,
        )
    }

    #[instrument(skip(self, entry, runtime_capabilities), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_dequeued_with_runtime_capabilities(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        token: crate::store::DequeueToken,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError> {
        self.append_entry_atomic_internal(
            agent_id,
            next_seq,
            entry,
            token.inbox_seq(),
            runtime_capabilities,
            clear_runtime_capabilities,
        )
    }

    #[instrument(skip(self, entry), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_direct(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
    ) -> Result<(), StoreError> {
        self.append_entry_direct_internal(agent_id, next_seq, entry, None, false)
    }

    #[instrument(skip(self, entry, runtime_capabilities), fields(agent_id = %agent_id, seq = next_seq))]
    fn append_entry_direct_with_runtime_capabilities(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError> {
        self.append_entry_direct_internal(
            agent_id,
            next_seq,
            entry,
            runtime_capabilities,
            clear_runtime_capabilities,
        )
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

        let mut batch = WriteBatch::default();
        let head_seq_key = AgentMetaKey::head_seq(agent_id);

        for (i, entry) in entries.iter().enumerate() {
            let seq = base_seq + i as u64;
            let entry_bytes = self.seal_value(serde_json::to_vec(entry)?)?;
            let record_key = RecordKey::new(agent_id, seq);
            batch.put_cf(&cf_record, record_key.encode(), entry_bytes);
        }

        let last_seq = base_seq + entries.len() as u64 - 1;
        batch.put_cf(&cf_meta, head_seq_key.encode(), last_seq.to_be_bytes());

        let _append_guard = self.append_lock(agent_id);
        let current_head = self.get_head_seq(agent_id)?;
        if base_seq != current_head + 1 {
            return Err(StoreError::SequenceMismatch {
                expected: current_head + 1,
                actual: base_seq,
            });
        }
        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(backend)?;

        debug!(last_seq, "Batch record entries committed");
        Ok(())
    }
}
