//! Axum handlers for the protected Safe Workspace endpoints. Blocking Git and
//! filesystem work runs on `spawn_blocking`.

use super::checkpoints::{
    apply_to_source_blocking, checkpoint_diff_blocking, list_checkpoints_blocking,
    restore_checkpoint_blocking, take_checkpoint,
};
use super::setup::prepare_workspace_blocking;
use super::storage::{
    find_existing_session_root, read_metadata, session_root, validate_key, WorkspaceLock,
};
use super::{SafeWorkspaceError, SafeWorkspaceStatus};
use crate::gateway::RouterState;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Json;
use serde::Serialize;
use std::path::Path;

fn workspace_error_response(error: &SafeWorkspaceError) -> Response {
    let status = match error {
        SafeWorkspaceError::Unsupported(_) | SafeWorkspaceError::InvalidMetadata(_) => {
            StatusCode::BAD_REQUEST
        }
        SafeWorkspaceError::Busy | SafeWorkspaceError::Conflict(_) => StatusCode::CONFLICT,
        SafeWorkspaceError::Git(_) | SafeWorkspaceError::Io(_) | SafeWorkspaceError::Json(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Json(serde_json::json!({
            "ok": false,
            "error": error.to_string(),
        })),
    )
        .into_response()
}

/// Response for a `spawn_blocking` task that panicked or was cancelled.
fn task_failed_response(error: &tokio::task::JoinError) -> Response {
    workspace_error_response(&SafeWorkspaceError::Io(std::io::Error::other(format!(
        "safe workspace task failed: {error}"
    ))))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareSafeWorkspaceResponse {
    workspace_path: String,
}

pub(in crate::gateway) async fn prepare_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    let workspace_key = match validate_key(&workspace_key, "workspace key") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(&error),
    };
    let session_id = match validate_key(&session_id, "session id") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(&error),
    };
    let data_dir = state.config.data_dir.clone();
    let source_path = state.config.resolve_workspace_for_project(&workspace_key);
    match tokio::task::spawn_blocking(move || {
        let metadata =
            prepare_workspace_blocking(&data_dir, &workspace_key, &session_id, &source_path)?;
        let root = session_root(&data_dir, &workspace_key, &session_id);
        let _lock = WorkspaceLock::acquire(&root)?;
        take_checkpoint(&metadata, "before chat turn")?;
        Ok::<_, SafeWorkspaceError>(PrepareSafeWorkspaceResponse {
            workspace_path: metadata.workspace_path.to_string_lossy().to_string(),
        })
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(&error),
        Err(error) => task_failed_response(&error),
    }
}

fn disabled_status() -> SafeWorkspaceStatus {
    SafeWorkspaceStatus {
        enabled: false,
        workspace_path: None,
        source_path: None,
        base_commit: None,
        created_at: None,
        checkpoints: Vec::new(),
    }
}

fn read_status_blocking(root: &Path) -> Result<SafeWorkspaceStatus, SafeWorkspaceError> {
    let metadata = read_metadata(root)?;
    let checkpoints = list_checkpoints_blocking(&metadata)?;
    Ok(SafeWorkspaceStatus {
        enabled: true,
        workspace_path: Some(metadata.workspace_path.to_string_lossy().to_string()),
        source_path: Some(metadata.source_display_path()),
        base_commit: Some(metadata.base_commit),
        created_at: Some(metadata.created_at),
        checkpoints,
    })
}

pub(in crate::gateway) async fn safe_workspace_status_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    let workspace_key = match validate_key(&workspace_key, "workspace key") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(&error),
    };
    let session_id = match validate_key(&session_id, "session id") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(&error),
    };
    let data_dir = state.config.data_dir.clone();
    let root = match find_existing_session_root(&data_dir, &workspace_key, &session_id) {
        Ok(Some(root)) => root,
        Ok(None) => return Json(disabled_status()).into_response(),
        Err(error) => return workspace_error_response(&error),
    };
    match tokio::task::spawn_blocking(move || read_status_blocking(&root)).await {
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(error)) => workspace_error_response(&error),
        Err(error) => task_failed_response(&error),
    }
}

pub(in crate::gateway) async fn safe_workspace_diff_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id, checkpoint_id)): AxumPath<(String, String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(&error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(&error),
    };
    match tokio::task::spawn_blocking(move || {
        let _lock = WorkspaceLock::acquire(&root)?;
        let metadata = read_metadata(&root)?;
        checkpoint_diff_blocking(&metadata, &checkpoint_id)
    })
    .await
    {
        Ok(Ok(diff)) => Json(diff).into_response(),
        Ok(Err(error)) => workspace_error_response(&error),
        Err(error) => task_failed_response(&error),
    }
}

pub(in crate::gateway) async fn restore_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id, checkpoint_id)): AxumPath<(String, String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(&error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(&error),
    };
    match tokio::task::spawn_blocking(move || restore_checkpoint_blocking(&root, &checkpoint_id))
        .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(&error),
        Err(error) => task_failed_response(&error),
    }
}

pub(in crate::gateway) async fn apply_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(&error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(&error),
    };
    match tokio::task::spawn_blocking(move || apply_to_source_blocking(&root)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(&error),
        Err(error) => task_failed_response(&error),
    }
}
