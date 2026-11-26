use super::*;
use crate::types::FactSource;
use aura_store_db::SealCipher;
use rocksdb::{ColumnFamilyDescriptor, Options};

fn test_db(dir: &std::path::Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let cfs = vec![
        ColumnFamilyDescriptor::new(cf::MEMORY_FACTS, Options::default()),
        ColumnFamilyDescriptor::new(cf::MEMORY_EVENTS, Options::default()),
        ColumnFamilyDescriptor::new(cf::MEMORY_PROCEDURES, Options::default()),
        ColumnFamilyDescriptor::new(cf::MEMORY_EVENT_INDEX, Options::default()),
        ColumnFamilyDescriptor::new(cf::MEMORY_CONFIG, Options::default()),
    ];
    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

fn make_fact(agent_id: AgentId, key: &str, val: &str) -> Fact {
    let now = Utc::now();
    Fact {
        fact_id: FactId::generate(),
        agent_id,
        key: key.to_string(),
        value: serde_json::Value::String(val.to_string()),
        confidence: 0.9,
        source: FactSource::Extracted,
        importance: 0.5,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        updated_at: now,
        continuity: crate::types::MemoryContinuity::default(),
    }
}

fn make_event(agent_id: AgentId, summary: &str, ts: DateTime<Utc>) -> AgentEvent {
    AgentEvent {
        event_id: AgentEventId::generate(),
        agent_id,
        event_type: "run".to_string(),
        summary: summary.to_string(),
        metadata: serde_json::Value::Null,
        importance: 0.6,
        access_count: 0,
        last_accessed: ts,
        timestamp: ts,
        continuity: crate::types::MemoryContinuity::default(),
    }
}

fn make_procedure(agent_id: AgentId, name: &str) -> Procedure {
    let now = Utc::now();
    Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id,
        name: name.to_string(),
        trigger: "test trigger".to_string(),
        steps: vec!["build".to_string(), "push".to_string()],
        context_constraints: serde_json::Value::Null,
        success_rate: 0.8,
        execution_count: 5,
        last_used: now,
        created_at: now,
        updated_at: now,
        skill_name: None,
        skill_relevance: None,
        continuity: crate::types::MemoryContinuity::default(),
    }
}

fn sealed_store(dir: &std::path::Path) -> MemoryStore {
    let cipher = Arc::new(SealCipher::new(&[9u8; 32]));
    MemoryStore::with_cipher(test_db(dir), Some(cipher))
}

// ----------------------------------------------------------------
// Sealed state-at-rest (Swarm TEE upgrade phase 5)
// ----------------------------------------------------------------

#[test]
fn sealed_fact_roundtrip_and_ciphertext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = sealed_store(dir.path());
    let agent = AgentId::generate();
    let fact = make_fact(agent, "secret-key", "secret-value");

    store.put_fact(&fact).unwrap();
    assert_eq!(
        store.get_fact(agent, fact.fact_id).unwrap().key,
        "secret-key"
    );
    assert_eq!(store.list_facts(agent).unwrap().len(), 1);
    assert!(store
        .get_fact_by_key(agent, "secret-key")
        .unwrap()
        .is_some());

    // Raw on-disk bytes must be a sealed envelope, not JSON.
    let cf = store.db().cf_handle(cf::MEMORY_FACTS).unwrap();
    let raw = store
        .db()
        .iterator_cf(&cf, IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap()
        .1;
    assert!(SealCipher::is_sealed(&raw));
    assert!(!raw
        .windows(b"secret-value".len())
        .any(|w| w == b"secret-value"));
}

#[test]
fn sealed_event_and_procedure_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = sealed_store(dir.path());
    let agent = AgentId::generate();
    let now = Utc::now();

    store
        .put_event(&make_event(agent, "did a thing", now))
        .unwrap();
    assert_eq!(store.list_events(agent, 10).unwrap().len(), 1);
    assert_eq!(
        store
            .list_events_since(agent, now - chrono::Duration::hours(1))
            .unwrap()
            .len(),
        1
    );

    let proc = make_procedure(agent, "deploy");
    store.put_procedure(&proc).unwrap();
    assert_eq!(
        store.get_procedure(agent, proc.procedure_id).unwrap().name,
        "deploy"
    );
    assert_eq!(store.list_procedures(agent).unwrap().len(), 1);
}

/// Plaintext mode stays byte-for-byte the legacy JSON format.
#[test]
fn plaintext_fact_on_disk_format_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::new(test_db(dir.path()));
    let agent = AgentId::generate();
    let fact = make_fact(agent, "k", "v");

    store.put_fact(&fact).unwrap();

    let cf = store.db().cf_handle(cf::MEMORY_FACTS).unwrap();
    let raw = store
        .db()
        .iterator_cf(&cf, IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(raw.to_vec(), serde_json::to_vec(&fact).unwrap());
    assert!(!SealCipher::is_sealed(&raw));
}

#[test]
fn continuity_config_is_agent_scoped_persisted_and_sealed() {
    let dir = tempfile::tempdir().unwrap();
    let store = sealed_store(dir.path());
    let configured_agent = AgentId::generate();
    let untouched_agent = AgentId::generate();
    let config = AgentContinuityConfig {
        use_memory: false,
        generate_memory: true,
        write_policy: crate::types::MemoryWritePolicy::Approval,
        retrieval_mode: crate::types::MemoryRetrievalMode::QueryAware,
        allow_user_scope: false,
        allow_project_scope: false,
    };

    store
        .put_continuity_config(configured_agent, &config)
        .unwrap();

    assert_eq!(
        store.get_continuity_config(configured_agent).unwrap(),
        config
    );
    assert_eq!(
        store.get_continuity_config(untouched_agent).unwrap(),
        AgentContinuityConfig::default()
    );

    let cf = store.db().cf_handle(cf::MEMORY_CONFIG).unwrap();
    let raw = store
        .db()
        .get_cf(&cf, configured_agent.as_bytes())
        .unwrap()
        .unwrap();
    assert!(SealCipher::is_sealed(&raw));
    assert!(!raw.windows(b"approval".len()).any(|w| w == b"approval"));
}
