//! Phase 6c regression test — ensure a [`TurnSummary`] roundtrips through
//! the memory write pipeline with the same candidate count + write-report
//! shape that `AgentLoopResult` produced before the inversion.
//!
//! The test uses an in-memory `MemoryStoreApi` fake and a `MockProvider`
//! that returns an empty string (so the heuristic stage produces all the
//! candidates and the LLM refiner short-circuits without proposing
//! KEEP/DROP edits). This is intentional: pinning the heuristic-only
//! behaviour catches any future change that drops or duplicates a
//! candidate purely because of the `TurnSummary` swap.
//!
//! The harness drives [`MemoryWritePipeline`] directly because
//! [`MemoryManager::new`] takes a live RocksDB handle (per the
//! production contract). The pipeline is what `MemoryManager::ingest`
//! delegates to, so the assertion still pins the ingest path.

#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use aura_context_memory::{
    AgentEvent, Fact, LlmRefiner, MemoryStats, MemoryStoreApi, MemoryWritePipeline, Procedure,
    RefinerConfig, TurnSummary, WriteConfig,
};
use aura_core::{AgentEventId, AgentId, FactId, ProcedureId};
use aura_reasoner::{MockProvider, MockResponse, ModelProvider};
use chrono::{DateTime, Utc};

/// Smallest possible in-memory `MemoryStoreApi` fake — keeps the
/// `aura-store-db` dep out of `aura-context-memory`'s test surface per
/// the Phase 6c brief.
#[derive(Default)]
struct FakeStore {
    facts: Mutex<Vec<Fact>>,
    events: Mutex<Vec<AgentEvent>>,
    procedures: Mutex<Vec<Procedure>>,
}

impl MemoryStoreApi for FakeStore {
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

fn pipeline_with_silent_llm() -> (Arc<FakeStore>, MemoryWritePipeline) {
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

#[tokio::test]
async fn turn_summary_with_no_iterations_produces_empty_report() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary::default();

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    // No text, no iterations, no conversation turn → nothing to do.
    assert_eq!(report.candidates_extracted, 0);
    assert_eq!(report.candidates_refined, 0);
    assert_eq!(report.facts_written, 0);
    assert_eq!(report.events_written, 0);
    assert_eq!(store.list_facts(agent_id).expect("list facts").len(), 0);
    assert_eq!(
        store.list_events(agent_id, 100).expect("list events").len(),
        0
    );
}

#[tokio::test]
async fn turn_summary_with_outcome_event_produces_one_event() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    // Drives `HeuristicExtractor::extract_task_outcome`: a non-zero
    // iteration count is enough to produce one Event candidate. Pre-Phase-6c
    // the same shape came from `AgentLoopResult { iterations: 3, .. }`.
    let summary = TurnSummary {
        iterations: 3,
        total_input_tokens: 11,
        total_output_tokens: 22,
        ..TurnSummary::default()
    };

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    assert_eq!(
        report.candidates_extracted, 1,
        "exactly one task-outcome event"
    );
    assert_eq!(report.candidates_refined, 1);
    assert_eq!(report.events_written, 1);
    assert_eq!(report.facts_written, 0);
    let events = store.list_events(agent_id, 10).expect("list events");
    assert_eq!(events.len(), 1);
    assert!(
        events[0].summary.contains("completed"),
        "expected completed outcome label, got: {}",
        events[0].summary,
    );
}

#[tokio::test]
async fn turn_summary_with_fact_text_produces_fact_candidate() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary {
        total_text: "the project uses React".to_string(),
        iterations: 1,
        ..TurnSummary::default()
    };

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    // One fact pattern matched + one task-outcome event.
    assert_eq!(report.candidates_extracted, 2);
    assert_eq!(report.candidates_refined, 2);
    assert_eq!(report.facts_written, 1);
    assert_eq!(report.events_written, 1);
    let facts = store.list_facts(agent_id).expect("list facts");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "project_technology");
}

#[tokio::test]
async fn turn_summary_with_timed_out_outcome_labels_event() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary {
        iterations: 5,
        timed_out: true,
        ..TurnSummary::default()
    };

    let _ = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");
    let events = store.list_events(agent_id, 10).expect("list events");
    assert_eq!(events.len(), 1);
    assert!(
        events[0].summary.contains("timed_out"),
        "expected timed_out label, got: {}",
        events[0].summary,
    );
}
