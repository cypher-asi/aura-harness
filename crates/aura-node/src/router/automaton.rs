use super::*;
use aura_protocol::{InstalledIntegration, InstalledTool};

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
        .map_err(|e| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))))?;

    Ok(Json(
        serde_json::json!({"ok": true, "automaton_id": automaton_id, "status": "stopped"}),
    ))
}
