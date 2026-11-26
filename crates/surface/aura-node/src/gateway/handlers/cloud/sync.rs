//! Hub-facing sync routes: host stats, record import, seal, traces.

use super::*;

/// `GET /v1/host`.
pub(in crate::gateway) async fn host_handler(
    State(state): State<RouterState>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(state.cloud.host_stats().await))
}

/// `POST /v1/sync/import`: append verified entries verbatim (the node
/// side of the record hand-off; a declared exception to kernel-only
/// appends, see `docs/invariants.md`).
pub(in crate::gateway) async fn sync_import_handler(
    State(state): State<RouterState>,
    Json(import): Json<SyncImport>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let agent = parse_agent(&import.agent_id)?;
    if state.cloud.is_sealed(&agent) {
        return Err((
            StatusCode::CONFLICT,
            format!("agent {agent} is sealed on this node"),
        ));
    }
    let storage = |e: aura_store_db::StoreError| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("storage error: {e}"),
        )
    };
    let head = state.store.get_head_seq(agent).map_err(storage)?;
    let head_hash = if head == 0 {
        None
    } else {
        Some(
            state
                .store
                .get_record_entry(agent, head)
                .map_err(storage)?
                .tx
                .hash,
        )
    };
    let fresh = verify_page(agent, head, head_hash, &import.entries)
        .map_err(|err| (StatusCode::CONFLICT, err.to_string()))?;
    if !fresh.is_empty() {
        state
            .store
            .append_replica_batch(agent, head + 1, fresh)
            .map_err(storage)?;
    }
    let head_seq = state.store.get_head_seq(agent).map_err(storage)?;
    Ok(Json(SyncImported { head_seq }))
}

/// `POST /v1/sync/seal/:agent_id`.
pub(in crate::gateway) async fn sync_seal_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let agent = parse_agent(&agent_hex)?;
    state.cloud.seal(agent);
    // New turns are refused from now on by the sealed check on `/v1/run`
    // and `/tx`. Stop what is already running: the agent's spawned
    // children, and every chat run (a cloud node only runs its hub's
    // agent), then wait for them to end so the head is final.
    let _ = state
        .agent_host
        .lifecycle(agent, aura_engine::agents::LifecycleAction::Stop);
    for run in state.chat_runs.iter() {
        run.shutdown.cancel();
    }
    let deadline = tokio::time::Instant::now() + SEAL_DRAIN;
    while !state.chat_runs.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A turn the scheduler runs (mail) holds the agent's claim; waiting
    // for it means no write is in flight when the head is read.
    let claim = tokio::time::timeout(SEAL_DRAIN, state.scheduler.processing_claim(agent))
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                format!("agent {agent} is still writing; try the seal again"),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("claim: {e}")))?;
    let head_seq = state.store.get_head_seq(agent).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("storage error: {e}"),
        )
    })?;
    drop(claim);
    Ok(Json(SealResponse { head_seq }))
}

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct SinceQuery {
    #[serde(default)]
    since: u64,
}

/// `GET /v1/sync/traces?since=`: trace runs are mirrored by the hub from
/// the agents' records today, so this page is empty; the cursor is kept
/// so the hub's loop needs no change when runs are served here.
pub(in crate::gateway) async fn sync_traces_handler(
    State(state): State<RouterState>,
    Query(query): Query<SinceQuery>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(TracePage {
        runs: Vec::new(),
        next_since: query.since,
    }))
}
