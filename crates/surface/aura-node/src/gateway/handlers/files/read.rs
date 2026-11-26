//! `/api/files` directory listing and `/api/read-file`.

use super::{error_response, file_revision, require_kind, resolve_allowed};
use crate::gateway::RouterState;
use aura_exec_tools::files_api::{self, ReadOutcome, WalkedEntry, MAX_READ_BYTES, MAX_WALK_DEPTH};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct ListFilesQuery {
    #[serde(default = "default_files_path")]
    path: String,
    #[serde(default = "default_files_depth")]
    depth: usize,
}

fn default_files_path() -> String {
    ".".into()
}
fn default_files_depth() -> usize {
    3
}

/// Wire shape for a single directory entry on `/api/files`.
///
/// Paths are workspace-relative and forward-slash normalised so the
/// frontend can render them unchanged on Windows and Unix. That
/// contract is the reason this DTO is owned by the router rather than
/// [`aura_exec_tools::files_api`] — the file-API helper stays on raw absolute
/// paths and each caller (aura-node vs the TUI embedded server)
/// handles its own serialisation shape.
#[derive(Debug, Serialize)]
struct FileDirEntry {
    name: String,
    path: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Self>>,
}

/// Convert walker output into [`FileDirEntry`] tree with workspace-relative
/// forward-slash paths. `base` is the directory the caller started
/// the walk from (after `resolve_allowed_path`); relative paths are
/// reported relative to `base` so the frontend can navigate within
/// the listing without knowing the absolute sandbox root.
fn to_file_entries(base: &std::path::Path, entries: Vec<WalkedEntry>) -> Vec<FileDirEntry> {
    entries
        .into_iter()
        .map(|e| {
            let rel = e
                .abs_path
                .strip_prefix(base)
                .unwrap_or(&e.abs_path)
                .to_string_lossy()
                .into_owned()
                .replace('\\', "/");
            FileDirEntry {
                name: e.name,
                path: rel,
                is_dir: e.is_dir,
                children: e.children.map(|c| to_file_entries(base, c)),
            }
        })
        .collect()
}

pub(in crate::gateway) async fn list_files_handler(
    State(state): State<RouterState>,
    Query(query): Query<ListFilesQuery>,
) -> impl IntoResponse {
    let depth = query.depth.min(MAX_WALK_DEPTH);

    // Every path — including the default "." — goes through the
    // canonicalizing resolver so a caller can't sneak past with e.g.
    // "./../etc". When the resolved path isn't a directory we surface
    // 400 rather than silently walking a single file.
    let base = match resolve_allowed(&state, &query.path) {
        Ok(p) => p,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(response) = require_kind(&base, |m| m.is_dir(), "path is not a directory").await {
        return response.into_response();
    }

    // `files_api::walk_directory` returns absolute `PathBuf`s; we
    // render them workspace-relative with forward slashes via
    // `to_file_entries` so the JSON contract stays identical to the
    // pre-consolidation handler.
    let walked = files_api::walk_directory(&base, None, depth).await;
    let entries = to_file_entries(&base, walked);

    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "entries": entries })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct ReadFileQuery {
    path: String,
}

pub(in crate::gateway) async fn read_file_handler(
    State(state): State<RouterState>,
    Query(query): Query<ReadFileQuery>,
) -> impl IntoResponse {
    let resolved = match resolve_allowed(&state, &query.path) {
        Ok(p) => p,
        Err(rejection) => return rejection.into_response(),
    };

    // Reject directories explicitly so we don't end up returning
    // `read_to_end` of a directory (which is an OS-specific error on
    // Linux / empty on Windows).
    if let Err(response) = require_kind(&resolved, |m| m.is_file(), "path is not a file").await {
        return response.into_response();
    }

    read_capped_response(&resolved).await
}

async fn read_capped_response(resolved: &std::path::Path) -> Response {
    match files_api::read_file_capped(resolved, MAX_READ_BYTES).await {
        Ok(ReadOutcome::Ok { bytes }) => {
            let revision = file_revision(&bytes);
            // Decode as UTF-8 for the JSON payload. Lossy conversion
            // matches the previous `read_to_string` behaviour for
            // clean text files and degrades gracefully on binary
            // input instead of returning a 500.
            let content = String::from_utf8_lossy(&bytes).into_owned();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "content": content,
                    "path": resolved.to_string_lossy(),
                    "bytes": bytes.len(),
                    "revision": revision,
                })),
            )
                .into_response()
        }
        Ok(ReadOutcome::TooLarge { max_bytes }) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("file exceeds {max_bytes}-byte read cap"),
                "max_bytes": max_bytes,
            })),
        )
            .into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{e}: {}", resolved.display()),
        )
        .into_response(),
    }
}
