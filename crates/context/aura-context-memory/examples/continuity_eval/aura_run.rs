//! Runs the scenario suite against Aura's own retriever.

use std::collections::HashMap;
use std::sync::Arc;

use aura_context_memory::{
    Fact, FactSource, MemoryAccessContext, MemoryContinuity, MemoryProvenance, MemoryQueryContext,
    MemoryRetrievalMode, MemoryRetriever, MemoryStore, MemoryStoreApi, Procedure, RetrievalConfig,
};
use aura_core_types::{AgentId, FactId, ProcedureId};
use chrono::{DateTime, Utc};

use crate::bench_store::test_db;
use crate::scoring::{aggregate, score_scenario, AgentReport, ScenarioResult};
use crate::suite::{EvalRecord, EvalSuite, Scenario};

/// Maps stored memory ids (hex) back to the scenario's record ids.
type MemoryIds = HashMap<String, String>;

fn continuity(record: &EvalRecord) -> MemoryContinuity {
    MemoryContinuity {
        scope: record.scope,
        status: record.status,
        sensitivity: record.sensitivity,
        pinned: record.pinned,
        provenance: MemoryProvenance {
            excerpt: Some(format!("benchmark record {}", record.id)),
            extractor_model: Some("benchmark-fixture".to_string()),
            ..MemoryProvenance::default()
        },
        superseded_by: None,
    }
}

pub(crate) async fn run_aura(
    suite: &EvalSuite,
    mode: MemoryRetrievalMode,
    agent_name: &str,
) -> AgentReport {
    let temp = tempfile::tempdir().expect("create benchmark directory");
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(temp.path())));
    let retriever = MemoryRetriever::new(Arc::clone(&store), RetrievalConfig::default());
    let mut runs = Vec::with_capacity(suite.scenarios.len());

    for scenario in &suite.scenarios {
        let agent_id = AgentId::generate();
        let access = MemoryAccessContext {
            project_id: Some(format!("eval-project:{}", scenario.id)),
            user_id: Some("eval-user".to_string()),
            include_legacy: false,
        };
        let memory_ids = seed_scenario(store.as_ref(), scenario, agent_id, &access);
        let run = ScenarioRun {
            scenario,
            agent_id,
            access,
            memory_ids,
        };
        runs.push(run.retrieve(&retriever, mode).await);
    }

    aggregate(agent_name.to_string(), runs)
}

fn seed_scenario(
    store: &dyn MemoryStoreApi,
    scenario: &Scenario,
    agent_id: AgentId,
    access: &MemoryAccessContext,
) -> MemoryIds {
    let now = Utc::now();
    let mut memory_ids = HashMap::new();
    for record in &scenario.records {
        let partition = access.storage_id(agent_id, record.scope);
        let memory_id = match record.kind.as_str() {
            "fact" => seed_fact(store, record, partition, now),
            "procedure" => seed_procedure(store, record, partition, now),
            other => panic!("unsupported benchmark record kind: {other}"),
        };
        memory_ids.insert(memory_id, record.id.clone());
    }
    memory_ids
}

fn seed_fact(
    store: &dyn MemoryStoreApi,
    record: &EvalRecord,
    partition: AgentId,
    now: DateTime<Utc>,
) -> String {
    let fact_id = FactId::generate();
    store
        .put_fact(&Fact {
            fact_id,
            agent_id: partition,
            key: record.key.clone(),
            value: serde_json::Value::String(record.value.clone()),
            confidence: record.confidence,
            source: FactSource::Extracted,
            importance: record.importance,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            updated_at: now,
            continuity: continuity(record),
        })
        .expect("seed benchmark fact");
    fact_id.to_hex()
}

fn seed_procedure(
    store: &dyn MemoryStoreApi,
    record: &EvalRecord,
    partition: AgentId,
    now: DateTime<Utc>,
) -> String {
    let procedure_id = ProcedureId::generate();
    store
        .put_procedure(&Procedure {
            procedure_id,
            agent_id: partition,
            name: record.key.clone(),
            trigger: record.trigger.clone(),
            steps: record.steps.clone(),
            context_constraints: serde_json::Value::Null,
            success_rate: record.confidence,
            execution_count: 1,
            last_used: now,
            created_at: now,
            updated_at: now,
            skill_name: record.skill_name.clone(),
            skill_relevance: record.skill_name.as_ref().map(|_| 0.8),
            continuity: continuity(record),
        })
        .expect("seed benchmark procedure");
    procedure_id.to_hex()
}

/// One seeded scenario, ready to query.
struct ScenarioRun<'a> {
    scenario: &'a Scenario,
    agent_id: AgentId,
    access: MemoryAccessContext,
    memory_ids: MemoryIds,
}

impl ScenarioRun<'_> {
    async fn retrieve(
        self,
        retriever: &MemoryRetriever,
        mode: MemoryRetrievalMode,
    ) -> ScenarioResult {
        let scenario = self.scenario;
        let packet = retriever
            .retrieve_with_query(
                self.agent_id,
                MemoryQueryContext {
                    text: scenario.query.clone(),
                    active_skills: scenario.active_skills.clone(),
                    allow_user_scope: scenario.allow_user_scope,
                    allow_project_scope: scenario.allow_project_scope,
                    access: self.access,
                },
                mode,
            )
            .await
            .expect("run Aura retrieval");
        let trace = packet.trace.expect("retrieval trace");
        let recalled_ids: Vec<String> = trace
            .selections
            .iter()
            .filter_map(|selection| self.memory_ids.get(&selection.memory_id).cloned())
            .collect();
        score_scenario(
            scenario,
            recalled_ids,
            trace.estimated_tokens,
            trace.duration_ms,
        )
    }
}
