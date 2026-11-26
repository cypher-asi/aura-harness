//! Procedure endpoints.

use crate::gateway::handlers::util::parse_agent_id;
use crate::gateway::RouterState;
use aura_context_memory::{
    MemoryAccessContext, MemoryScope, MemoryStatus, MemoryStoreApi, Procedure,
};
use aura_core_types::{AgentId, ProcedureId};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use super::super::wire::{
    CreateProcedureBody, MemoryAccessParams, ProcedureListParams, UpdateProcedureBody,
};
use super::{
    find_procedure, memory_store, parse_procedure_id, scoped_partitions, store_err,
    target_partition, ApiError, ApiResult,
};

pub(in crate::gateway) async fn list_procedures(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<ProcedureListParams>,
) -> ApiResult<Vec<Procedure>> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let mut procs = Vec::new();
    for partition in scoped_partitions(agent_id, &params.access) {
        procs.extend(store.list_procedures(partition).map_err(store_err)?);
    }

    if let Some(ref skill) = params.skill {
        procs.retain(|p| p.skill_name.as_deref() == Some(skill.as_str()));
    }
    if let Some(min_rel) = params.min_relevance {
        procs.retain(|p| p.skill_relevance.unwrap_or(0.0) >= min_rel);
    }

    Ok(Json(procs))
}

pub(in crate::gateway) async fn get_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    find_procedure(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        proc_id,
    )
    .map(Json)
}

pub(in crate::gateway) async fn create_procedure(
    State(state): State<RouterState>,
    Path(agent_hex): Path<String>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<CreateProcedureBody>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let now = Utc::now();
    let access = params.access();
    let mut continuity = body.continuity.unwrap_or_default();
    continuity.provenance.contributor_agent_id = Some(agent_id.to_hex());
    let agent_id = target_partition(agent_id, &access, continuity.scope)?;
    continuity.provenance.project_id = access.project_id.clone();
    continuity.provenance.user_id = access.user_id.clone();
    if matches!(continuity.scope, MemoryScope::Project | MemoryScope::User)
        && store
            .list_procedures(agent_id)
            .map_err(store_err)?
            .into_iter()
            .any(|existing| {
                existing.name == body.name
                    && existing.continuity.status == MemoryStatus::Active
                    && (existing.trigger != body.trigger || existing.steps != body.steps)
            })
    {
        continuity.status = MemoryStatus::Pending;
    }
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
        continuity,
    };
    store.put_procedure(&proc).map_err(store_err)?;
    Ok(Json(proc))
}

pub(in crate::gateway) async fn update_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
    Json(body): Json<UpdateProcedureBody>,
) -> ApiResult<Procedure> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    let mut proc = find_procedure(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        proc_id,
    )?;
    let source_partition = proc.agent_id;
    apply_procedure_patch(store.as_ref(), &mut proc, source_partition, body)?;
    let access = params.access();
    relocate_procedure(
        store.as_ref(),
        &mut proc,
        source_partition,
        agent_id,
        &access,
    )?;
    Ok(Json(proc))
}

/// Apply the optional fields of `body` to `proc`. Activating a pending
/// procedure supersedes the other active procedures with the same name in
/// `source_partition`.
fn apply_procedure_patch(
    store: &dyn MemoryStoreApi,
    proc: &mut Procedure,
    source_partition: AgentId,
    body: UpdateProcedureBody,
) -> Result<(), ApiError> {
    if let Some(name) = body.name {
        proc.name = name;
    }
    if let Some(trigger) = body.trigger {
        proc.trigger = trigger;
    }
    if let Some(steps) = body.steps {
        proc.steps = steps;
    }
    if let Some(context_constraints) = body.context_constraints {
        proc.context_constraints = context_constraints;
    }
    if body.skill_name.is_some() || body.skill_relevance.is_some() {
        proc.skill_name = body.skill_name;
        proc.skill_relevance = body.skill_relevance;
    }
    if let Some(success_rate) = body.success_rate {
        proc.success_rate = success_rate;
    }
    proc.updated_at = Utc::now();
    if let Some(continuity) = body.continuity {
        if proc.continuity.status == MemoryStatus::Pending
            && continuity.status == MemoryStatus::Active
        {
            supersede_active_procedures(store, source_partition, proc)?;
        }
        proc.continuity = continuity;
    }
    Ok(())
}

fn supersede_active_procedures(
    store: &dyn MemoryStoreApi,
    partition: AgentId,
    proc: &Procedure,
) -> Result<(), ApiError> {
    for mut previous in store.list_procedures(partition).map_err(store_err)? {
        if previous.procedure_id != proc.procedure_id
            && previous.name == proc.name
            && previous.continuity.status == MemoryStatus::Active
        {
            previous.continuity.status = MemoryStatus::Superseded;
            previous.continuity.superseded_by = Some(proc.procedure_id.to_hex());
            previous.updated_at = Utc::now();
            store.put_procedure(&previous).map_err(store_err)?;
        }
    }
    Ok(())
}

/// Stamp provenance, store `proc` in the partition its scope targets and
/// remove it from `source_partition` when it moved.
fn relocate_procedure(
    store: &dyn MemoryStoreApi,
    proc: &mut Procedure,
    source_partition: AgentId,
    agent_id: AgentId,
    access: &MemoryAccessContext,
) -> Result<(), ApiError> {
    let destination = target_partition(agent_id, access, proc.continuity.scope)?;
    proc.continuity
        .provenance
        .contributor_agent_id
        .get_or_insert_with(|| agent_id.to_hex());
    proc.continuity.provenance.project_id = access.project_id.clone();
    proc.continuity.provenance.user_id = access.user_id.clone();
    if destination != source_partition
        && matches!(
            proc.continuity.scope,
            MemoryScope::Project | MemoryScope::User
        )
        && store
            .list_procedures(destination)
            .map_err(store_err)?
            .into_iter()
            .any(|existing| {
                existing.procedure_id != proc.procedure_id
                    && existing.name == proc.name
                    && existing.continuity.status == MemoryStatus::Active
                    && (existing.trigger != proc.trigger || existing.steps != proc.steps)
            })
    {
        proc.continuity.status = MemoryStatus::Pending;
    }
    proc.agent_id = destination;
    store.put_procedure(proc).map_err(store_err)?;
    if source_partition != destination {
        store
            .delete_procedure(source_partition, proc.procedure_id)
            .map_err(store_err)?;
    }
    Ok(())
}

pub(in crate::gateway) async fn delete_procedure(
    State(state): State<RouterState>,
    Path((agent_hex, proc_hex)): Path<(String, String)>,
    Query(params): Query<MemoryAccessParams>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let store = memory_store(&state)?;
    let agent_id = parse_agent_id(&agent_hex)?;
    let proc_id = parse_procedure_id(&proc_hex)?;
    let procedure = find_procedure(
        store.as_ref(),
        &scoped_partitions(agent_id, &params),
        proc_id,
    )?;
    store
        .delete_procedure(procedure.agent_id, proc_id)
        .map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}
