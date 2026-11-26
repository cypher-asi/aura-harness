//! Fact endpoints.

use crate::gateway::handlers::util::parse_agent_id;
use crate::gateway::RouterState;
use aura_context_memory::{
    Fact, FactSource, MemoryAccessContext, MemoryScope, MemoryStatus, MemoryStoreApi,
};
use aura_core_types::{AgentId, FactId};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use super::super::wire::{CreateFactBody, MemoryAccessParams, UpdateFactBody};
use super::{
    find_fact, memory_store, parse_fact_id, scoped_partitions, store_err, target_partition,
    ApiError, ApiResult,
};

pub(in crate::gateway) async fn list_facts(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<Vec<Fact>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut facts = Vec::new();
    for partition in scoped_partitions(agent_id, &params) {
        facts.extend(store.list_facts(partition).map_err(store_err)?);
    }
    facts.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(Json(facts))
}

pub(in crate::gateway) async fn get_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    find_fact(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        fact_id,
    )
    .map(Json)
}

pub(in crate::gateway) async fn get_fact_by_key(
    State(state): State<RouterState>,
    Path((agent_hex, key)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<serde_json::Value> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    for partition in scoped_partitions(agent_id, &params) {
        match store.get_fact_by_key(partition, &key) {
            Ok(Some(fact)) => {
                // Phase 5 (error-handling polish): the previous
                // `.unwrap_or_default()` quietly returned an empty
                // `serde_json::Value::Null` to the caller if `Fact`
                // serialization ever failed (e.g. a non-string map key
                // sneaks into a future schema). Surface the error as a
                // 500 so the misbehaviour is visible instead of looking
                // like a successful empty response.
                let value = serde_json::to_value(&fact).map_err(|e| {
                    tracing::error!(
                        error = %e,
                        agent_id = %agent_hex,
                        key,
                        "memory router: serialising Fact for get_fact_by_key failed"
                    );
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("failed to serialize fact: {e}"),
                        })),
                    )
                })?;
                return Ok(Json(value));
            }
            Ok(None) => {}
            Err(e) => return Err(store_err(e)),
        }
    }
    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "fact not found for key" })),
    ))
}

pub(in crate::gateway) async fn create_fact(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<CreateFactBody>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let access = params.access();
    let mut continuity = body.continuity.unwrap_or_default();
    continuity.provenance.contributor_agent_id = Some(agent_id.to_hex());
    let agent_id = target_partition(agent_id, &access, continuity.scope)?;
    continuity.provenance.project_id = access.project_id.clone();
    continuity.provenance.user_id = access.user_id.clone();
    let source = match body.source.as_deref() {
        Some("user_provided") => FactSource::UserProvided,
        Some("consolidated") => FactSource::Consolidated,
        _ => FactSource::Extracted,
    };
    if matches!(continuity.scope, MemoryScope::Project | MemoryScope::User)
        && store
            .get_fact_by_key(agent_id, &body.key)
            .map_err(store_err)?
            .is_some_and(|existing| existing.value != body.value)
    {
        continuity.status = MemoryStatus::Pending;
    }
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
        continuity,
    };
    store.put_fact(&fact).map_err(store_err)?;
    Ok(Json(fact))
}

pub(in crate::gateway) async fn update_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<UpdateFactBody>,
) -> ApiResult<Fact> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    let partitions = scoped_partitions(agent_id, &params);
    let mut fact = find_fact(store.as_ref(), &partitions, fact_id)?;
    let source_partition = fact.agent_id;
    apply_fact_patch(store.as_ref(), &mut fact, source_partition, body)?;
    let access = params.access();
    relocate_fact(
        store.as_ref(),
        &mut fact,
        source_partition,
        agent_id,
        &access,
    )?;
    Ok(Json(fact))
}

/// Apply the optional fields of `body` to `fact`. Activating a pending fact
/// supersedes the other active facts with the same key in `source_partition`.
fn apply_fact_patch(
    store: &dyn MemoryStoreApi,
    fact: &mut Fact,
    source_partition: AgentId,
    body: UpdateFactBody,
) -> Result<(), ApiError> {
    if let Some(key) = body.key {
        fact.key = key;
    }
    if let Some(value) = body.value {
        fact.value = value;
    }
    if let Some(confidence) = body.confidence {
        fact.confidence = confidence;
    }
    if let Some(importance) = body.importance {
        fact.importance = importance;
    }
    fact.updated_at = Utc::now();
    if let Some(continuity) = body.continuity {
        if fact.continuity.status == MemoryStatus::Pending
            && continuity.status == MemoryStatus::Active
        {
            supersede_active_facts(store, source_partition, fact)?;
        }
        fact.continuity = continuity;
    }
    if let Some(ref s) = body.source {
        fact.source = match s.as_str() {
            "user_provided" => FactSource::UserProvided,
            "consolidated" => FactSource::Consolidated,
            _ => FactSource::Extracted,
        };
    }
    Ok(())
}

fn supersede_active_facts(
    store: &dyn MemoryStoreApi,
    partition: AgentId,
    fact: &Fact,
) -> Result<(), ApiError> {
    for mut previous in store.list_facts(partition).map_err(store_err)? {
        if previous.fact_id != fact.fact_id
            && previous.key == fact.key
            && previous.continuity.status == MemoryStatus::Active
        {
            previous.continuity.status = MemoryStatus::Superseded;
            previous.continuity.superseded_by = Some(fact.fact_id.to_hex());
            previous.updated_at = Utc::now();
            store.put_fact(&previous).map_err(store_err)?;
        }
    }
    Ok(())
}

/// Stamp provenance, store `fact` in the partition its scope targets and
/// remove it from `source_partition` when it moved.
fn relocate_fact(
    store: &dyn MemoryStoreApi,
    fact: &mut Fact,
    source_partition: AgentId,
    agent_id: AgentId,
    access: &MemoryAccessContext,
) -> Result<(), ApiError> {
    let destination = target_partition(agent_id, access, fact.continuity.scope)?;
    fact.continuity
        .provenance
        .contributor_agent_id
        .get_or_insert_with(|| agent_id.to_hex());
    fact.continuity.provenance.project_id = access.project_id.clone();
    fact.continuity.provenance.user_id = access.user_id.clone();
    if destination != source_partition
        && matches!(
            fact.continuity.scope,
            MemoryScope::Project | MemoryScope::User
        )
        && store
            .get_fact_by_key(destination, &fact.key)
            .map_err(store_err)?
            .is_some_and(|existing| {
                existing.fact_id != fact.fact_id && existing.value != fact.value
            })
    {
        fact.continuity.status = MemoryStatus::Pending;
    }
    fact.agent_id = destination;
    store.put_fact(fact).map_err(store_err)?;
    if source_partition != destination {
        store
            .delete_fact(source_partition, fact.fact_id)
            .map_err(store_err)?;
    }
    Ok(())
}

pub(in crate::gateway) async fn delete_fact(
    State(state): State<RouterState>,
    Path((agent_hex, fact_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let fact_id = parse_fact_id(&fact_hex)?;
    let fact = find_fact(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        fact_id,
    )?;
    store
        .delete_fact(fact.agent_id, fact_id)
        .map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}
