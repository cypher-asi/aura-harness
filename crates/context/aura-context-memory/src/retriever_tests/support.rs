//! Shared fixtures for the retriever tests.

use crate::types::{AgentEvent, Fact, FactSource, MemoryContinuity, Procedure};
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::Utc;
use rocksdb::{ColumnFamilyDescriptor, DBWithThreadMode, MultiThreaded, Options};
use std::sync::Arc;

pub(super) fn test_db(dir: &std::path::Path) -> Arc<DBWithThreadMode<MultiThreaded>> {
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
        ColumnFamilyDescriptor::new("memory_config", Options::default()),
        ColumnFamilyDescriptor::new("agent_skills", Options::default()),
    ];
    Arc::new(DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, dir, cfs).unwrap())
}

pub(super) fn make_fact(
    agent_id: AgentId,
    key: &str,
    val: &str,
    importance: f32,
    confidence: f32,
) -> Fact {
    let now = Utc::now();
    Fact {
        fact_id: FactId::generate(),
        agent_id,
        key: key.to_string(),
        value: serde_json::Value::String(val.to_string()),
        confidence,
        source: FactSource::Extracted,
        importance,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        updated_at: now,
        continuity: MemoryContinuity::default(),
    }
}

pub(super) fn make_event(
    agent_id: AgentId,
    event_type: &str,
    summary: &str,
    ts: chrono::DateTime<Utc>,
) -> AgentEvent {
    AgentEvent {
        event_id: AgentEventId::generate(),
        agent_id,
        event_type: event_type.to_string(),
        summary: summary.to_string(),
        metadata: serde_json::Value::Null,
        importance: 0.6,
        access_count: 0,
        last_accessed: ts,
        timestamp: ts,
        continuity: MemoryContinuity::default(),
    }
}

pub(super) fn make_procedure(agent_id: AgentId, name: &str, steps: &[&str]) -> Procedure {
    let now = Utc::now();
    Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id,
        name: name.to_string(),
        trigger: "test trigger".to_string(),
        steps: steps.iter().map(|s| (*s).to_string()).collect(),
        context_constraints: serde_json::Value::Null,
        success_rate: 0.8,
        execution_count: 5,
        last_used: now,
        created_at: now,
        updated_at: now,
        skill_name: None,
        skill_relevance: None,
        continuity: MemoryContinuity::default(),
    }
}
