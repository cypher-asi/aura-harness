use super::*;
use aura_protocol::{AgentPermissionsWire, InstalledIntegration, InstalledTool};

#[derive(Debug, Deserialize)]
pub(super) struct AutomatonStartRequest {
    project_id: String,
    #[serde(default)]
    auth_token: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    git_repo_url: Option<String>,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    installed_tools: Option<Vec<InstalledTool>>,
    #[serde(default)]
    installed_integrations: Option<Vec<InstalledIntegration>>,
    /// Capability + scope bundle for the agent driving this automaton.
    /// Defaults to empty for older callers, preserving the strict policy
    /// behavior until aura-os sends the real agent bundle.
    #[serde(default)]
    agent_permissions: AgentPermissionsWire,
    /// Retry-warm-up: the reason text persisted on the previous
    /// attempt's `task_failed` record. Threaded into the task-run
    /// automaton config as `prior_failure`, which the automaton folds
    /// into `TaskInfo::execution_notes`. Ignored on dev-loop starts
    /// (`task_id` is `None`). `#[serde(default)]` keeps older clients
    /// — which never sent this field — working unchanged.
    #[serde(default)]
    prior_failure: Option<String>,
    /// Retry-warm-up: recent work-log entries the caller wants the
    /// agent to re-see on this attempt. Threaded into the task-run
    /// automaton config as `work_log` and fed straight into
    /// `AgenticTaskParams::work_log`. Ignored on dev-loop starts.
    #[serde(default)]
    work_log: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AutomatonStartResponse {
    automaton_id: String,
    event_stream_url: String,
}

/// Start a dev-loop or single-task automaton.
/// When `task_id` is provided, runs a single task; otherwise starts the full dev loop.
pub(super) async fn automaton_start_handler(
    headers: HeaderMap,
    State(state): State<RouterState>,
    Json(req): Json<AutomatonStartRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    let auth_token = req.auth_token.or_else(|| {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(String::from)
    });

    let workspace_root = req.workspace_root.map(|s| {
        let path = std::path::PathBuf::from(s);
        state.config.resolve_project_path(&path)
    });
    let agent_permissions = crate::session::agent_permissions_from_wire(req.agent_permissions);

    let automaton_id = if let Some(task_id) = req.task_id {
        bridge
            .run_task_with_capabilities(
                &req.project_id,
                &task_id,
                workspace_root,
                auth_token,
                req.model,
                req.git_repo_url,
                req.git_branch,
                req.installed_tools,
                req.installed_integrations,
                agent_permissions,
                req.prior_failure,
                req.work_log,
            )
            .await
    } else {
        bridge
            .start_dev_loop_with_capabilities(
                &req.project_id,
                workspace_root,
                auth_token,
                req.model,
                req.git_repo_url,
                req.git_branch,
                req.installed_tools,
                req.installed_integrations,
                agent_permissions,
            )
            .await
    }
    .map_err(|e| (StatusCode::CONFLICT, Json(serde_json::json!({"error": e}))))?;

    Ok((
        StatusCode::CREATED,
        Json(AutomatonStartResponse {
            event_stream_url: format!("/stream/automaton/{automaton_id}"),
            automaton_id,
        }),
    ))
}

/// Get the status of a running automaton.
pub(super) async fn automaton_status_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    match bridge.get_status(&automaton_id) {
        Some(info) => Ok(Json(serde_json::to_value(&info).unwrap_or_default())),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("automaton {automaton_id} not found")})),
        )),
    }
}

/// List all running automatons.
pub(super) async fn automaton_list_handler(
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    let list = bridge.list_automatons();
    Ok(Json(
        serde_json::to_value(&list).unwrap_or(serde_json::json!([])),
    ))
}

/// Pause a running automaton.
pub(super) async fn automaton_pause_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    bridge
        .pause_by_id(&automaton_id)
        .map_err(|e| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))))?;

    Ok(Json(
        serde_json::json!({"ok": true, "automaton_id": automaton_id, "status": "paused"}),
    ))
}

/// Stop a running automaton.
pub(super) async fn automaton_stop_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    bridge
        .stop_by_id(&automaton_id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))))?;

    Ok(Json(
        serde_json::json!({"ok": true, "automaton_id": automaton_id, "status": "stopped"}),
    ))
}
