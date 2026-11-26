//! Hosted workspace import (`import_workspace_handler`).

use super::workspace::validate_workspace_key;
use super::{error_response, Rejection};
use crate::gateway::RouterState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::Deserialize;
use std::path::{Component, PathBuf};

const MAX_IMPORTED_FILES: usize = 4_096;
const MAX_IMPORTED_BYTES: usize = 8 * 1024 * 1024;

/// Sanitized relative path + decoded contents of each imported file.
type DecodedFiles = Vec<(PathBuf, Vec<u8>)>;

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct ImportWorkspaceRequest {
    workspace_key: String,
    files: Vec<ImportedWorkspaceFile>,
}

#[derive(Debug, Deserialize)]
struct ImportedWorkspaceFile {
    relative_path: String,
    contents_base64: String,
}

fn sanitize_import_path(relative_path: &str) -> Result<PathBuf, String> {
    let mut sanitized = PathBuf::new();
    for component in std::path::Path::new(relative_path).components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("invalid imported file path: {relative_path}"));
            }
        }
    }
    if sanitized.as_os_str().is_empty() {
        return Err("imported files must include a relative path".to_string());
    }
    Ok(sanitized)
}

async fn create_safe_parent(
    workspace: &std::path::Path,
    relative: &std::path::Path,
) -> Result<(), String> {
    let mut current = workspace.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let Component::Normal(part) = component else {
                return Err("invalid imported file parent".to_string());
            };
            current.push(part);
            match tokio::fs::symlink_metadata(&current).await {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "imported file parent must not be a symlink: {}",
                        current.display()
                    ));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "imported file parent is not a directory: {}",
                        current.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::fs::create_dir(&current).await.map_err(|error| {
                        format!("failed to create imported workspace directory: {error}")
                    })?;
                }
                Err(error) => {
                    return Err(format!(
                        "failed to inspect imported workspace directory: {error}"
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(in crate::gateway) async fn import_workspace_handler(
    State(state): State<RouterState>,
    Json(request): Json<ImportWorkspaceRequest>,
) -> impl IntoResponse {
    match import_workspace(&state, request).await {
        Ok(response) => response,
        Err(rejection) => rejection.into_response(),
    }
}

async fn import_workspace(
    state: &RouterState,
    request: ImportWorkspaceRequest,
) -> Result<Response, Rejection> {
    let workspace_key = validate_workspace_key(&request.workspace_key)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, error))?;
    check_file_count(request.files.len())?;
    let file_count = request.files.len();
    let (decoded, total_bytes) = decode_files(request.files)?;

    let workspace = state.config.resolve_workspace_for_project(workspace_key);
    prepare_workspace_root(&workspace).await?;
    for (relative_path, contents) in decoded {
        write_imported_file(&workspace, &relative_path, contents).await?;
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "path": workspace.to_string_lossy(),
            "files": file_count,
            "bytes": total_bytes,
        })),
    )
        .into_response())
}

fn check_file_count(count: usize) -> Result<(), Rejection> {
    if count == 0 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "at least one file is required",
        ));
    }
    if count > MAX_IMPORTED_FILES {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("import exceeds the {MAX_IMPORTED_FILES}-file limit"),
        ));
    }
    Ok(())
}

/// Sanitize paths and decode contents, enforcing the total byte cap.
fn decode_files(files: Vec<ImportedWorkspaceFile>) -> Result<(DecodedFiles, usize), Rejection> {
    let mut decoded = Vec::with_capacity(files.len());
    let mut total_bytes = 0usize;
    for file in files {
        let relative_path = sanitize_import_path(&file.relative_path)
            .map_err(|error| error_response(StatusCode::BAD_REQUEST, error))?;
        let contents = base64::engine::general_purpose::STANDARD
            .decode(file.contents_base64)
            .map_err(|error| {
                error_response(
                    StatusCode::BAD_REQUEST,
                    format!("invalid imported file contents: {error}"),
                )
            })?;
        total_bytes = match total_bytes.checked_add(contents.len()) {
            Some(total) if total <= MAX_IMPORTED_BYTES => total,
            _ => {
                return Err(error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("import exceeds the {MAX_IMPORTED_BYTES}-byte limit"),
                ));
            }
        };
        decoded.push((relative_path, contents));
    }
    Ok((decoded, total_bytes))
}

async fn prepare_workspace_root(workspace: &std::path::Path) -> Result<(), Rejection> {
    if let Err(error) = tokio::fs::create_dir_all(workspace).await {
        return Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create hosted workspace: {error}"),
        ));
    }
    match tokio::fs::symlink_metadata(workspace).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(error_response(
                StatusCode::BAD_REQUEST,
                "hosted workspace root must be a real directory",
            ))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to inspect hosted workspace: {error}"),
        )),
    }
}

async fn write_imported_file(
    workspace: &std::path::Path,
    relative_path: &std::path::Path,
    contents: Vec<u8>,
) -> Result<(), Rejection> {
    if let Err(error) = create_safe_parent(workspace, relative_path).await {
        return Err(error_response(StatusCode::BAD_REQUEST, error));
    }
    let destination = workspace.join(relative_path);
    match tokio::fs::symlink_metadata(&destination).await {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_dir() => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "imported file destination is not a regular file: {}",
                    destination.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to inspect import destination: {error}"),
            ));
        }
    }
    if let Err(error) = tokio::fs::write(&destination, contents).await {
        return Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write imported file: {error}"),
        ));
    }
    Ok(())
}
