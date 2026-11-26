//! Aggregate endpoints: snapshot, wipe, stats, Agent Continuity config,
//! retrieval trace and consolidation.

use aura_context_memory::MemoryScope;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::gateway::handlers::util::parse_agent_id;
use crate::gateway::RouterState;

use super::super::wire::MemoryAccessParams;
use super::{memory_store, scoped_partitions, store_err, ApiResult};

pub(in crate::gateway) async fn snapshot(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<aura_context_memory::MemoryPacket> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut facts = Vec::new();
    let mut events = Vec::new();
    let mut procedures = Vec::new();
    for partition in scoped_partitions(agent_id, &params) {
        facts.extend(store.list_facts(partition).map_err(store_err)?);
        events.extend(store.list_events(partition, 1000).map_err(store_err)?);
        procedures.extend(store.list_procedures(partition).map_err(store_err)?);
    }
    facts.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    events.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
    procedures.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(Json(aura_context_memory::MemoryPacket {
        facts,
        events,
        procedures,
        trace: None,
    }))
}

pub(in crate::gateway) async fn wipe(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let access = params.access();
    let partitions = if params.scope.is_some() {
        scoped_partitions(agent_id, &params)
    } else if access.project_id.is_some() {
        vec![access.storage_id(agent_id, MemoryScope::Agent)]
    } else {
        vec![agent_id]
    };
    for partition in partitions {
        store.delete_all(partition).map_err(store_err)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::gateway) async fn stats(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<aura_context_memory::MemoryStats> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut combined = aura_context_memory::MemoryStats {
        facts: 0,
        events: 0,
        procedures: 0,
    };
    for partition in scoped_partitions(agent_id, &params) {
        let stats = store.stats(partition).map_err(store_err)?;
        combined.facts += stats.facts;
        combined.events += stats.events;
        combined.procedures += stats.procedures;
    }
    Ok(Json(combined))
}

// ============================================================================
// Agent Continuity controls and evidence
// ============================================================================

pub(in crate::gateway) async fn get_continuity_config(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
) -> ApiResult<aura_context_memory::AgentContinuityConfig> {
    let agent_id = parse_agent_id(&agent_hex)?;
    let manager = state.memory_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "memory system not configured" })),
        )
    })?;
    manager
        .continuity_config(agent_id)
        .await
        .map(Json)
        .map_err(store_err)
}

pub(in crate::gateway) async fn update_continuity_config(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Json(config): Json<aura_context_memory::AgentContinuityConfig>,
) -> ApiResult<aura_context_memory::AgentContinuityConfig> {
    let agent_id = parse_agent_id(&agent_hex)?;
    let manager = state.memory_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "memory system not configured" })),
        )
    })?;
    manager
        .save_continuity_config(agent_id, config)
        .await
        .map(Json)
        .map_err(store_err)
}

pub(in crate::gateway) async fn latest_retrieval_trace(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<serde_json::Value> {
    let agent_id = parse_agent_id(&agent_hex)?;
    let manager = state.memory_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "memory system not configured" })),
        )
    })?;
    let partition_id = params.access().storage_id(agent_id, MemoryScope::Agent);
    Ok(Json(match manager.latest_retrieval_trace(partition_id) {
        Some(trace) => serde_json::to_value(trace).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
        })?,
        None => serde_json::Value::Null,
    }))
}

// ============================================================================
// Consolidation
// ============================================================================

pub(in crate::gateway) async fn consolidate(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<aura_context_memory::ConsolidationReport> {
    let agent_id = parse_agent_id(&agent_hex)?;
    let mm = state.memory_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "memory system not configured" })),
        )
    })?;
    let mut combined = aura_context_memory::ConsolidationReport::default();
    for partition in scoped_partitions(agent_id, &params) {
        let report = mm.consolidate(partition).await.map_err(store_err)?;
        combined.facts_merged += report.facts_merged;
        combined.facts_evolved += report.facts_evolved;
        combined.events_compressed += report.events_compressed;
        combined.events_deleted += report.events_deleted;
        combined.facts_forgotten += report.facts_forgotten;
        combined.procedures_forgotten += report.procedures_forgotten;
        combined.insights_created += report.insights_created;
    }
    Ok(Json(combined))
}
