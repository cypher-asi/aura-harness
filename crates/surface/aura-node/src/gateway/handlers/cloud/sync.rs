//! Hub-facing sync routes: host stats, record import, seal, session
//! checkpoints, traces.

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
        return Err(sealed(agent));
    }
    let head = state.store.get_head_seq(agent).map_err(|e| storage(&e))?;
    let head_hash = if head == 0 {
        None
    } else {
        Some(
            state
                .store
                .get_record_entry(agent, head)
                .map_err(|e| storage(&e))?
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
            .map_err(|e| storage(&e))?;
    }
    state.cloud.host(agent);
    let head_seq = state.store.get_head_seq(agent).map_err(|e| storage(&e))?;
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
    end_chat_runs(&state).await;
    // A turn the scheduler runs (mail) holds the agent's claim; waiting
    // for it means no write is in flight when the head is read.
    let claim = hold_claim(&state, agent).await?;
    let head_seq = state.store.get_head_seq(agent).map_err(|e| storage(&e))?;
    drop(claim);
    // Its run is complete now; finishing it gives the hub's last copy
    // the scorecard.
    end_trace(&state, agent).await;
    Ok(Json(SealResponse { head_seq }))
}

/// Finish `agent`'s conversation run (file writes, off the runtime).
async fn end_trace(state: &RouterState, agent: AgentId) {
    let cloud = Arc::clone(&state.cloud);
    let _ = tokio::task::spawn_blocking(move || cloud.end_trace(agent)).await;
}

/// `GET /v1/sync/session/:agent_id`: the agent's current session
/// checkpoint here, or `null`; the hub keeps a copy in case the machine
/// is lost. Served after a seal too, for Bring back.
pub(in crate::gateway) async fn sync_session_get_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
) -> ApiResult<impl IntoResponse> {
    let Some(sessions) = state.cloud.sessions().cloned() else {
        return Err(not_cloud());
    };
    let agent = parse_agent(&agent_hex)?;
    let checkpoint = tokio::task::spawn_blocking(move || sessions.load_current(agent))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not read the checkpoint: {e}"),
            )
        })?;
    Ok(Json(checkpoint))
}

/// `PUT /v1/sync/session/:agent_id`: make `checkpoint` the agent's current
/// session here, so its first run continues the conversation it had,
/// tool calls included. The checkpoint must not be past the record the
/// node holds for the agent.
pub(in crate::gateway) async fn sync_session_put_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
    Json(checkpoint): Json<SessionCheckpoint>,
) -> ApiResult<impl IntoResponse> {
    let Some(sessions) = state.cloud.sessions() else {
        return Err(not_cloud());
    };
    let agent = parse_agent(&agent_hex)?;
    if state.cloud.is_sealed(&agent) {
        return Err(sealed(agent));
    }
    if checkpoint.version != CHECKPOINT_VERSION {
        return Err(bad(format!(
            "unknown checkpoint version {}",
            checkpoint.version
        )));
    }
    let head = state.store.get_head_seq(agent).map_err(|e| storage(&e))?;
    if checkpoint.record_seq > head {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "the checkpoint is at entry {} but the record here ends at {head}",
                checkpoint.record_seq
            ),
        ));
    }
    sessions.save(agent, &checkpoint).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save the checkpoint: {e}"),
        )
    })?;
    Ok(Json(SessionSaved {
        record_seq: checkpoint.record_seq,
        messages: u64::try_from(checkpoint.messages.len()).unwrap_or(u64::MAX),
    }))
}

/// `POST /v1/sync/session/:agent_id/new`: start a new session in the
/// agent's record, as `/new` does for a local agent. Live chat runs hold
/// the old conversation, so they end first; the next run finds the
/// `SessionStart` and starts empty.
pub(in crate::gateway) async fn sync_session_new_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let agent = parse_agent(&agent_hex)?;
    if state.cloud.is_sealed(&agent) {
        return Err(sealed(agent));
    }
    end_chat_runs(&state).await;
    let claim = hold_claim(&state, agent).await?;
    let kernel = Kernel::new(
        state.store.clone(),
        state.provider.clone(),
        ExecutorRouter::new(),
        KernelConfig::default(),
        agent,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("kernel: {e}")))?;
    let started = kernel
        .process_direct(Transaction::session_start(agent))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not record the new session: {e}"),
            )
        })?;
    drop(claim);
    end_trace(&state, agent).await;
    Ok(Json(SessionStarted {
        head_seq: started.entry.seq,
    }))
}

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct TracesQuery {
    #[serde(default)]
    since: u64,
    /// Whose runs; without it the page is empty.
    #[serde(default)]
    agent: Option<String>,
}

/// `GET /v1/sync/traces?agent=`: the agent's newest runs here, whole
/// ([`aura_agent_trace::db::RunCopy`]), newest first. The hub replaces
/// its copy of each, so a run that grew is sent again in full.
pub(in crate::gateway) async fn sync_traces_handler(
    State(state): State<RouterState>,
    Query(query): Query<TracesQuery>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let runs = match query.agent.as_deref() {
        Some(hex) => {
            let agent = parse_agent(hex)?;
            let cloud = Arc::clone(&state.cloud);
            tokio::task::spawn_blocking(move || cloud.runs_of(agent, TRACE_RUNS_SERVED))
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("could not read the runs: {e}"),
                    )
                })?
                .iter()
                .filter_map(|run| serde_json::to_value(run).ok())
                .collect()
        }
        None => Vec::new(),
    };
    Ok(Json(TracePage {
        runs,
        next_since: query.since,
    }))
}
