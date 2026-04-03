use crate::consolidation::{ConsolidationConfig, MemoryConsolidator};
use crate::store::{MemoryStore, MemoryStoreApi};
use crate::types::{AgentEvent, Fact, FactSource, Procedure};
use aura_core::{AgentEventId, AgentId, FactId, ProcedureId};
use aura_reasoner::{MockProvider, MockResponse, ModelProvider};
use chrono::{Duration, Utc};
use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};
use std::sync::Arc;

fn test_db(dir: &std::path::Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let cfs = vec![
        ColumnFamilyDescriptor::new("record", Options::default()),
        ColumnFamilyDescriptor::new("agent_meta", Options::default()),
        ColumnFamilyDescriptor::new("inbox", Options::default()),
        ColumnFamilyDescriptor::new("memory_facts", Options::default()),
        ColumnFamilyDescriptor::new("memory_events", Options::default()),
        ColumnFamilyDescriptor::new("memory_procedures", Options::default()),
        ColumnFamilyDescriptor::new("memory_event_index", Options::default()),
        ColumnFamilyDescriptor::new("agent_skills", Options::default()),
    ];
    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

fn test_store(dir: &std::path::Path) -> Arc<dyn MemoryStoreApi> {
    Arc::new(MemoryStore::new(test_db(dir)))
}

#[tokio::test]
async fn forget_prunes_low_importance_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let old_time = Utc::now() - Duration::days(90);
    let fact = Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "stale".to_string(),
        value: serde_json::Value::String("old".into()),
        confidence: 0.5,
        source: FactSource::Extracted,
        importance: 0.05,
        access_count: 0,
        last_accessed: old_time,
        created_at: old_time,
        updated_at: old_time,
    };
    store.put_fact(&fact).unwrap();

    let keep = Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "keep_me".to_string(),
        value: serde_json::Value::String("important".into()),
        confidence: 0.9,
        source: FactSource::Extracted,
        importance: 0.8,
        access_count: 5,
        last_accessed: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_fact(&keep).unwrap();

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text("")),
    );
    let consolidator =
        MemoryConsolidator::new(Arc::clone(&store), provider, ConsolidationConfig::default());
    let report = consolidator.consolidate(agent).await.unwrap();

    assert_eq!(report.facts_forgotten, 1);
    let remaining = store.list_facts(agent).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].key, "keep_me");
}

#[tokio::test]
async fn forget_prunes_low_success_procedures() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let bad_proc = Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id: agent,
        name: "bad".to_string(),
        trigger: "test".to_string(),
        steps: vec!["a".into()],
        context_constraints: serde_json::Value::Null,
        success_rate: 0.1,
        execution_count: 5,
        last_used: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_procedure(&bad_proc).unwrap();

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text("")),
    );
    let consolidator =
        MemoryConsolidator::new(Arc::clone(&store), provider, ConsolidationConfig::default());
    let report = consolidator.consolidate(agent).await.unwrap();

    assert_eq!(report.procedures_forgotten, 1);
    assert!(store.list_procedures(agent).unwrap().is_empty());
}

#[tokio::test]
async fn compress_events_creates_summaries() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let config = ConsolidationConfig {
        max_events_before_compression: 5,
        ..Default::default()
    };

    for i in 0..10 {
        let event = AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id: agent,
            event_type: "task_run".to_string(),
            summary: format!("did thing {i}"),
            metadata: serde_json::Value::Null,
            importance: 0.5,
            access_count: 0,
            last_accessed: Utc::now(),
            timestamp: Utc::now() + Duration::milliseconds(i),
        };
        store.put_event(&event).unwrap();
    }

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new()
            .with_response(MockResponse::text(
                "SUMMARY: Batch of routine tasks completed\nSUMMARY: Several things done",
            ))
            .with_default_response(MockResponse::text("")),
    );

    let consolidator = MemoryConsolidator::new(Arc::clone(&store), provider, config);
    let report = consolidator.consolidate(agent).await.unwrap();

    assert_eq!(report.events_compressed, 2);
    assert_eq!(report.events_deleted, 5);
}

#[tokio::test]
async fn evolve_merges_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let f1 = Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "lang".to_string(),
        value: serde_json::Value::String("Rust".into()),
        confidence: 0.8,
        source: FactSource::Extracted,
        importance: 0.6,
        access_count: 0,
        last_accessed: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let f2 = Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "primary_language".to_string(),
        value: serde_json::Value::String("Rust".into()),
        confidence: 0.9,
        source: FactSource::Extracted,
        importance: 0.7,
        access_count: 1,
        last_accessed: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_fact(&f1).unwrap();
    store.put_fact(&f2).unwrap();

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text(
            "MERGE 1 2 key=\"primary_language\" value=\"Rust\"\n\
             INSIGHT key=\"tech_stack\" value=\"Rust-based project\"",
        )),
    );

    let consolidator =
        MemoryConsolidator::new(Arc::clone(&store), provider, ConsolidationConfig::default());
    let report = consolidator.consolidate(agent).await.unwrap();

    assert_eq!(report.facts_merged, 1);
    assert_eq!(report.insights_created, 1);

    let facts = store.list_facts(agent).unwrap();
    assert_eq!(facts.len(), 2);
}

#[tokio::test]
async fn consolidate_below_threshold_no_compression() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    for i in 0..2 {
        let event = AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id: agent,
            event_type: "t".to_string(),
            summary: format!("e{i}"),
            metadata: serde_json::Value::Null,
            importance: 0.5,
            access_count: 0,
            last_accessed: Utc::now(),
            timestamp: Utc::now() + Duration::milliseconds(i),
        };
        store.put_event(&event).unwrap();
    }

    let provider: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new().with_default_response(MockResponse::text("")));
    let consolidator =
        MemoryConsolidator::new(Arc::clone(&store), provider, ConsolidationConfig::default());
    let report = consolidator.consolidate(agent).await.unwrap();

    assert_eq!(report.events_compressed, 0);
    assert_eq!(report.events_deleted, 0);
}
