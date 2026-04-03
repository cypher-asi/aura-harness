use crate::refinement::{LlmRefiner, RefinerConfig};
use crate::store::{MemoryStore, MemoryStoreApi};
use crate::write_pipeline::{MemoryWritePipeline, WriteConfig};
use aura_agent::AgentLoopResult;
use aura_core::{AgentId, FactId};
use aura_reasoner::{MockProvider, MockResponse, ModelProvider};
use chrono::Utc;
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
async fn ingest_extracts_and_writes_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text(
            "1. KEEP key=\"project_technology\" confidence=0.9 importance=0.7\n\
             2. KEEP key=\"task_outcome\" confidence=0.8 importance=0.6",
        )),
    );

    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let pipeline = MemoryWritePipeline::new(Arc::clone(&store), refiner, WriteConfig::default());

    let result = AgentLoopResult {
        total_text: "The project uses Rust for the backend".to_string(),
        iterations: 3,
        ..Default::default()
    };

    let report = pipeline.ingest(agent, &result).await.unwrap();
    assert!(report.candidates_extracted > 0);
    assert!(report.facts_written > 0 || report.events_written > 0);

    let facts = store.list_facts(agent).unwrap();
    let events = store.list_events(agent, 100).unwrap();
    assert!(!facts.is_empty() || !events.is_empty());
}

#[tokio::test]
async fn ingest_drops_low_confidence_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text(
            "1. DROP key=\"project_technology\" confidence=0.1 importance=0.1 reason=\"transient\"\n\
             2. DROP key=\"task_outcome\" confidence=0.1 importance=0.1 reason=\"transient\"",
        )),
    );

    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let pipeline = MemoryWritePipeline::new(Arc::clone(&store), refiner, WriteConfig::default());

    let result = AgentLoopResult {
        total_text: "The project uses Go for the backend".to_string(),
        iterations: 1,
        ..Default::default()
    };

    let report = pipeline.ingest(agent, &result).await.unwrap();
    assert!(report.candidates_dropped > 0);
    assert_eq!(report.facts_written, 0);
}

#[tokio::test]
async fn ingest_empty_result_skips_refinement() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new());
    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let pipeline = MemoryWritePipeline::new(Arc::clone(&store), refiner, WriteConfig::default());

    let result = AgentLoopResult::default();
    let report = pipeline.ingest(agent, &result).await.unwrap();
    assert_eq!(report.candidates_extracted, 0);
    assert_eq!(report.candidates_refined, 0);
}

#[tokio::test]
async fn ingest_enforces_fact_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    let agent = AgentId::generate();

    let config = WriteConfig {
        max_facts_per_agent: 3,
        ..Default::default()
    };
    for i in 0..3 {
        let fact = crate::types::Fact {
            fact_id: FactId::generate(),
            agent_id: agent,
            key: format!("existing_{i}"),
            value: serde_json::Value::String("val".into()),
            confidence: 0.9,
            source: crate::types::FactSource::Extracted,
            importance: 0.1,
            access_count: 0,
            last_accessed: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.put_fact(&fact).unwrap();
    }

    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text(
            "1. KEEP key=\"new_tech\" confidence=0.95 importance=0.9\n\
             2. KEEP key=\"task_outcome\" confidence=0.8 importance=0.6",
        )),
    );

    let refiner = LlmRefiner::new(provider, RefinerConfig::default());
    let pipeline = MemoryWritePipeline::new(Arc::clone(&store), refiner, config);

    let result = AgentLoopResult {
        total_text: "The project uses Python for scripting".to_string(),
        iterations: 2,
        ..Default::default()
    };

    let _report = pipeline.ingest(agent, &result).await.unwrap();
    let facts = store.list_facts(agent).unwrap();
    assert!(
        facts.len() <= 3,
        "Facts should be capped at max_facts_per_agent, got {}",
        facts.len()
    );
}
