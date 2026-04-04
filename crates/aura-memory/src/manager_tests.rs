use crate::consolidation::ConsolidationConfig;
use crate::manager::MemoryManager;
use crate::procedures::{ProcedureConfig, StepSequence};
use crate::refinement::RefinerConfig;
use crate::retrieval::RetrievalConfig;
use crate::store::{MemoryStore, MemoryStoreApi};
use crate::write_pipeline::WriteConfig;
use aura_core::{AgentEventId, AgentId, FactId, ProcedureId};
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

fn make_manager(dir: &std::path::Path) -> (MemoryManager, Arc<dyn MemoryStoreApi>) {
    let db = test_db(dir);
    let store: Arc<dyn MemoryStoreApi> = Arc::new(MemoryStore::new(Arc::clone(&db)));
    let provider: Arc<dyn ModelProvider> = Arc::new(
        MockProvider::new().with_default_response(MockResponse::text("")),
    );
    let mgr = MemoryManager::new(
        db,
        provider,
        RefinerConfig::default(),
        WriteConfig::default(),
        RetrievalConfig::default(),
        ConsolidationConfig::default(),
        ProcedureConfig::default(),
    );
    (mgr, store)
}

#[tokio::test]
async fn retrieve_returns_stored_data() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, store) = make_manager(dir.path());
    let agent = AgentId::generate();

    let fact = crate::types::Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "test_key".to_string(),
        value: serde_json::Value::String("test_val".into()),
        confidence: 0.9,
        source: crate::types::FactSource::Extracted,
        importance: 0.5,
        access_count: 0,
        last_accessed: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_fact(&fact).unwrap();

    let packet = mgr.retrieve(agent).await.unwrap();
    assert_eq!(packet.facts.len(), 1);
    assert_eq!(packet.facts[0].key, "test_key");
}

#[tokio::test]
async fn prepare_context_injects_memory() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, store) = make_manager(dir.path());
    let agent = AgentId::generate();

    let fact = crate::types::Fact {
        fact_id: FactId::generate(),
        agent_id: agent,
        key: "lang".to_string(),
        value: serde_json::Value::String("Rust".into()),
        confidence: 0.9,
        source: crate::types::FactSource::Extracted,
        importance: 0.5,
        access_count: 0,
        last_accessed: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_fact(&fact).unwrap();

    let mut config = aura_agent::AgentLoopConfig {
        system_prompt: "You are helpful.".to_string(),
        ..Default::default()
    };

    mgr.prepare_context(agent, &mut config).await;
    assert!(
        config.system_prompt.contains("<agent_memory>"),
        "system prompt should contain <agent_memory> block"
    );
    assert!(
        config.system_prompt.contains("lang"),
        "system prompt should contain the fact key"
    );
}

#[tokio::test]
async fn prepare_context_strips_old_memory_block() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, _store) = make_manager(dir.path());
    let agent = AgentId::generate();

    let mut config = aura_agent::AgentLoopConfig {
        system_prompt: "Base prompt\n<agent_memory>\nold stuff\n</agent_memory>".to_string(),
        ..Default::default()
    };

    mgr.prepare_context(agent, &mut config).await;
    assert!(!config.system_prompt.contains("old stuff"));
}

#[test]
fn extract_procedures_from_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, store) = make_manager(dir.path());
    let agent = AgentId::generate();

    for _ in 0..3 {
        let event = crate::types::AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id: agent,
            event_type: "task_run".to_string(),
            summary: "did stuff".to_string(),
            metadata: serde_json::json!({"tool_sequence": ["read", "edit", "build", "test"]}),
            importance: 0.5,
            access_count: 0,
            last_accessed: Utc::now(),
            timestamp: Utc::now(),
        };
        store.put_event(&event).unwrap();
    }

    let seq = StepSequence {
        steps: vec![
            "read".into(),
            "edit".into(),
            "build".into(),
            "test".into(),
        ],
        task_hint: Some("fix the bug".into()),
        succeeded: true,
    };

    let proc = mgr.extract_procedures(agent, &seq).unwrap();
    assert!(proc.is_some(), "should extract a procedure from recurring pattern");
}

#[test]
fn match_procedures_returns_matches() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, store) = make_manager(dir.path());
    let agent = AgentId::generate();

    let proc = crate::types::Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id: agent,
        name: "deploy flow".to_string(),
        trigger: "deploy the application to production".to_string(),
        steps: vec!["build".into(), "push".into(), "verify".into()],
        context_constraints: serde_json::Value::Null,
        success_rate: 0.9,
        execution_count: 10,
        last_used: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.put_procedure(&proc).unwrap();

    let matches = mgr.match_procedures(agent, "deploy application").unwrap();
    assert!(!matches.is_empty());
}

#[test]
fn record_procedure_feedback_updates_rate() {
    let dir = tempfile::tempdir().unwrap();
    let (mgr, store) = make_manager(dir.path());
    let agent = AgentId::generate();

    let proc = crate::types::Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id: agent,
        name: "test proc".to_string(),
        trigger: "test".to_string(),
        steps: vec!["a".into(), "b".into(), "c".into()],
        context_constraints: serde_json::Value::Null,
        success_rate: 0.5,
        execution_count: 3,
        last_used: Utc::now(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let pid = proc.procedure_id;
    store.put_procedure(&proc).unwrap();

    mgr.record_procedure_feedback(agent, pid, true, None)
        .unwrap();
    let updated = store.get_procedure(agent, pid).unwrap();
    assert!(
        updated.success_rate > 0.5,
        "success rate should increase after positive feedback"
    );
    assert_eq!(updated.execution_count, 4);
}
