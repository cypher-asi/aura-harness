//! Deterministic retrieval of memory for system prompt injection.

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::MemoryPacket;
use aura_core::AgentId;
use std::sync::Arc;

pub struct MemoryRetriever {
    store: Arc<MemoryStore>,
    config: RetrievalConfig,
}

#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub max_facts: usize,
    pub max_events: usize,
    pub max_procedures: usize,
    pub min_confidence: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            max_facts: 20,
            max_events: 10,
            max_procedures: 5,
            min_confidence: 0.3,
        }
    }
}

impl MemoryRetriever {
    #[must_use]
    pub const fn new(store: Arc<MemoryStore>, config: RetrievalConfig) -> Self {
        Self { store, config }
    }

    /// Retrieve a `MemoryPacket` for injection into the agent's system prompt.
    ///
    /// Facts are sorted by importance (descending), events by recency,
    /// and procedures by success rate.
    ///
    /// # Errors
    /// Returns error on store read failure.
    pub fn retrieve(&self, agent_id: AgentId) -> Result<MemoryPacket, MemoryError> {
        let mut facts = self.store.list_facts(agent_id)?;
        facts.retain(|f| f.confidence >= self.config.min_confidence);
        facts.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        facts.truncate(self.config.max_facts);

        let events = self.store.list_events(agent_id, self.config.max_events)?;

        let mut procedures = self.store.list_procedures(agent_id)?;
        procedures.sort_by(|a, b| {
            b.success_rate
                .partial_cmp(&a.success_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        procedures.truncate(self.config.max_procedures);

        Ok(MemoryPacket {
            facts,
            events,
            procedures,
        })
    }
}
