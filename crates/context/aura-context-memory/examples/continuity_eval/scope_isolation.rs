//! Checks that query-aware retrieval shares exactly the intended scopes.

use std::collections::HashSet;
use std::sync::Arc;

use aura_context_memory::{
    Fact, FactSource, MemoryAccessContext, MemoryContinuity, MemoryQueryContext,
    MemoryRetrievalMode, MemoryRetriever, MemoryScope, MemoryStore, MemoryStoreApi,
    RetrievalConfig,
};
use aura_core_types::{AgentId, FactId};
use chrono::Utc;
use serde::Serialize;

use crate::bench_store::test_db;

/// Markers `agent_b` must recall in project X as user U.
const EXPECTED_KEYS: [&str; 3] = [
    "marker_agent_b_private",
    "marker_project_x_shared",
    "marker_user_u_personal",
];

/// Markers that would leak another agent's, project's or user's memory.
const FORBIDDEN_KEYS: [&str; 4] = [
    "marker_agent_a_private",
    "marker_project_y_shared",
    "marker_user_v_personal",
    "marker_legacy_unscoped",
];

#[derive(Debug, Serialize)]
pub(crate) struct ScopeIsolationReport {
    pub(crate) checks_passed: usize,
    pub(crate) checks_total: usize,
    relevant_recall_rate: f64,
    pub(crate) forbidden_leakage_rate: f64,
    recalled_keys: Vec<String>,
    forbidden_keys: Vec<String>,
}

pub(crate) async fn evaluate_scope_isolation() -> ScopeIsolationReport {
    let temp = tempfile::tempdir().expect("create scope benchmark directory");
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(temp.path())));
    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();
    let project_x_user_u = access_context("project-x", "user-u");
    let project_y_user_v = access_context("project-y", "user-v");

    let seeds = [
        (
            project_x_user_u.storage_id(agent_a, MemoryScope::Agent),
            MemoryScope::Agent,
            "marker_agent_a_private",
        ),
        (
            project_x_user_u.storage_id(agent_b, MemoryScope::Agent),
            MemoryScope::Agent,
            "marker_agent_b_private",
        ),
        (
            project_x_user_u.storage_id(agent_a, MemoryScope::Project),
            MemoryScope::Project,
            "marker_project_x_shared",
        ),
        (
            project_x_user_u.storage_id(agent_a, MemoryScope::User),
            MemoryScope::User,
            "marker_user_u_personal",
        ),
        (
            project_y_user_v.storage_id(agent_a, MemoryScope::Project),
            MemoryScope::Project,
            "marker_project_y_shared",
        ),
        (
            project_y_user_v.storage_id(agent_a, MemoryScope::User),
            MemoryScope::User,
            "marker_user_v_personal",
        ),
        (agent_b, MemoryScope::Agent, "marker_legacy_unscoped"),
    ];
    seed_markers(store.as_ref(), &seeds);

    let recalled = recall_markers(store, agent_b, project_x_user_u).await;
    score_isolation(&recalled)
}

fn access_context(project_id: &str, user_id: &str) -> MemoryAccessContext {
    MemoryAccessContext {
        project_id: Some(project_id.to_string()),
        user_id: Some(user_id.to_string()),
        include_legacy: false,
    }
}

fn seed_markers(store: &dyn MemoryStoreApi, seeds: &[(AgentId, MemoryScope, &str)]) {
    let now = Utc::now();
    for &(partition, scope, key) in seeds {
        store
            .put_fact(&Fact {
                fact_id: FactId::generate(),
                agent_id: partition,
                key: key.to_string(),
                value: serde_json::Value::String(key.to_string()),
                confidence: 0.99,
                source: FactSource::Extracted,
                importance: 0.9,
                access_count: 0,
                last_accessed: now,
                created_at: now,
                updated_at: now,
                continuity: MemoryContinuity {
                    scope,
                    ..MemoryContinuity::default()
                },
            })
            .expect("seed scope benchmark fact");
    }
}

/// Keys of the facts `agent_id` recalls for the query "marker".
async fn recall_markers(
    store: Arc<dyn MemoryStoreApi>,
    agent_id: AgentId,
    access: MemoryAccessContext,
) -> HashSet<String> {
    let packet = MemoryRetriever::new(
        store,
        RetrievalConfig {
            max_facts: 20,
            token_budget: 10_000,
            ..RetrievalConfig::default()
        },
    )
    .retrieve_with_query(
        agent_id,
        MemoryQueryContext {
            text: "marker".to_string(),
            allow_user_scope: true,
            allow_project_scope: true,
            access,
            ..MemoryQueryContext::default()
        },
        MemoryRetrievalMode::QueryAware,
    )
    .await
    .expect("run scope benchmark retrieval");

    packet.facts.into_iter().map(|fact| fact.key).collect()
}

fn recalled_among(recalled: &HashSet<String>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter(|key| recalled.contains(**key))
        .map(|key| (*key).to_string())
        .collect()
}

fn score_isolation(recalled: &HashSet<String>) -> ScopeIsolationReport {
    let recalled_keys = recalled_among(recalled, &EXPECTED_KEYS);
    let forbidden_keys = recalled_among(recalled, &FORBIDDEN_KEYS);
    let checks_passed = recalled_keys.len() + FORBIDDEN_KEYS.len() - forbidden_keys.len();
    ScopeIsolationReport {
        checks_passed,
        checks_total: EXPECTED_KEYS.len() + FORBIDDEN_KEYS.len(),
        relevant_recall_rate: recalled_keys.len() as f64 / EXPECTED_KEYS.len() as f64,
        forbidden_leakage_rate: forbidden_keys.len() as f64 / FORBIDDEN_KEYS.len() as f64,
        recalled_keys,
        forbidden_keys,
    }
}
