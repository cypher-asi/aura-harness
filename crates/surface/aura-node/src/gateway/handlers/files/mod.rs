//! Workspace file endpoints: listing, reading, editing, and hosted
//! workspace import/resolve/delete.
//!
//! Split into:
//! - [`read`] — `/api/files` listing and `/api/read-file`.
//! - [`write`] — revision-checked file saves.
//! - [`import`] — hosted workspace import.
//! - [`workspace`] — hosted workspace resolve/delete and key validation.

mod import;
mod read;
mod workspace;
mod write;

use crate::config::PathError;
use crate::gateway::RouterState;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use std::path::PathBuf;

pub(in crate::gateway) use import::import_workspace_handler;
pub(in crate::gateway) use read::{list_files_handler, read_file_handler};
pub(in crate::gateway) use workspace::{delete_workspace_handler, resolve_workspace_handler};
pub(in crate::gateway) use write::write_file_handler;

/// Keeps the base64 JSON request below Swarm's 1 MiB gateway limit.
pub(in crate::gateway) const MAX_WRITE_BYTES: usize = 700 * 1024;

fn file_revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Error half of the file handlers' internal results; converted with
/// `into_response` at the handler boundary.
type Rejection = (StatusCode, Json<serde_json::Value>);

/// Build a `{ "ok": false, "error": ... }` rejection with `status`.
fn error_response(status: StatusCode, error: impl Serialize) -> Rejection {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": error })),
    )
}

/// Map a [`PathError`] to an HTTP status + JSON body.
///
/// Keeping the mapping in one place means `/api/files` and
/// `/api/read-file` report traversal attempts, missing files, and
/// permission failures with the same status codes, and changes to that
/// policy only need to land here.
fn path_error_response(err: &PathError) -> (StatusCode, Json<serde_json::Value>) {
    match err {
        PathError::NotFound(p) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("path not found: {}", p.display()),
            })),
        ),
        PathError::Escapes(_) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "ok": false,
                "error": "path escapes workspace",
            })),
        ),
        PathError::NotPermitted(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error": msg,
            })),
        ),
    }
}

/// Resolve `path` through the canonicalizing sandbox resolver.
fn resolve_allowed(state: &RouterState, path: &str) -> Result<PathBuf, Rejection> {
    state
        .config
        .resolve_allowed_path(std::path::Path::new(path))
        .map_err(|e| path_error_response(&e))
}

/// Require `path` to exist and satisfy `is_kind`; otherwise 400 with
/// `wrong_kind` (or 404 when metadata can't be read).
async fn require_kind(
    path: &std::path::Path,
    is_kind: fn(&std::fs::Metadata) -> bool,
    wrong_kind: &'static str,
) -> Result<(), Rejection> {
    match tokio::fs::metadata(path).await {
        Ok(m) if is_kind(&m) => Ok(()),
        Ok(_) => Err(error_response(StatusCode::BAD_REQUEST, wrong_kind)),
        Err(_) => Err(error_response(StatusCode::NOT_FOUND, "path not found")),
    }
}
