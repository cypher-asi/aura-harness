//! Reads, inbox, agent metadata and processing claims: the
//! [`ReadStore`] impl and the helpers it shares with the append path.

use super::RocksStore;
use crate::cf;
use crate::error::{backend, StoreError};
use crate::keys::{AgentMetaKey, InboxKey, KeyCodec, RecordKey};
use crate::store::ReadStore;
use aura_core_types::AgentStatus;
use aura_core_types::{
    AgentId, ContextHash, RecordEntry, RuntimeCapabilityInstall, Transaction, UserToolDefaults,
};
use rocksdb::{Direction, IteratorMode, WriteBatch};
use tracing::{debug, instrument};

impl RocksStore {
    /// Up to `limit` of `agent_id`'s record entries from `from_seq`,
    /// decoded as `T`.
    fn scan_record_as<T: serde::de::DeserializeOwned>(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<T>, StoreError> {
        let cf = self.cf(cf::RECORD)?;

        let start_key = RecordKey::scan_from(agent_id, from_seq);
        let end_key = RecordKey::scan_end(agent_id);

        let iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(&start_key, Direction::Forward));

        let mut entries = Vec::with_capacity(limit);

        for item in iter {
            let (key, value) = item.map_err(backend)?;

            if key.as_ref() >= end_key.as_slice() {
                break;
            }

            let record_key = RecordKey::decode(&key)?;

            if record_key.agent_id != agent_id {
                break;
            }

            let value = self.open_value(&value)?;
            let entry = serde_json::from_slice::<T>(&value).map_err(|e| {
                StoreError::Deserialization(format!("record seq={}: {e}", record_key.seq))
            })?;
            entries.push(entry);

            if entries.len() >= limit {
                break;
            }
        }
        Ok(entries)
    }

    /// Read a u64 value from agent metadata.
    fn read_meta_u64(&self, key: &AgentMetaKey) -> Result<u64, StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let encoded_key = key.encode();

        match self.db.get_cf(&cf, &encoded_key).map_err(backend)? {
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

    pub(super) fn runtime_capability_key(agent_id: AgentId) -> [u8; 32] {
        *agent_id.as_bytes()
    }

    fn processing_claim_key(agent_id: AgentId) -> Vec<u8> {
        AgentMetaKey::processing_claim(agent_id).encode()
    }
}

// Read spans are at debug level: reads run once per agent row every time
// the agent lists are rebuilt, and an info span (enabled by the apps'
// default `aura=info` filter) formats its fields on every call.
impl ReadStore for RocksStore {
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
        let tx_bytes = self.seal_value(serde_json::to_vec(tx)?)?;

        // Write batch: inbox entry + update tail
        let mut batch = WriteBatch::default();
        batch.put_cf(&cf_inbox, inbox_key.encode(), tx_bytes);
        batch.put_cf(&cf_meta, tail_key.encode(), (tail + 1).to_be_bytes());

        self.db
            .write_opt(batch, &self.write_opts())
            .map_err(backend)?;

