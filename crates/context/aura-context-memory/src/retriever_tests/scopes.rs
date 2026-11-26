//! Scoped retrieval across agent, project and user partitions.

use super::support::{make_fact, test_db};
use crate::retrieval::{MemoryQueryContext, MemoryRetriever, RetrievalConfig};
use crate::store::{MemoryStore, MemoryStoreApi};
use crate::types::{MemoryAccessContext, MemoryPacket, MemoryRetrievalMode, MemoryScope};
use aura_core_types::AgentId;
use std::collections::HashSet;
use std::sync::Arc;

fn access_context(project_id: &str, user_id: &str) -> MemoryAccessContext {
    MemoryAccessContext {
        project_id: Some(project_id.into()),
        user_id: Some(user_id.into()),
        include_legacy: false,
    }
}

/// Seeds one marker fact per partition, plus a legacy unscoped fact for
/// `agent_b`.
fn seed_scoped_markers(
    store: &dyn MemoryStoreApi,
    agent_a: AgentId,
    agent_b: AgentId,
    project_x_user_u: &MemoryAccessContext,
    project_y_user_v: &MemoryAccessContext,
) {
    let seeds = [
        (
            project_x_user_u.storage_id(agent_a, MemoryScope::Agent),
            MemoryScope::Agent,
            "marker_agent_a_project_x",
        ),
        (
            project_x_user_u.storage_id(agent_b, MemoryScope::Agent),
            MemoryScope::Agent,
            "marker_agent_b_project_x",
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
    ];
    for (partition, scope, key) in seeds {
        let mut fact = make_fact(partition, key, key, 0.9, 0.99);
        fact.continuity.scope = scope;
        store.put_fact(&fact).unwrap();
    }
    store
        .put_fact(&make_fact(
            agent_b,
            "marker_legacy_global_agent",
            "marker_legacy_global_agent",
            0.9,
            0.99,
        ))
        .unwrap();
}

async fn retrieve_markers(
    store: Arc<dyn MemoryStoreApi>,
    agent_id: AgentId,
    access: MemoryAccessContext,
) -> MemoryPacket {
    MemoryRetriever::new(
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
            text: "marker".into(),
            allow_user_scope: true,
            allow_project_scope: true,
            access,
            ..MemoryQueryContext::default()
        },
        MemoryRetrievalMode::QueryAware,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn scoped_retrieval_shares_only_the_intended_three_layers() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();
    let project_x_user_u = access_context("project-x", "user-u");
    let project_y_user_v = access_context("project-y", "user-v");

    seed_scoped_markers(
        store.as_ref(),
        agent_a,
        agent_b,
        &project_x_user_u,
        &project_y_user_v,
    );
    let packet = retrieve_markers(store, agent_b, project_x_user_u).await;

    let keys: HashSet<_> = packet.facts.iter().map(|fact| fact.key.as_str()).collect();
    assert_eq!(
        keys,
        HashSet::from([
            "marker_agent_b_project_x",
            "marker_project_x_shared",
            "marker_user_u_personal",
        ])
    );
    assert!(packet
        .trace
        .unwrap()
        .selections
        .iter()
        .all(|selection| matches!(
            selection.scope,
            MemoryScope::Agent | MemoryScope::Project | MemoryScope::User
        )));
}
