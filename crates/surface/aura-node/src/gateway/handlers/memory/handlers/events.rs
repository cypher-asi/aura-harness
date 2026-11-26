//! Event endpoints.

use crate::gateway::handlers::util::parse_agent_id;
use crate::gateway::RouterState;
use aura_context_memory::AgentEvent;
use aura_core_types::AgentEventId;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use super::super::wire::{BulkDeleteEventsBody, CreateEventBody, MemoryAccessParams};
use super::{
    find_event, memory_store, parse_event_id, scoped_partitions, store_err, target_partition,
    ApiResult,
};

pub(in crate::gateway) async fn list_events(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<Vec<AgentEvent>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut events = Vec::new();
    for partition in scoped_partitions(agent_id, &params) {
        events.extend(store.list_events(partition, 1000).map_err(store_err)?);
    }
    events.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
    events.truncate(1000);
    Ok(Json(events))
}

pub(in crate::gateway) async fn create_event(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<CreateEventBody>,
) -> ApiResult<AgentEvent> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let access = params.access();
    let mut continuity = body.continuity.unwrap_or_default();
    continuity.provenance.contributor_agent_id = Some(agent_id.to_hex());
    let agent_id = target_partition(agent_id, &access, continuity.scope)?;
    continuity.provenance.project_id = access.project_id.clone();
    continuity.provenance.user_id = access.user_id.clone();
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
        continuity,
    };
    store.put_event(&event).map_err(store_err)?;
    Ok(Json(event))
}

pub(in crate::gateway) async fn delete_event(
    State(state): State<RouterState>,
    Path((agent_hex, event_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let event_id = parse_event_id(&event_hex)?;
    let event = find_event(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        event_id,
    )?;
    store
        .delete_event(event.agent_id, event_id)
        .map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::gateway) async fn bulk_delete_events(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<BulkDeleteEventsBody>,
) -> ApiResult<serde_json::Value> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut deleted = 0;
    for partition in scoped_partitions(agent_id, &params) {
        deleted += store
            .delete_events_before(partition, body.before)
            .map_err(store_err)?;
    }
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
