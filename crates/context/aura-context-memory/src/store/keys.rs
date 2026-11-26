//! Key encoding and agent-prefix range scans (see the key table in the
//! module docs).

use super::MemoryStore;
use crate::error::MemoryError;
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::{DateTime, Utc};
use rocksdb::{DBWithThreadMode, IteratorMode, MultiThreaded, WriteBatch};
use std::sync::Arc;

impl MemoryStore {
    pub(super) fn fact_key(agent_id: AgentId, fact_id: FactId) -> Vec<u8> {
        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(fact_id.as_bytes());
        key
    }

    pub(super) fn event_key(
        agent_id: AgentId,
        timestamp: DateTime<Utc>,
        event_id: AgentEventId,
    ) -> Vec<u8> {
        let mut key = Vec::with_capacity(56);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(&timestamp.timestamp_millis().to_be_bytes());
        key.extend_from_slice(event_id.as_bytes());
        key
    }

    pub(super) fn procedure_key(agent_id: AgentId, procedure_id: ProcedureId) -> Vec<u8> {
        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(procedure_id.as_bytes());
        key
    }

    pub(super) fn event_index_key(agent_id: AgentId, event_id: AgentEventId) -> Vec<u8> {
        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(event_id.as_bytes());
        key
    }

    pub(super) fn agent_prefix(agent_id: AgentId) -> Vec<u8> {
        agent_id.as_bytes().to_vec()
    }

    /// Compute the exclusive upper-bound key for prefix iteration.
    ///
    /// Increments the last non-0xFF byte. When all bytes are 0xFF, appends a
    /// zero byte to form a key that is lexicographically greater than any
    /// valid agent prefix.
    pub(crate) fn agent_prefix_end(agent_id: AgentId) -> Vec<u8> {
        let mut end = agent_id.as_bytes().to_vec();
        for byte in end.iter_mut().rev() {
            if *byte < 0xFF {
                *byte += 1;
                return end;
            }
            *byte = 0;
        }
        end.push(0);
        end
    }

    pub(super) fn batch_delete_range(
        db: &DBWithThreadMode<MultiThreaded>,
        cf: &Arc<rocksdb::BoundColumnFamily<'_>>,
        prefix: &[u8],
        end: &[u8],
        batch: &mut WriteBatch,
    ) -> Result<(), MemoryError> {
        let iter = db.iterator_cf(cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        for item in iter {
            let (k, _) = item?;
            if k.as_ref() >= end {
                break;
            }
            batch.delete_cf(cf, &k);
        }
        Ok(())
    }

    pub(super) fn count_for_agent(
        &self,
        cf_name: &str,
        agent_id: AgentId,
    ) -> Result<usize, MemoryError> {
        let cf = self.cf_handle(cf_name)?;
        let prefix = Self::agent_prefix(agent_id);
        let end = Self::agent_prefix_end(agent_id);
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        );

        let mut count = 0usize;
        for item in iter {
            let (k, _) = item?;
            if k.as_ref() >= end.as_slice() {
                break;
            }
            count += 1;
        }
        Ok(count)
    }
}
