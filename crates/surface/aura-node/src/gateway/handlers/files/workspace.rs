//! Hosted workspace resolve/delete and workspace key validation.

use crate::gateway::RouterState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct ResolveWorkspaceQuery {
    project_name: String,
}

pub(in crate::gateway) async fn resolve_workspace_handler(
    State(state): State<RouterState>,
    Query(query): Query<ResolveWorkspaceQuery>,
) -> impl IntoResponse {
    let path = state
        .config
        .resolve_workspace_for_project(&query.project_name);
    Json(serde_json::json!({
        "path": path.to_string_lossy(),
    }))
}

pub(super) fn validate_workspace_key(workspace_key: &str) -> Result<&str, String> {
    let workspace_key = workspace_key.trim();
    if workspace_key.is_empty() {
        return Err("workspace_key is required".to_string());
    }
    if workspace_key.len() > 128
        || !workspace_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(
            "workspace_key must contain only ASCII letters, numbers, '-' or '_'".to_string(),
        );
    }
    Ok(workspace_key)
}

pub(in crate::gateway) async fn delete_workspace_handler(
    State(state): State<RouterState>,
    Path(workspace_key): Path<String>,
) -> impl IntoResponse {
    let Ok(workspace_key) = validate_workspace_key(&workspace_key) else {
        return StatusCode::BAD_REQUEST;
    };
    let workspace = state.config.resolve_workspace_for_project(workspace_key);
    match tokio::fs::symlink_metadata(&workspace).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return StatusCode::BAD_REQUEST;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = super::super::safe_workspace::delete_project_safe_workspaces(
                &state.config.data_dir,
                workspace_key,
            )
            .await
            {
                warn!(%error, "failed to delete hosted project Safe Workspaces");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            return StatusCode::NO_CONTENT;
        }
        Err(error) => {
            warn!(path = %workspace.display(), %error, "failed to inspect hosted workspace");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    if let Err(error) = super::super::safe_workspace::delete_project_safe_workspaces(
        &state.config.data_dir,
        workspace_key,
    )
    .await
    {
        warn!(%error, "failed to delete hosted project Safe Workspaces");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    match tokio::fs::remove_dir_all(&workspace).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            warn!(path = %workspace.display(), %error, "failed to delete hosted workspace");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
