use super::*;
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::Utc;

#[test]
fn readable_partitions_keep_narrow_to_broad_precedence() {
    let agent_id = AgentId::generate();
    let access = MemoryAccessContext {
        project_id: Some("project-1".to_string()),
        user_id: Some("user-1".to_string()),
        include_legacy: true,
    };

    assert_eq!(
        access.readable_partitions(agent_id, true, true),
        vec![
            access.storage_id(agent_id, MemoryScope::Agent),
            access.storage_id(agent_id, MemoryScope::Project),
            access.storage_id(agent_id, MemoryScope::User),
            agent_id,
        ]
    );
}

fn make_fact(key: &str, val: &str) -> Fact {
    let now = Utc::now();
    Fact {
        fact_id: FactId::generate(),
        agent_id: AgentId::generate(),
        key: key.to_string(),
        value: serde_json::Value::String(val.to_string()),
        confidence: 0.9,
        source: FactSource::Extracted,
        importance: 0.5,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        updated_at: now,
        continuity: MemoryContinuity::default(),
    }
}

fn make_event(event_type: &str, summary: &str) -> AgentEvent {
    let now = Utc::now();
    AgentEvent {
        event_id: AgentEventId::generate(),
        agent_id: AgentId::generate(),
        event_type: event_type.to_string(),
        summary: summary.to_string(),
        metadata: serde_json::Value::Null,
        importance: 0.6,
        access_count: 0,
        last_accessed: now,
        timestamp: now,
        continuity: MemoryContinuity::default(),
    }
}

fn make_procedure(name: &str, steps: Vec<&str>) -> Procedure {
    let now = Utc::now();
    Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id: AgentId::generate(),
        name: name.to_string(),
        trigger: "test".to_string(),
        steps: steps.into_iter().map(String::from).collect(),
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

#[test]
fn is_empty_default() {
    let p = MemoryPacket::default();
    assert!(p.is_empty());
}

#[test]
fn is_empty_facts_only() {
    let p = MemoryPacket {
        facts: vec![make_fact("k", "v")],
        ..Default::default()
    };
    assert!(!p.is_empty());
}

#[test]
fn is_empty_events_only() {
    let p = MemoryPacket {
        events: vec![make_event("t", "s")],
        ..Default::default()
    };
    assert!(!p.is_empty());
}

#[test]
fn is_empty_procedures_only() {
    let p = MemoryPacket {
        procedures: vec![make_procedure("p", vec!["a", "b"])],
        ..Default::default()
    };
    assert!(!p.is_empty());
}

#[test]
fn format_for_prompt_empty() {
    let p = MemoryPacket::default();
    assert_eq!(p.format_for_prompt(), "");
}

#[test]
fn format_for_prompt_facts_only() {
    let p = MemoryPacket {
        facts: vec![make_fact("lang", "Rust")],
        ..Default::default()
    };
    let out = p.format_for_prompt();
    assert!(out.contains("<agent_memory>"));
    assert!(out.contains("<facts>"));
    assert!(out.contains("lang: Rust"));
    assert!(out.contains("confidence: 0.90"));
    assert!(!out.contains("<recent_events>"));
    assert!(!out.contains("<procedures>"));
}

#[test]
fn format_for_prompt_non_string_value() {
    let now = Utc::now();
    let fact = Fact {
        fact_id: FactId::generate(),
        agent_id: AgentId::generate(),
        key: "count".to_string(),
        value: serde_json::json!(42),
        confidence: 0.8,
        source: FactSource::Extracted,
        importance: 0.5,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        updated_at: now,
        continuity: MemoryContinuity::default(),
    };
    let p = MemoryPacket {
        facts: vec![fact],
        ..Default::default()
    };
    let out = p.format_for_prompt();
    assert!(out.contains("count: 42"));
}

#[test]
fn format_for_prompt_mixed() {
    let p = MemoryPacket {
        facts: vec![make_fact("k", "v")],
        events: vec![make_event("task_run", "did stuff")],
        procedures: vec![make_procedure("deploy", vec!["build", "push"])],
        trace: None,
    };
    let out = p.format_for_prompt();
    assert!(out.contains("<facts>"));
    assert!(out.contains("<recent_events>"));
    assert!(out.contains("<procedures>"));
    assert!(out.contains("build -> push"));
}

#[test]
fn fact_source_display() {
    assert_eq!(FactSource::Extracted.to_string(), "extracted");
    assert_eq!(FactSource::UserProvided.to_string(), "user_provided");
    assert_eq!(FactSource::Consolidated.to_string(), "consolidated");
}

#[test]
fn fact_serde_roundtrip() {
    let f = make_fact("key", "value");
    let json = serde_json::to_string(&f).unwrap();
    let parsed: Fact = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.key, "key");
}

#[test]
fn event_serde_roundtrip() {
    let e = make_event("task_run", "summary");
    let json = serde_json::to_string(&e).unwrap();
    let parsed: AgentEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_type, "task_run");
}

#[test]
fn procedure_serde_roundtrip() {
    let p = make_procedure("deploy", vec!["build", "push"]);
    let json = serde_json::to_string(&p).unwrap();
    let parsed: Procedure = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "deploy");
    assert_eq!(parsed.steps.len(), 2);
}
