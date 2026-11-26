//! The storage trait the rest of the crate programs against.

use super::MemoryStats;
use crate::error::MemoryError;
use crate::types::{AgentContinuityConfig, AgentEvent, Fact, Procedure};
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::{DateTime, Utc};

/// Abstraction over the memory store for testability.
///
/// All operations are blocking — callers on async runtimes should wrap
/// calls in `tokio::task::spawn_blocking`.
pub trait MemoryStoreApi: Send + Sync {
    fn get_continuity_config(
        &self,
        agent_id: AgentId,
    ) -> Result<AgentContinuityConfig, MemoryError>;
    fn put_continuity_config(
        &self,
        agent_id: AgentId,
        config: &AgentContinuityConfig,
    ) -> Result<(), MemoryError>;

    fn put_fact(&self, fact: &Fact) -> Result<(), MemoryError>;
    fn get_fact(&self, agent_id: AgentId, fact_id: FactId) -> Result<Fact, MemoryError>;
    fn get_fact_by_key(&self, agent_id: AgentId, key: &str) -> Result<Option<Fact>, MemoryError>;
    fn list_facts(&self, agent_id: AgentId) -> Result<Vec<Fact>, MemoryError>;
    fn touch_fact(&self, agent_id: AgentId, fact_id: FactId) -> Result<(), MemoryError>;
    fn delete_fact(&self, agent_id: AgentId, fact_id: FactId) -> Result<(), MemoryError>;

    fn put_event(&self, event: &AgentEvent) -> Result<(), MemoryError>;
    fn list_events(&self, agent_id: AgentId, limit: usize) -> Result<Vec<AgentEvent>, MemoryError>;
    fn list_events_since(
        &self,
        agent_id: AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<AgentEvent>, MemoryError>;
    fn delete_event_direct(
        &self,
        agent_id: AgentId,
        timestamp: DateTime<Utc>,
        event_id: AgentEventId,
    ) -> Result<(), MemoryError>;
    fn delete_event(&self, agent_id: AgentId, event_id: AgentEventId) -> Result<(), MemoryError>;
    fn delete_events_before(
        &self,
        agent_id: AgentId,
        before: DateTime<Utc>,
    ) -> Result<usize, MemoryError>;

    fn put_procedure(&self, proc: &Procedure) -> Result<(), MemoryError>;
    fn get_procedure(
        &self,
        agent_id: AgentId,
        procedure_id: ProcedureId,
    ) -> Result<Procedure, MemoryError>;
    fn list_procedures(&self, agent_id: AgentId) -> Result<Vec<Procedure>, MemoryError>;
    fn delete_procedure(
        &self,
        agent_id: AgentId,
        procedure_id: ProcedureId,
    ) -> Result<(), MemoryError>;

    fn delete_all(&self, agent_id: AgentId) -> Result<(), MemoryError>;
    fn stats(&self, agent_id: AgentId) -> Result<MemoryStats, MemoryError>;
}
