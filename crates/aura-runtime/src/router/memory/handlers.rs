//! HTTP handlers for the memory CRUD surface.
//!
//! Wire types (`CreateFactBody`, `CreateEventBody`, …) live in
//! [`super::wire`]; the handlers here convert those into the
//! `aura_memory` domain types and dispatch to
//! [`aura_memory::MemoryStoreApi`].

use aura_core::{AgentEventId, FactId, ProcedureId};
use aura_memory::{AgentEvent, Fact, FactSource, MemoryStoreApi, Procedure};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use crate::router::ids::parse_agent_id;
use crate::router::RouterState;

use super::wire::{
    BulkDeleteEventsBody, CreateEventBody, CreateFactBody, CreateProcedureBody,
    ProcedureListParams, UpdateProcedureBody,
};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<serde_json::Value>)>;

fn parse_fact_id(hex: &str) -> Result<FactId, (StatusCode, Json<serde_json::Value>)> {
    FactId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid fact_id: {e}") })),
        )
    })
}

fn parse_event_id(hex: &str) -> Result<AgentEventId, (StatusCode, Json<serde_json::Value>)> {
    AgentEventId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid event_id: {e}") })),
        )
    })
}

fn parse_procedure_id(hex: &str) -> Result<ProcedureId, (StatusCode, Json<serde_json::Value>)> {
    ProcedureId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid procedure_id: {e}") })),
        )
    })
}

fn memory_store(
    state: &RouterState,
) -> Result<&std::sync::Arc<dyn MemoryStoreApi>, (StatusCode, Json<serde_json::Value>)> {
    state
        .memory_manager
        .as_ref()
        .map(|mm| mm.store())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "memory system not configured" })),
            )
        })
}

fn store_err(e: aura_memory::MemoryError) -> (StatusCode, Json<serde_json::Value>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(serde_json::json!({ "error": msg })))
}

// ============================================================================
// Facts
// ============================================================================

pub(in crate::router) async fn list_facts(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<Vec<Fact>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    store.list_facts(agent_id).map(Json).map_err(store_err)
}

pub(in crate::router) async fn get_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    store
        .get_fact(agent_id, fact_id)
        .map(Json)
        .map_err(store_err)
}

pub(in crate::router) async fn get_fact_by_key(
    State(state): State<RouterState>,
    Path((agent_hex, key)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    match store.get_fact_by_key(agent_id, &key) {
        Ok(Some(fact)) => Ok(Json(serde_json::to_value(fact).unwrap_or_default())),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "fact not found for key" })),
        )),
        Err(e) => Err(store_err(e)),
    }
}

pub(in crate::router) async fn create_fact(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Json(body): Json<CreateFactBody>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let source = match body.source.as_deref() {
        Some("user_provided") => FactSource::UserProvided,
        Some("consolidated") => FactSource::Consolidated,
        _ => FactSource::Extracted,
    };
    let fact = Fact {
        fact_id: FactId::generate(),
        agent_id,
        key: body.key,
        value: body.value,
        confidence: body.confidence,
        source,
        importance: body.importance,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        updated_at: now,
    };
    store.put_fact(&fact).map_err(store_err)?;
    Ok(Json(fact))
}

pub(in crate::router) async fn update_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
    Json(body): Json<CreateFactBody>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    let mut fact = store.get_fact(agent_id, fact_id).map_err(store_err)?;
    fact.key = body.key;
    fact.value = body.value;
    fact.confidence = body.confidence;
    fact.importance = body.importance;
    fact.updated_at = Utc::now();
    if let Some(ref s) = body.source {
        fact.source = match s.as_str() {
            "user_provided" => FactSource::UserProvided,
            "consolidated" => FactSource::Consolidated,
            _ => FactSource::Extracted,
        };
    }
    store.put_fact(&fact).map_err(store_err)?;
    Ok(Json(fact))
}

pub(in crate::router) async fn delete_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    store.delete_fact(agent_id, fact_id).map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Events
// ============================================================================

pub(in crate::router) async fn list_events(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<Vec<AgentEvent>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    store
        .list_events(agent_id, 1000)
        .map(Json)
        .map_err(store_err)
}

