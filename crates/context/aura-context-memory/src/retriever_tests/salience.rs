//! Salience retrieval: limits, filters, ordering and access tracking.

use super::support::{make_event, make_fact, make_procedure, test_db};
use crate::retrieval::{MemoryRetriever, RetrievalConfig};
use crate::store::{MemoryStore, MemoryStoreApi};
use aura_core_types::AgentId;
use chrono::{Duration, Utc};
use std::sync::Arc;

#[tokio::test]
async fn retrieve_empty_store_returns_empty_packet() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let retriever = MemoryRetriever::new(store, RetrievalConfig::default());
    let agent = AgentId::generate();

    let packet = retriever.retrieve(agent).await.unwrap();
    assert!(packet.facts.is_empty());
    assert!(packet.events.is_empty());
    assert!(packet.procedures.is_empty());
    assert!(packet.is_empty());
}

#[tokio::test]
async fn retrieve_returns_facts_events_procedures() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();
    let now = Utc::now();

    store
        .put_fact(&make_fact(agent, "lang", "Rust", 0.8, 0.9))
        .unwrap();
    store
        .put_event(&make_event(agent, "task", "did stuff", now))
        .unwrap();
    store
        .put_procedure(&make_procedure(agent, "deploy", &["build", "push"]))
        .unwrap();

    let retriever = MemoryRetriever::new(store, RetrievalConfig::default());
    let packet = retriever.retrieve(agent).await.unwrap();

    assert_eq!(packet.facts.len(), 1);
    assert_eq!(packet.events.len(), 1);
    assert_eq!(packet.procedures.len(), 1);
    assert_eq!(packet.facts[0].key, "lang");
}

#[tokio::test]
async fn retrieve_filters_low_confidence_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    store
        .put_fact(&make_fact(agent, "low1", "v", 0.5, 0.1))
        .unwrap();
    store
        .put_fact(&make_fact(agent, "low2", "v", 0.5, 0.2))
        .unwrap();
    store
        .put_fact(&make_fact(agent, "high", "v", 0.5, 0.9))
        .unwrap();

    let config = RetrievalConfig {
        min_confidence: 0.3,
        ..RetrievalConfig::default()
    };
    let retriever = MemoryRetriever::new(store, config);
    let packet = retriever.retrieve(agent).await.unwrap();

    assert_eq!(packet.facts.len(), 1);
    assert_eq!(packet.facts[0].key, "high");
}

#[tokio::test]
async fn retrieve_respects_max_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    for i in 0..30 {
        store
            .put_fact(&make_fact(agent, &format!("fact_{i}"), "v", 0.5, 0.9))
            .unwrap();
    }

    let config = RetrievalConfig {
        max_facts: 5,
        token_budget: 100_000,
        ..RetrievalConfig::default()
    };
    let retriever = MemoryRetriever::new(store, config);
    let packet = retriever.retrieve(agent).await.unwrap();

    assert_eq!(packet.facts.len(), 5);
}

#[tokio::test]
async fn retrieve_respects_max_events() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();
    let now = Utc::now();

    for i in 0..20 {
        store
            .put_event(&make_event(
                agent,
                "task",
                &format!("event {i}"),
                now + Duration::seconds(i),
            ))
            .unwrap();
    }

    let config = RetrievalConfig {
        max_events: 3,
        token_budget: 100_000,
        ..RetrievalConfig::default()
    };
    let retriever = MemoryRetriever::new(store, config);
    let packet = retriever.retrieve(agent).await.unwrap();

    assert_eq!(packet.events.len(), 3);
}

#[tokio::test]
async fn retrieve_sorts_by_salience() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    store
        .put_fact(&make_fact(agent, "low_importance", "v", 0.1, 0.9))
        .unwrap();
    store
        .put_fact(&make_fact(agent, "high_importance", "v", 0.95, 0.9))
        .unwrap();
    store
        .put_fact(&make_fact(agent, "mid_importance", "v", 0.5, 0.9))
        .unwrap();

    let config = RetrievalConfig {
        token_budget: 100_000,
        ..RetrievalConfig::default()
    };
    let retriever = MemoryRetriever::new(store, config);
    let packet = retriever.retrieve(agent).await.unwrap();

    assert_eq!(packet.facts.len(), 3);
    assert_eq!(packet.facts[0].key, "high_importance");
    assert_eq!(packet.facts[1].key, "mid_importance");
    assert_eq!(packet.facts[2].key, "low_importance");
}

#[tokio::test]
async fn retrieve_updates_access_count() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    let fact = make_fact(agent, "tracked", "v", 0.8, 0.9);
    let fact_id = fact.fact_id;
    store.put_fact(&fact).unwrap();

    assert_eq!(store.get_fact(agent, fact_id).unwrap().access_count, 0);

    let retriever = MemoryRetriever::new(Arc::clone(&store), RetrievalConfig::default());
    let packet = retriever.retrieve(agent).await.unwrap();
    assert_eq!(packet.facts.len(), 1);

    let updated = store.get_fact(agent, fact_id).unwrap();
    assert_eq!(updated.access_count, 1);
}

#[tokio::test]
async fn retrieve_token_budget_limits_output() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(test_db(dir.path())));
    let agent = AgentId::generate();

    // Each fact costs roughly (4 + key_len + 18 + val_len) / 4 tokens.
    // With long values this adds up quickly.
    for i in 0..20 {
        let long_val = "x".repeat(200);
        store
            .put_fact(&make_fact(agent, &format!("fact_{i}"), &long_val, 0.8, 0.9))
            .unwrap();
    }

    let config = RetrievalConfig {
        max_facts: 20,
        token_budget: 50,
        ..RetrievalConfig::default()
    };
    let retriever = MemoryRetriever::new(store, config);
    let packet = retriever.retrieve(agent).await.unwrap();

    // With a 50-token budget, only a fraction of the 20 facts should fit.
    assert!(packet.facts.len() < 20);
    // A ~200-char value → ~55+ tokens per fact, so the budget of 50 should
    // allow at most 0–1 facts.
    assert!(packet.facts.len() <= 1);
}
