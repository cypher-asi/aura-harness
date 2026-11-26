//! Per-partition capacity limits applied after a write.

use super::MemoryWritePipeline;
use crate::error::MemoryError;
use aura_core_types::AgentId;

impl MemoryWritePipeline {
    pub(super) fn enforce_capacity(&self, agent_id: AgentId) -> Result<(), MemoryError> {
        let mut facts = self.store.list_facts(agent_id)?;
        if facts.len() > self.config.max_facts_per_agent {
            facts.sort_by(|a, b| {
                a.importance
                    .partial_cmp(&b.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let to_remove = facts.len() - self.config.max_facts_per_agent;
            for fact in facts.iter().take(to_remove) {
                self.store.delete_fact(agent_id, fact.fact_id)?;
            }
        }

        let overflow_buffer = 100;
        let events = self
            .store
            .list_events(agent_id, self.config.max_events_per_agent + overflow_buffer)?;
        if events.len() > self.config.max_events_per_agent {
            for event in events.iter().skip(self.config.max_events_per_agent) {
                self.store.delete_event(agent_id, event.event_id)?;
            }
        }

        let mut procs = self.store.list_procedures(agent_id)?;
        #[allow(clippy::cast_precision_loss)]
        if procs.len() > self.config.max_procedures_per_agent {
            procs.sort_by(|a, b| {
                let score_a = a.success_rate * a.execution_count as f32;
                let score_b = b.success_rate * b.execution_count as f32;
                score_a
                    .partial_cmp(&score_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let to_remove = procs.len() - self.config.max_procedures_per_agent;
            for proc in procs.iter().take(to_remove) {
                self.store.delete_procedure(agent_id, proc.procedure_id)?;
            }
        }

        Ok(())
    }
}
