//! In-memory store fake and pipeline constructors shared by the topic files.

use std::sync::{Arc, Mutex};

use aura_context_memory::{
    AgentEvent, Fact, LlmRefiner, MemoryStats, MemoryStoreApi, MemoryWritePipeline, Procedure,
    RefinerConfig, WriteConfig,
};
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use aura_model::{MockProvider, MockResponse, ModelProvider};
use chrono::{DateTime, Utc};

/// Smallest possible in-memory `MemoryStoreApi` fake — keeps the
/// `aura-store-db` dep out of `aura-context-memory`'s test surface per
/// the Phase 6c brief.
#[derive(Default)]
pub(crate) struct FakeStore {
    pub(crate) facts: Mutex<Vec<Fact>>,
    events: Mutex<Vec<AgentEvent>>,
    procedures: Mutex<Vec<Procedure>>,
}

impl MemoryStoreApi for FakeStore {
    fn get_continuity_config(
        &self,
        _agent_id: AgentId,
    ) -> Result<aura_context_memory::AgentContinuityConfig, aura_context_memory::MemoryError> {
        Ok(aura_context_memory::AgentContinuityConfig::default())
    }

    fn put_continuity_config(
        &self,
        _agent_id: AgentId,
        _config: &aura_context_memory::AgentContinuityConfig,
    ) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }

    fn put_fact(&self, fact: &Fact) -> Result<(), aura_context_memory::MemoryError> {
        let mut facts = self.facts.lock().expect("facts lock");
        if let Some(existing) = facts.iter_mut().find(|f| f.fact_id == fact.fact_id) {
            *existing = fact.clone();
        } else {
            facts.push(fact.clone());
        }
        Ok(())
    }
    fn get_fact(
        &self,
        agent_id: AgentId,
        fact_id: FactId,
    ) -> Result<Fact, aura_context_memory::MemoryError> {
        self.facts
            .lock()
            .expect("facts lock")
            .iter()
            .find(|f| f.agent_id == agent_id && f.fact_id == fact_id)
            .cloned()
            .ok_or(aura_context_memory::MemoryError::FactNotFound {
                agent_id: agent_id.to_hex(),
                fact_id: fact_id.to_hex(),
            })
    }
    fn get_fact_by_key(
        &self,
        agent_id: AgentId,
        key: &str,
    ) -> Result<Option<Fact>, aura_context_memory::MemoryError> {
        Ok(self
            .facts
            .lock()
            .expect("facts lock")
            .iter()
            .find(|f| f.agent_id == agent_id && f.key == key)
            .cloned())
    }
    fn list_facts(&self, agent_id: AgentId) -> Result<Vec<Fact>, aura_context_memory::MemoryError> {
        Ok(self
            .facts
            .lock()
            .expect("facts lock")
            .iter()
            .filter(|f| f.agent_id == agent_id)
            .cloned()
            .collect())
    }
    fn touch_fact(
        &self,
        _agent_id: AgentId,
        _fact_id: FactId,
    ) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }
    fn delete_fact(
        &self,
        agent_id: AgentId,
        fact_id: FactId,
    ) -> Result<(), aura_context_memory::MemoryError> {
        self.facts
            .lock()
            .expect("facts lock")
            .retain(|f| !(f.agent_id == agent_id && f.fact_id == fact_id));
        Ok(())
    }

    fn put_event(&self, event: &AgentEvent) -> Result<(), aura_context_memory::MemoryError> {
        self.events.lock().expect("events lock").push(event.clone());
        Ok(())
    }
    fn list_events(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<AgentEvent>, aura_context_memory::MemoryError> {
        let events = self.events.lock().expect("events lock");
        Ok(events
            .iter()
            .filter(|e| e.agent_id == agent_id)
            .take(limit)
            .cloned()
            .collect())
    }
    fn list_events_since(
        &self,
        agent_id: AgentId,
        since: DateTime<Utc>,
    ) -> Result<Vec<AgentEvent>, aura_context_memory::MemoryError> {
        Ok(self
            .events
            .lock()
            .expect("events lock")
            .iter()
            .filter(|e| e.agent_id == agent_id && e.timestamp >= since)
            .cloned()
            .collect())
    }
    fn delete_event_direct(
        &self,
        _agent_id: AgentId,
        _timestamp: DateTime<Utc>,
        _event_id: AgentEventId,
    ) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }
    fn delete_event(
        &self,
        _agent_id: AgentId,
        _event_id: AgentEventId,
    ) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }
    fn delete_events_before(
        &self,
        _agent_id: AgentId,
        _before: DateTime<Utc>,
    ) -> Result<usize, aura_context_memory::MemoryError> {
        Ok(0)
    }

    fn put_procedure(&self, proc: &Procedure) -> Result<(), aura_context_memory::MemoryError> {
        self.procedures
            .lock()
            .expect("procs lock")
            .push(proc.clone());
        Ok(())
    }
    fn get_procedure(
        &self,
        agent_id: AgentId,
        procedure_id: ProcedureId,
    ) -> Result<Procedure, aura_context_memory::MemoryError> {
        self.procedures
            .lock()
            .expect("procs lock")
            .iter()
            .find(|p| p.agent_id == agent_id && p.procedure_id == procedure_id)
            .cloned()
            .ok_or(aura_context_memory::MemoryError::ProcedureNotFound {
                agent_id: agent_id.to_hex(),
                procedure_id: procedure_id.to_hex(),
            })
    }
    fn list_procedures(
        &self,
        agent_id: AgentId,
    ) -> Result<Vec<Procedure>, aura_context_memory::MemoryError> {
        Ok(self
            .procedures
            .lock()
            .expect("procs lock")
            .iter()
            .filter(|p| p.agent_id == agent_id)
            .cloned()
            .collect())
    }
    fn delete_procedure(
        &self,
        _agent_id: AgentId,
        _procedure_id: ProcedureId,
    ) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }
    fn delete_all(&self, _agent_id: AgentId) -> Result<(), aura_context_memory::MemoryError> {
        Ok(())
    }
    fn stats(&self, agent_id: AgentId) -> Result<MemoryStats, aura_context_memory::MemoryError> {
        Ok(MemoryStats {
            facts: self
                .facts
                .lock()
                .expect("facts lock")
                .iter()
                .filter(|f| f.agent_id == agent_id)
                .count(),
            events: self
                .events
                .lock()
                .expect("events lock")
                .iter()
                .filter(|e| e.agent_id == agent_id)
                .count(),
            procedures: self
                .procedures
                .lock()
                .expect("procs lock")
                .iter()
                .filter(|p| p.agent_id == agent_id)
                .count(),
        })
    }
}

