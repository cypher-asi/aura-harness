//! HTTP handlers for the memory CRUD surface.
//!
//! Wire types (`CreateFactBody`, `CreateEventBody`, …) live in
//! [`super::wire`]; the handlers convert those into the
//! `aura_context_memory` domain types and dispatch to
//! [`aura_context_memory::MemoryStoreApi`].
//!
//! This module holds the shared `ApiResult` / id-parsing / partition
//! helpers; the handlers live in one file per domain: [`facts`],
//! [`events`], [`procedures`] and [`aggregates`] (snapshot, stats,
//! continuity config, retrieval trace and consolidation).

mod aggregates;
mod events;
mod facts;
mod procedures;

pub(in crate::gateway) use aggregates::{
    consolidate, get_continuity_config, latest_retrieval_trace, snapshot, stats,
    update_continuity_config, wipe,
};
pub(in crate::gateway) use events::{bulk_delete_events, create_event, delete_event, list_events};
pub(in crate::gateway) use facts::{
    create_fact, delete_fact, get_fact, get_fact_by_key, list_facts, update_fact,
};
pub(in crate::gateway) use procedures::{
    create_procedure, delete_procedure, get_procedure, list_procedures, update_procedure,
};

use aura_context_memory::{
    AgentEvent, Fact, MemoryAccessContext, MemoryScope, MemoryStoreApi, Procedure,
};
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use axum::{http::StatusCode, Json};

use crate::gateway::RouterState;

use super::wire::MemoryAccessParams;

/// Error half of every memory handler result: status + `{ "error": ... }`.
pub(super) type ApiError = (StatusCode, Json<serde_json::Value>);
pub(super) type ApiResult<T> = Result<Json<T>, ApiError>;

pub(super) fn parse_fact_id(hex: &str) -> Result<FactId, (StatusCode, Json<serde_json::Value>)> {
    FactId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid fact_id: {e}") })),
        )
    })
}

pub(super) fn parse_event_id(
    hex: &str,
) -> Result<AgentEventId, (StatusCode, Json<serde_json::Value>)> {
    AgentEventId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid event_id: {e}") })),
        )
    })
}

pub(super) fn parse_procedure_id(
    hex: &str,
) -> Result<ProcedureId, (StatusCode, Json<serde_json::Value>)> {
    ProcedureId::from_hex(hex).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid procedure_id: {e}") })),
        )
    })
}

pub(super) fn memory_store(
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

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` callback, which passes the error by value"
)]
pub(super) fn store_err(
    e: aura_context_memory::MemoryError,
) -> (StatusCode, Json<serde_json::Value>) {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(serde_json::json!({ "error": msg })))
}

fn bad_request(message: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message.into() })),
    )
}

pub(super) fn scoped_partitions(agent_id: AgentId, params: &MemoryAccessParams) -> Vec<AgentId> {
    let access = params.access();
    if let Some(scope) = params.scope {
        match scope {
            MemoryScope::Agent if access.project_id.is_none() => vec![agent_id],
            MemoryScope::Project if access.project_id.is_none() => Vec::new(),
            MemoryScope::User if access.user_id.is_none() => Vec::new(),
            _ => vec![access.storage_id(agent_id, scope)],
        }
    } else {
        access.readable_partitions(agent_id, true, true)
    }
}

pub(super) fn target_partition(
    agent_id: AgentId,
    access: &MemoryAccessContext,
    scope: MemoryScope,
) -> Result<AgentId, (StatusCode, Json<serde_json::Value>)> {
    match scope {
        MemoryScope::Project if access.project_id.is_none() => Err(bad_request(
            "project_id is required for project-scoped memory",
        )),
        MemoryScope::User if access.user_id.is_none() => {
            Err(bad_request("user_id is required for personal memory"))
        }
        _ => Ok(
            if access.project_id.is_none() && scope == MemoryScope::Agent {
                // Preserve the v1 API contract for project-less callers.
                agent_id
            } else {
                access.storage_id(agent_id, scope)
            },
        ),
    }
}

pub(super) fn find_fact(
    store: &dyn MemoryStoreApi,
    partitions: &[AgentId],
    fact_id: FactId,
) -> Result<Fact, (StatusCode, Json<serde_json::Value>)> {
    for partition in partitions {
        match store.get_fact(*partition, fact_id) {
            Ok(fact) => return Ok(fact),
            Err(error) if error.to_string().contains("not found") => {}
            Err(error) => return Err(store_err(error)),
        }
    }
    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "fact not found" })),
    ))
}

pub(super) fn find_procedure(
    store: &dyn MemoryStoreApi,
    partitions: &[AgentId],
    procedure_id: ProcedureId,
) -> Result<Procedure, (StatusCode, Json<serde_json::Value>)> {
    for partition in partitions {
        match store.get_procedure(*partition, procedure_id) {
            Ok(procedure) => return Ok(procedure),
            Err(error) if error.to_string().contains("not found") => {}
            Err(error) => return Err(store_err(error)),
        }
    }
    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "procedure not found" })),
    ))
}

pub(super) fn find_event(
    store: &dyn MemoryStoreApi,
    partitions: &[AgentId],
    event_id: AgentEventId,
) -> Result<AgentEvent, (StatusCode, Json<serde_json::Value>)> {
    for partition in partitions {
        let events = store.list_events(*partition, 10_000).map_err(store_err)?;
        if let Some(event) = events.into_iter().find(|event| event.event_id == event_id) {
            return Ok(event);
        }
    }
    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "event not found" })),
    ))
}
