//! RocksDB-backed memory store.

use crate::error::MemoryError;
use crate::types::{AgentEvent, Fact, Procedure};
use aura_core::{AgentEventId, AgentId, FactId, ProcedureId};
use aura_store::cf;
use chrono::{DateTime, Utc};
use rocksdb::{DBWithThreadMode, IteratorMode, MultiThreaded};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct MemoryStore {
    db: Arc<DBWithThreadMode<MultiThreaded>>,
}

impl MemoryStore {
    #[must_use]
    pub const fn new(db: Arc<DBWithThreadMode<MultiThreaded>>) -> Self {
        Self { db }
    }

    fn cf_handle(
        &self,
        name: &str,
    ) -> Result<Arc<rocksdb::BoundColumnFamily<'_>>, MemoryError> {
        self.db
            .cf_handle(name)
            .ok_or_else(|| MemoryError::ColumnFamilyNotFound(name.to_string()))
    }

    // === Key encoding ===

    fn fact_key(agent_id: AgentId, fact_id: FactId) -> Vec<u8> {
        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(fact_id.as_bytes());
        key
    }

    fn event_key(agent_id: AgentId, timestamp: DateTime<Utc>, event_id: AgentEventId) -> Vec<u8> {
        let mut key = Vec::with_capacity(56);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(&timestamp.timestamp_millis().to_be_bytes());
        key.extend_from_slice(event_id.as_bytes());
        key
    }

    fn procedure_key(agent_id: AgentId, procedure_id: ProcedureId) -> Vec<u8> {
        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(agent_id.as_bytes());
        key.extend_from_slice(procedure_id.as_bytes());
        key
    }

    fn agent_prefix(agent_id: AgentId) -> Vec<u8> {
        agent_id.as_bytes().to_vec()
    }

    fn agent_prefix_end(agent_id: AgentId) -> Vec<u8> {
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

    // === Facts ===

    /// Store or overwrite a fact.
    ///
    /// # Errors
    /// Returns error on CF lookup or serialization/write failure.
    pub fn put_fact(&self, fact: &Fact) -> Result<(), MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_FACTS)?;
        let key = Self::fact_key(fact.agent_id, fact.fact_id);
        let value = serde_json::to_vec(fact)?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    /// Get a specific fact by agent and fact ID.
    ///
    /// # Errors
    /// Returns `FactNotFound` if missing, or on deserialization failure.
    pub fn get_fact(&self, agent_id: AgentId, fact_id: FactId) -> Result<Fact, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_FACTS)?;
        let key = Self::fact_key(agent_id, fact_id);
        match self.db.get_cf(&cf, key)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| MemoryError::Deserialization(e.to_string())),
            None => Err(MemoryError::FactNotFound {
                agent_id: agent_id.to_hex(),
                fact_id: fact_id.to_hex(),
            }),
        }
    }

    /// Find a fact by its semantic key within an agent's fact store.
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn get_fact_by_key(
        &self,
        agent_id: AgentId,
        key: &str,
    ) -> Result<Option<Fact>, MemoryError> {
        let facts = self.list_facts(agent_id)?;
        Ok(facts.into_iter().find(|f| f.key == key))
    }

    /// List all facts for an agent.
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn list_facts(&self, agent_id: AgentId) -> Result<Vec<Fact>, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_FACTS)?;
        let prefix = Self::agent_prefix(agent_id);
        let end = Self::agent_prefix_end(agent_id);
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        );

        let mut facts = Vec::new();
        for item in iter {
            let (k, v) = item?;
            if k.as_ref() >= end.as_slice() {
                break;
            }
            let fact: Fact = serde_json::from_slice(&v)
                .map_err(|e| MemoryError::Deserialization(e.to_string()))?;
            facts.push(fact);
        }
        Ok(facts)
    }

    /// Delete a specific fact.
    ///
    /// # Errors
    /// Returns error on CF lookup or write failure.
    pub fn delete_fact(&self, agent_id: AgentId, fact_id: FactId) -> Result<(), MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_FACTS)?;
        let key = Self::fact_key(agent_id, fact_id);
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    // === Events ===

    /// Store an episodic event.
    ///
    /// # Errors
    /// Returns error on CF lookup or serialization/write failure.
    pub fn put_event(&self, event: &AgentEvent) -> Result<(), MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
        let key = Self::event_key(event.agent_id, event.timestamp, event.event_id);
        let value = serde_json::to_vec(event)?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    /// List most recent events for an agent (reverse chronological).
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn list_events(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<AgentEvent>, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
        let end = Self::agent_prefix_end(agent_id);
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&end, rocksdb::Direction::Reverse),
        );
        let prefix = Self::agent_prefix(agent_id);

        let mut events = Vec::new();
        for item in iter {
            let (k, v) = item?;
            if k.len() < prefix.len() || k[..prefix.len()] != *prefix.as_slice() {
                break;
            }
            let event: AgentEvent = serde_json::from_slice(&v)
                .map_err(|e| MemoryError::Deserialization(e.to_string()))?;
            events.push(event);
            if events.len() >= limit {
                break;
            }
        }
        Ok(events)
    }

    /// List events since a given timestamp (forward chronological).
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn list_events_since(
        &self,
        agent_id: AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<AgentEvent>, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
        let start = {
            let mut k = Vec::with_capacity(40);
            k.extend_from_slice(agent_id.as_bytes());
            k.extend_from_slice(&since.timestamp_millis().to_be_bytes());
            k
        };
        let end = Self::agent_prefix_end(agent_id);
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&start, rocksdb::Direction::Forward),
        );

        let mut events = Vec::new();
        for item in iter {
            let (k, v) = item?;
            if k.as_ref() >= end.as_slice() {
                break;
            }
            let event: AgentEvent = serde_json::from_slice(&v)
                .map_err(|e| MemoryError::Deserialization(e.to_string()))?;
            events.push(event);
        }
        Ok(events)
    }

    /// Delete a specific event by scanning to find its timestamp-based key.
    ///
    /// # Errors
    /// Returns `EventNotFound` if missing, or on write failure.
    pub fn delete_event(
        &self,
        agent_id: AgentId,
        event_id: AgentEventId,
    ) -> Result<(), MemoryError> {
        let events = self.list_events(agent_id, 100_000)?;
        if let Some(event) = events.iter().find(|e| e.event_id == event_id) {
            let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
            let key = Self::event_key(agent_id, event.timestamp, event_id);
            self.db.delete_cf(&cf, key)?;
            Ok(())
        } else {
            Err(MemoryError::EventNotFound {
                agent_id: agent_id.to_hex(),
                event_id: event_id.to_hex(),
            })
        }
    }

    /// Delete all events before a given timestamp.
    ///
    /// # Errors
    /// Returns error on CF lookup or write failure.
    pub fn delete_events_before(
        &self,
        agent_id: AgentId,
        before: DateTime<Utc>,
    ) -> Result<usize, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
        let prefix = Self::agent_prefix(agent_id);
        let cutoff = {
            let mut k = Vec::with_capacity(40);
            k.extend_from_slice(agent_id.as_bytes());
            k.extend_from_slice(&before.timestamp_millis().to_be_bytes());
            k
        };
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        );

        let mut keys_to_delete = Vec::new();
        for item in iter {
            let (k, _) = item?;
            if k.as_ref() >= cutoff.as_slice() {
                break;
            }
            if k.len() < prefix.len() || k[..prefix.len()] != *prefix.as_slice() {
                break;
            }
            keys_to_delete.push(k.to_vec());
        }

        let deleted = keys_to_delete.len();
        for key in &keys_to_delete {
            self.db.delete_cf(&cf, key)?;
        }
        Ok(deleted)
    }

    // === Procedures ===

    /// Store or overwrite a procedure.
    ///
    /// # Errors
    /// Returns error on CF lookup or serialization/write failure.
    pub fn put_procedure(&self, proc: &Procedure) -> Result<(), MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_PROCEDURES)?;
        let key = Self::procedure_key(proc.agent_id, proc.procedure_id);
        let value = serde_json::to_vec(proc)?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    /// Get a specific procedure by agent and procedure ID.
    ///
    /// # Errors
    /// Returns `ProcedureNotFound` if missing, or on deserialization failure.
    pub fn get_procedure(
        &self,
        agent_id: AgentId,
        procedure_id: ProcedureId,
    ) -> Result<Procedure, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_PROCEDURES)?;
        let key = Self::procedure_key(agent_id, procedure_id);
        match self.db.get_cf(&cf, key)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| MemoryError::Deserialization(e.to_string())),
            None => Err(MemoryError::ProcedureNotFound {
                agent_id: agent_id.to_hex(),
                procedure_id: procedure_id.to_hex(),
            }),
        }
    }

    /// List all procedures for an agent.
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn list_procedures(&self, agent_id: AgentId) -> Result<Vec<Procedure>, MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_PROCEDURES)?;
        let prefix = Self::agent_prefix(agent_id);
        let end = Self::agent_prefix_end(agent_id);
        let iter = self.db.iterator_cf(
            &cf,
            IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        );

        let mut procs = Vec::new();
        for item in iter {
            let (k, v) = item?;
            if k.as_ref() >= end.as_slice() {
                break;
            }
            let proc: Procedure = serde_json::from_slice(&v)
                .map_err(|e| MemoryError::Deserialization(e.to_string()))?;
            procs.push(proc);
        }
        Ok(procs)
    }

    /// Delete a specific procedure.
    ///
    /// # Errors
    /// Returns error on CF lookup or write failure.
    pub fn delete_procedure(
        &self,
        agent_id: AgentId,
        procedure_id: ProcedureId,
    ) -> Result<(), MemoryError> {
        let cf = self.cf_handle(cf::MEMORY_PROCEDURES)?;
        let key = Self::procedure_key(agent_id, procedure_id);
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    // === Aggregate ===

    /// Delete all memory (facts, events, procedures) for an agent.
    ///
    /// # Errors
    /// Returns error on CF lookup or write failure.
    pub fn delete_all(&self, agent_id: AgentId) -> Result<(), MemoryError> {
        let facts = self.list_facts(agent_id)?;
        for fact in &facts {
            self.delete_fact(agent_id, fact.fact_id)?;
        }
        let events = self.list_events(agent_id, 100_000)?;
        for event in &events {
            let cf = self.cf_handle(cf::MEMORY_EVENTS)?;
            let key = Self::event_key(agent_id, event.timestamp, event.event_id);
            self.db.delete_cf(&cf, key)?;
        }
        let procs = self.list_procedures(agent_id)?;
        for proc in &procs {
            self.delete_procedure(agent_id, proc.procedure_id)?;
        }
        Ok(())
    }

    /// Get aggregate stats for an agent's memory.
    ///
    /// # Errors
    /// Returns error on CF lookup or deserialization failure.
    pub fn stats(&self, agent_id: AgentId) -> Result<MemoryStats, MemoryError> {
        let facts = self.list_facts(agent_id)?;
        let events = self.list_events(agent_id, 100_000)?;
        let procs = self.list_procedures(agent_id)?;

        Ok(MemoryStats {
            facts: facts.len(),
            events: events.len(),
            procedures: procs.len(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub facts: usize,
    pub events: usize,
    pub procedures: usize,
}
