//! Query-aware retrieval: relevance ranking and status/sensitivity/scope filters.

use super::support::{make_fact, test_db};
use crate::retrieval::{MemoryQueryContext, MemoryRetriever, RetrievalConfig};
use crate::store::{MemoryStore, MemoryStoreApi};
use crate::types::{MemoryRetrievalMode, MemoryScope, MemorySensitivity, MemoryStatus};
use aura_core_types::AgentId;
use std::sync::Arc;

#[tokio::test]
async fn query_aware_retrieval_prefers_request_relevance_over_raw_salience() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    store
        .put_fact(&make_fact(
            agent,
            "test_command",
            "cargo test --workspace",
            0.35,
            0.9,
        ))
        .unwrap();
    store
        .put_fact(&make_fact(
            agent,
            "favorite_color",
            "cobalt blue",
            1.0,
            0.99,
        ))
        .unwrap();

    let retriever = MemoryRetriever::new(
        store,
        RetrievalConfig {
            max_facts: 1,
            token_budget: 1_000,
            ..RetrievalConfig::default()
        },
    );
    let packet = retriever
        .retrieve_with_query(
            agent,
            MemoryQueryContext {
                text: "Run the project test command".into(),
                ..MemoryQueryContext::default()
            },
            MemoryRetrievalMode::QueryAware,
        )
        .await
        .unwrap();

    assert_eq!(packet.facts[0].key, "test_command");
    let trace = packet.trace.unwrap();
    assert_eq!(trace.candidate_count, 2);
    assert_eq!(trace.selected_count, 1);
    assert!(trace.query_aware);
    assert_eq!(trace.selections[0].reason, "current_request");
}

#[tokio::test]
async fn query_aware_retrieval_does_not_spend_tokens_on_unrelated_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();
    store
        .put_fact(&make_fact(
            agent,
            "build_command",
            "cargo build --release",
            0.5,
            0.9,
        ))
        .unwrap();
    store
        .put_fact(&make_fact(
            agent,
            "favorite_color",
            "cobalt blue",
            1.0,
            0.99,
        ))
        .unwrap();

    let packet = MemoryRetriever::new(store, RetrievalConfig::default())
        .retrieve_with_query(
            agent,
            MemoryQueryContext {
                text: "What is the build command?".into(),
                ..MemoryQueryContext::default()
            },
            MemoryRetrievalMode::QueryAware,
        )
        .await
        .unwrap();

    assert_eq!(packet.facts.len(), 1);
    assert_eq!(packet.facts[0].key, "build_command");
    assert_eq!(packet.trace.unwrap().selected_count, 1);
}

#[tokio::test]
async fn retrieval_excludes_pending_sensitive_and_unapproved_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    let mut active = make_fact(agent, "build_command", "cargo build", 0.7, 0.9);
    active.continuity.status = MemoryStatus::Active;
    store.put_fact(&active).unwrap();

    let mut pending = make_fact(agent, "pending_command", "do not recall", 1.0, 0.99);
    pending.continuity.status = MemoryStatus::Pending;
    store.put_fact(&pending).unwrap();

    let mut sensitive = make_fact(agent, "password", "do not recall", 1.0, 0.99);
    sensitive.continuity.sensitivity = MemorySensitivity::Sensitive;
    store.put_fact(&sensitive).unwrap();

    let mut user_scoped = make_fact(agent, "user_preference", "concise answers", 1.0, 0.99);
    user_scoped.continuity.scope = MemoryScope::User;
    store.put_fact(&user_scoped).unwrap();

    let retriever = MemoryRetriever::new(store, RetrievalConfig::default());
    let packet = retriever
        .retrieve_with_query(
            agent,
            MemoryQueryContext {
                text: "build command pending password preference".into(),
                ..MemoryQueryContext::default()
            },
            MemoryRetrievalMode::QueryAware,
        )
        .await
        .unwrap();

    assert_eq!(packet.facts.len(), 1);
    assert_eq!(packet.facts[0].key, "build_command");
    assert_eq!(packet.trace.unwrap().candidate_count, 4);
}
