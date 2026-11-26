//! A conflicting write to shared memory from a second agent waits for
//! approval instead of superseding the first agent's fact.

use aura_context_memory::{
    ConversationTurn, Fact, IngestProvenance, MemoryAccessContext, MemoryScope, MemoryStatus,
    MemoryStoreApi, MemoryWritePipeline, RefinementRequestContext, TurnSummary,
};
use aura_core_types::AgentId;

use crate::support::pipeline_with_llm_responses;

/// Ingests the same turn as `agent_a`, then `agent_b`; the second write must
/// report one pending conflict.
async fn ingest_from_both_agents(
    pipeline: &MemoryWritePipeline,
    agent_a: AgentId,
    agent_b: AgentId,
) {
    let context = RefinementRequestContext {
        aura_project_id: Some("project-shared".to_string()),
        user_id: Some("user-shared".to_string()),
        ..RefinementRequestContext::default()
    };
    let turn = ConversationTurn {
        user_message: "Remember the deployment region".to_string(),
        assistant_text: "Noted".to_string(),
    };

    for agent_id in [agent_a, agent_b] {
        let report = pipeline
            .ingest_with_provenance_and_request_context(
                agent_id,
                &TurnSummary::default(),
                Some(&turn),
                context.clone(),
                &[],
                IngestProvenance {
                    source_session_id: Some("session-project"),
                    initial_status: MemoryStatus::Active,
                },
            )
            .await
            .expect("ingest shared candidate");
        if agent_id == agent_b {
            assert_eq!(report.conflicts_pending, 1);
        }
    }
}

fn count_with_status(facts: &[Fact], status: MemoryStatus) -> usize {
    facts
        .iter()
        .filter(|fact| fact.continuity.status == status)
        .count()
}

async fn assert_conflicting_shared_memory_requires_approval(
    scope: MemoryScope,
    responses: &[&str],
) {
    let (store, pipeline) = pipeline_with_llm_responses(responses);
    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();

    ingest_from_both_agents(&pipeline, agent_a, agent_b).await;

    let partition = MemoryAccessContext {
        project_id: Some("project-shared".to_string()),
        user_id: Some("user-shared".to_string()),
        include_legacy: false,
    }
    .storage_id(agent_a, scope);
    let facts = store.list_facts(partition).expect("list shared facts");
    assert_eq!(facts.len(), 2);
    assert_eq!(count_with_status(&facts, MemoryStatus::Active), 1);
    assert_eq!(count_with_status(&facts, MemoryStatus::Pending), 1);
}

#[tokio::test]
async fn conflicting_shared_memory_from_another_agent_requires_approval() {
    assert_conflicting_shared_memory_requires_approval(
        MemoryScope::Project,
        &[
            "FACT key=\"deploy_region\" value=\"us-east\" scope=project confidence=0.99 importance=0.9",
            "FACT key=\"deploy_region\" value=\"eu-west\" scope=project confidence=0.99 importance=0.9",
        ],
    )
    .await;
    assert_conflicting_shared_memory_requires_approval(
        MemoryScope::User,
        &[
            "FACT key=\"response_style\" value=\"concise\" scope=user confidence=0.99 importance=0.9",
            "FACT key=\"response_style\" value=\"detailed\" scope=user confidence=0.99 importance=0.9",
        ],
    )
    .await;
}
