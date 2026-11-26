//! Revision-checked workspace file saves (`write_file_handler`).

use super::{
    error_response, file_revision, require_kind, resolve_allowed, Rejection, MAX_WRITE_BYTES,
};
use crate::gateway::RouterState;
use aura_exec_tools::files_api::{self, ReadOutcome, MAX_READ_BYTES};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::Deserialize;

const CHANGED_SINCE_OPENED: &str = "file changed since it was opened; reopen it before saving";
const NOT_UTF8: &str = "only UTF-8 text files can be edited";

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct WriteFileRequest {
    path: String,
    content_base64: String,
    expected_revision: String,
}

/// Replace an existing UTF-8 workspace file without clobbering a newer edit.
///
/// The caller must send the revision digest it originally read. This turns an
/// agent or second browser changing the file between open and save into an HTTP
/// 409 instead of silently losing that newer work.
pub(in crate::gateway) async fn write_file_handler(
    State(state): State<RouterState>,
    Json(request): Json<WriteFileRequest>,
) -> impl IntoResponse {
    match write_file(&state, request).await {
        Ok(response) => response,
        Err(rejection) => rejection.into_response(),
    }
}

async fn write_file(state: &RouterState, request: WriteFileRequest) -> Result<Response, Rejection> {
    validate_revision(&request.expected_revision)?;
    let content = decode_content(&request.content_base64)?;
    let resolved = resolve_allowed(state, &request.path)?;
    require_kind(&resolved, |m| m.is_file(), "path is not a file").await?;
    let current = read_current(&resolved).await?;
    if std::str::from_utf8(&current).is_err() {
        return Err(error_response(StatusCode::BAD_REQUEST, NOT_UTF8));
    }
    if !file_revision(&current).eq_ignore_ascii_case(&request.expected_revision) {
        return Err(error_response(StatusCode::CONFLICT, CHANGED_SINCE_OPENED));
    }
    Ok(write_content(&resolved, &content).await)
}

fn validate_revision(expected_revision: &str) -> Result<(), Rejection> {
    if expected_revision.len() != 64
        || !expected_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "expected_revision must be a 64-character hex digest",
        ));
    }
    Ok(())
}

/// Decode the base64 body and enforce the write cap and UTF-8 contract.
fn decode_content(content_base64: &str) -> Result<Vec<u8>, Rejection> {
    let Ok(content) = base64::engine::general_purpose::STANDARD.decode(content_base64) else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "content_base64 must contain valid base64",
        ));
    };
    if content.len() > MAX_WRITE_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("file exceeds {MAX_WRITE_BYTES}-byte write cap"),
                "max_bytes": MAX_WRITE_BYTES,
            })),
        ));
    }
    if std::str::from_utf8(&content).is_err() {
        return Err(error_response(StatusCode::BAD_REQUEST, NOT_UTF8));
    }
    Ok(content)
}

async fn read_current(resolved: &std::path::Path) -> Result<Vec<u8>, Rejection> {
    match files_api::read_file_capped(resolved, MAX_READ_BYTES).await {
        Ok(ReadOutcome::Ok { bytes }) => Ok(bytes),
        Ok(ReadOutcome::TooLarge { .. }) => {
            Err(error_response(StatusCode::CONFLICT, CHANGED_SINCE_OPENED))
        }
        Err(error) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read file before saving: {error}"),
        )),
    }
}

async fn write_content(resolved: &std::path::Path, content: &[u8]) -> Response {
    match tokio::fs::write(resolved, content).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "path": resolved.to_string_lossy(),
                "revision": file_revision(content),
            })),
        )
            .into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write file: {error}"),
        )
        .into_response(),
    }
}