pub(in crate::router) async fn create_event(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Json(body): Json<CreateEventBody>,
) -> ApiResult<AgentEvent> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let event = AgentEvent {
        event_id: AgentEventId::generate(),
        agent_id,
        event_type: body.event_type,
        summary: body.summary,
        metadata: body.metadata,
        importance: body.importance,
        access_count: 0,
        last_accessed: now,
        timestamp: now,
    };
    store.put_event(&event).map_err(store_err)?;
    Ok(Json(event))
}

pub(in crate::router) async fn delete_event(
    State(state): State<RouterState>,
    Path((agent_hex, event_hex)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let event_id = parse_event_id(&event_hex)?;
    store.delete_event(agent_id, event_id).map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::router) async fn bulk_delete_events(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Json(body): Json<BulkDeleteEventsBody>,
) -> ApiResult<serde_json::Value> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let deleted = store
        .delete_events_before(agent_id, body.before)
        .map_err(store_err)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

// ============================================================================
// Procedures
// ============================================================================

pub(in crate::router) async fn list_procedures(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<ProcedureListParams>,
) -> ApiResult<Vec<Procedure>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut procs = store.list_procedures(agent_id).map_err(store_err)?;

    if let Some(ref skill) = params.skill {
        procs.retain(|p| p.skill_name.as_deref() == Some(skill.as_str()));
    }
    if let Some(min_rel) = params.min_relevance {
        procs.retain(|p| p.skill_relevance.unwrap_or(0.0) >= min_rel);
    }

    Ok(Json(procs))
}

pub(in crate::router) async fn get_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    store
        .get_procedure(agent_id, proc_id)
        .map(Json)
        .map_err(store_err)
}

pub(in crate::router) async fn create_procedure(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Json(body): Json<CreateProcedureBody>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let proc = Procedure {
        procedure_id: ProcedureId::generate(),
        agent_id,
        name: body.name,
        trigger: body.trigger,
        steps: body.steps,
        context_constraints: body.context_constraints,
        success_rate: 0.0,
        execution_count: 0,
        last_used: now,
        created_at: now,
        updated_at: now,
        skill_name: body.skill_name,
        skill_relevance: body.skill_relevance,
    };
    store.put_procedure(&proc).map_err(store_err)?;
    Ok(Json(proc))
}

pub(in crate::router) async fn update_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
    Json(body): Json<UpdateProcedureBody>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    let mut proc = store.get_procedure(agent_id, proc_id).map_err(store_err)?;
    proc.name = body.name;
    proc.trigger = body.trigger;
    proc.steps = body.steps;
    proc.context_constraints = body.context_constraints;
    if body.skill_name.is_some() || body.skill_relevance.is_some() {
        proc.skill_name = body.skill_name;
        proc.skill_relevance = body.skill_relevance;
    }
    proc.updated_at = Utc::now();
    store.put_procedure(&proc).map_err(store_err)?;
    Ok(Json(proc))
}

pub(in crate::router) async fn delete_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    store
        .delete_procedure(agent_id, proc_id)
        .map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Aggregates
// ============================================================================

pub(in crate::router) async fn snapshot(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<aura_memory::MemoryPacket> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let facts = store.list_facts(agent_id).map_err(store_err)?;
    let events = store.list_events(agent_id, 1000).map_err(store_err)?;
    let procedures = store.list_procedures(agent_id).map_err(store_err)?;
    Ok(Json(aura_memory::MemoryPacket {
        facts,
        events,
        procedures,
    }))
}

pub(in crate::router) async fn wipe(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    store.delete_all(agent_id).map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::router) async fn stats(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<aura_memory::MemoryStats> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    store.stats(agent_id).map(Json).map_err(store_err)
}

// ============================================================================
// Consolidation
// ============================================================================

pub(in crate::router) async fn consolidate(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<aura_memory::ConsolidationReport> {
    let agent_id = parse_agent_id(&agent_hex)?;
    let mm = state.memory_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "memory system not configured" })),
        )
    })?;
    mm.consolidate(agent_id).await.map(Json).map_err(store_err)
}
