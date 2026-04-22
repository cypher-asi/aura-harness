use super::*;
use crate::config::PathError;
use crate::files_api::{self, ReadOutcome, WalkedEntry, MAX_READ_BYTES, MAX_WALK_DEPTH};

#[derive(Debug, Deserialize)]
pub(super) struct ListFilesQuery {
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
/// [`crate::files_api`] — the file-API helper stays on raw absolute
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

pub(super) async fn list_files_handler(
    State(state): State<RouterState>,
    Query(query): Query<ListFilesQuery>,
) -> impl IntoResponse {
    let depth = query.depth.min(MAX_WALK_DEPTH);

    // Every path — including the default "." — goes through the
    // canonicalizing resolver so a caller can't sneak past with e.g.
    // "./../etc". When the resolved path isn't a directory we surface
    // 400 rather than silently walking a single file.
    let input = std::path::Path::new(&query.path);
    let base = match state.config.resolve_allowed_path(input) {
        Ok(p) => p,
        Err(e) => {
            let (status, body) = path_error_response(&e);
            return (status, body).into_response();
        }
    };

    match tokio::fs::metadata(&base).await {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "error": "path is not a directory" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "path not found" })),
            )
                .into_response();
        }
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
pub(super) struct ReadFileQuery {
    path: String,
}

pub(super) async fn read_file_handler(
    State(state): State<RouterState>,
    Query(query): Query<ReadFileQuery>,
) -> impl IntoResponse {
    let input = std::path::Path::new(&query.path);
    let resolved = match state.config.resolve_allowed_path(input) {
        Ok(p) => p,
        Err(e) => {
            let (status, body) = path_error_response(&e);
            return (status, body).into_response();
        }
    };

    // Reject directories explicitly so we don't end up returning
    // `read_to_end` of a directory (which is an OS-specific error on
    // Linux / empty on Windows).
    match tokio::fs::metadata(&resolved).await {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "error": "path is not a file" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "path not found" })),
            )
                .into_response();
        }
    }

    match files_api::read_file_capped(&resolved, MAX_READ_BYTES).await {
        Ok(ReadOutcome::Ok { bytes }) => {
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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("{e}: {}", resolved.display()),
            })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveWorkspaceQuery {
    project_name: String,
}

pub(super) async fn resolve_workspace_handler(
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