        debug!(inbox_seq = tail, "Transaction enqueued");
        Ok(())
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn dequeue_tx(
        &self,
        agent_id: AgentId,
    ) -> Result<Option<(crate::store::DequeueToken, Transaction)>, StoreError> {
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

        if let Some(bytes) = self.db.get_cf(&cf_inbox, &encoded_key).map_err(backend)? {
            let bytes = self.open_value(&bytes)?;
            let tx: Transaction = serde_json::from_slice(&bytes)
                .map_err(|e| StoreError::Deserialization(e.to_string()))?;
            debug!(inbox_seq = head, "Transaction dequeued");
            let token = crate::store::DequeueToken::from_inbox_seq(head);
            Ok(Some((token, tx)))
        } else {
            Err(StoreError::InboxCorruption {
                agent_id,
                expected_seq: head,
            })
        }
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id))]
    fn get_head_seq(&self, agent_id: AgentId) -> Result<u64, StoreError> {
        let key = AgentMetaKey::head_seq(agent_id);
        self.read_meta_u64(&key)
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id, from_seq, limit))]
    fn scan_record(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, StoreError> {
        let entries = self.scan_record_as::<RecordEntry>(agent_id, from_seq, limit)?;
        debug!(count = entries.len(), "Record scan complete");
        Ok(entries)
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id, from_seq, limit))]
    fn scan_context_hashes(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<(u64, ContextHash)>, StoreError> {
        /// What the hash window needs of an entry; serde skips the rest
        /// without building it.
        #[derive(serde::Deserialize)]
        struct Hashed {
            seq: u64,
            context_hash: ContextHash,
        }
        Ok(self
            .scan_record_as::<Hashed>(agent_id, from_seq, limit)?
            .into_iter()
            .map(|entry| (entry.seq, entry.context_hash))
            .collect())
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id, from_seq, limit))]
    fn scan_record_descending(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, StoreError> {
        if from_seq == 0 || limit == 0 {
            return Ok(Vec::new());
        }

        let cf = self.cf(cf::RECORD)?;
        let start_key = RecordKey::scan_from(agent_id, from_seq);
        let agent_start_key = RecordKey::scan_from(agent_id, 1);

        let iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(&start_key, Direction::Reverse));
        let mut entries = Vec::with_capacity(limit);

        for item in iter {
            let (key, value) = item.map_err(backend)?;

            if key.as_ref() < agent_start_key.as_slice() {
                break;
            }

            let record_key = RecordKey::decode(&key)?;
            if record_key.agent_id != agent_id {
                break;
            }

            let value = self.open_value(&value)?;
            let entry = serde_json::from_slice::<RecordEntry>(&value).map_err(|e| {
                StoreError::Deserialization(format!("record seq={}: {e}", record_key.seq))
            })?;
            entries.push(entry);

            if entries.len() >= limit {
                break;
            }
        }

        debug!(count = entries.len(), "Descending record scan complete");
        Ok(entries)
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id, seq))]
    fn get_record_entry(&self, agent_id: AgentId, seq: u64) -> Result<RecordEntry, StoreError> {
        let cf = self.cf(cf::RECORD)?;
        let key = RecordKey::new(agent_id, seq);

        match self.db.get_cf(&cf, key.encode()).map_err(backend)? {
            Some(bytes) => {
                let bytes = self.open_value(&bytes)?;
                let entry: RecordEntry = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Deserialization(e.to_string()))?;
                Ok(entry)
            }
            None => Err(StoreError::RecordEntryNotFound(agent_id, seq)),
        }
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id))]
    fn get_agent_status(&self, agent_id: AgentId) -> Result<AgentStatus, StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let key = AgentMetaKey::status(agent_id);

        match self.db.get_cf(&cf, key.encode()).map_err(backend)? {
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

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn get_runtime_capabilities(
        &self,
        agent_id: AgentId,
    ) -> Result<Option<RuntimeCapabilityInstall>, StoreError> {
        let cf = self.cf(cf::RUNTIME_CAPABILITIES)?;
        let key = Self::runtime_capability_key(agent_id);

        match self.db.get_cf(&cf, key).map_err(backend)? {
            Some(bytes) => {
                let bytes = self.open_value(&bytes)?;
                let capability_state = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Deserialization(e.to_string()))?;
                Ok(Some(capability_state))
            }
            None => Ok(None),
        }
    }

    #[instrument(skip(self), fields(agent_id = %agent_id, ?status))]
    fn set_agent_status(&self, agent_id: AgentId, status: AgentStatus) -> Result<(), StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let key = AgentMetaKey::status(agent_id);

        self.db
            .put_cf_opt(&cf, key.encode(), [status.as_byte()], &self.write_opts())
            .map_err(backend)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id))]
    fn has_pending_tx(&self, agent_id: AgentId) -> Result<bool, StoreError> {
        let head = self.read_meta_u64(&AgentMetaKey::inbox_head(agent_id))?;
        let tail = self.read_meta_u64(&AgentMetaKey::inbox_tail(agent_id))?;
        Ok(tail > head)
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id))]
    fn get_inbox_depth(&self, agent_id: AgentId) -> Result<u64, StoreError> {
        let head = self.read_meta_u64(&AgentMetaKey::inbox_head(agent_id))?;
        let tail = self.read_meta_u64(&AgentMetaKey::inbox_tail(agent_id))?;
        Ok(tail.saturating_sub(head))
    }

    #[instrument(skip(self), fields(user_id = %user_id))]
    fn get_user_tool_defaults(
        &self,
        user_id: &str,
    ) -> Result<Option<UserToolDefaults>, StoreError> {
        let cf = self.cf(cf::USER_TOOL_DEFAULTS)?;
        match self.db.get_cf(&cf, user_id.as_bytes()).map_err(backend)? {
            Some(bytes) => {
                let bytes = self.open_value(&bytes)?;
                let defaults = serde_json::from_slice::<UserToolDefaults>(&bytes)
                    .map_err(|e| StoreError::Deserialization(e.to_string()))?;
                Ok(Some(defaults))
            }
            None => Ok(None),
        }
    }

    #[instrument(skip(self, defaults), fields(user_id = %user_id))]
    fn put_user_tool_defaults(
        &self,
        user_id: &str,
        defaults: &UserToolDefaults,
    ) -> Result<(), StoreError> {
        let cf = self.cf(cf::USER_TOOL_DEFAULTS)?;
        let bytes = self.seal_value(serde_json::to_vec(defaults)?)?;
        self.db
            .put_cf_opt(&cf, user_id.as_bytes(), bytes, &self.write_opts())
            .map_err(backend)?;
        Ok(())
    }

    #[instrument(skip(self), fields(user_id = %user_id))]
    fn delete_user_tool_defaults(&self, user_id: &str) -> Result<(), StoreError> {
        let cf = self.cf(cf::USER_TOOL_DEFAULTS)?;
        self.db
            .delete_cf_opt(&cf, user_id.as_bytes(), &self.write_opts())
            .map_err(backend)?;
        Ok(())
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn try_claim_agent_processing(&self, agent_id: AgentId) -> Result<bool, StoreError> {
        let _guard = self
            .processing_claim_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cf = self.cf(cf::AGENT_META)?;
        let key = Self::processing_claim_key(agent_id);

        if self.db.get_cf(&cf, &key).map_err(backend)?.is_some() {
            return Ok(false);
        }

        self.db
            .put_cf_opt(&cf, key, [1_u8], &self.write_opts())
            .map_err(backend)?;
        Ok(true)
    }

    #[instrument(skip(self), fields(agent_id = %agent_id))]
    fn release_agent_processing(&self, agent_id: AgentId) -> Result<(), StoreError> {
        let _guard = self
            .processing_claim_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cf = self.cf(cf::AGENT_META)?;
        let key = Self::processing_claim_key(agent_id);
        self.db
            .delete_cf_opt(&cf, key, &self.write_opts())
            .map_err(backend)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(agent_id = %agent_id))]
    fn is_agent_processing(&self, agent_id: AgentId) -> Result<bool, StoreError> {
        let cf = self.cf(cf::AGENT_META)?;
        let key = Self::processing_claim_key(agent_id);
        Ok(self.db.get_cf(&cf, key).map_err(backend)?.is_some())
    }
}