pub(crate) fn pipeline_with_silent_llm() -> (Arc<FakeStore>, MemoryWritePipeline) {
    let store: Arc<FakeStore> = Arc::new(FakeStore::default());
    // Provider returns an empty body so the refiner short-circuits at
    // "no KEEP/DROP/FACT lines parsed" and keeps every heuristic
    // candidate as-is.
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::new().with_default_response(MockResponse::text("")));
    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let store_api: Arc<dyn MemoryStoreApi> = store.clone();
    let pipeline = MemoryWritePipeline::new(store_api, refiner, WriteConfig::default());
    (store, pipeline)
}

pub(crate) fn pipeline_with_llm_response(response: &str) -> (Arc<FakeStore>, MemoryWritePipeline) {
    let store: Arc<FakeStore> = Arc::new(FakeStore::default());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::new().with_default_response(MockResponse::text(response)));
    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let store_api: Arc<dyn MemoryStoreApi> = store.clone();
    let pipeline = MemoryWritePipeline::new(store_api, refiner, WriteConfig::default());
    (store, pipeline)
}

pub(crate) fn pipeline_with_llm_responses(
    responses: &[&str],
) -> (Arc<FakeStore>, MemoryWritePipeline) {
    let store: Arc<FakeStore> = Arc::new(FakeStore::default());
    let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::new(
        MockProvider::new().with_responses(
            responses
                .iter()
                .map(|response| MockResponse::text(*response))
                .collect(),
        ),
    );
    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let store_api: Arc<dyn MemoryStoreApi> = store.clone();
    let pipeline = MemoryWritePipeline::new(store_api, refiner, WriteConfig::default());
    (store, pipeline)
}
