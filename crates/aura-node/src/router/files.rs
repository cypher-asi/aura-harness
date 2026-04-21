use super::*;
use crate::config::PathError;
use tokio::io::AsyncReadExt;
use tracing::warn;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".svn",
    ".hg",
    "vendor",
];

/// Maximum number of bytes the `read-file` handler will return.
///
/// Caps accidental `cat`s of huge files and, more importantly, denies an
/// OOM vector where a symlink / junction pointed at a pseudo-file such as
/// `/dev/zero` so `read_to_string` could never terminate. The cap is
/// enforced with `AsyncReadExt::take` so bytes beyond the limit never
/// hit our address space.
const MAX_READ_BYTES: u64 = 5 * 1024 * 1024;

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

#[derive(Debug, Serialize)]
struct FileDirEntry {
    name: String,
    path: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Self>>,
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

async fn walk_directory(
    base: &std::path::Path,
    start_rel: &std::path::Path,
    max_depth: usize,
) -> Vec<FileDirEntry> {
    use std::collections::HashMap;
    use std::path::PathBuf;

    // Phase 1: async iterative traversal. We avoid async recursion by driving
    // the walk with an explicit stack and recording each directory's ordered
    // children in a flat map keyed by relative path.
    let mut contents: HashMap<PathBuf, Vec<(String, bool, PathBuf)>> = HashMap::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(start_rel.to_path_buf(), 0)];

    while let Some((rel, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        let abs = base.join(&rel);
        let mut read_dir = match tokio::fs::read_dir(&abs).await {
            Ok(rd) => rd,
            Err(_) => {
                warn!(path = %abs.display(), "failed to read directory during walk");
                continue;
            }
        };

        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        let mut files: Vec<(String, PathBuf)> = Vec::new();

        loop {
            match read_dir.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let is_dir = match entry.file_type().await {
                        Ok(ft) => ft.is_dir(),
                        Err(_) => false,
                    };
                    let entry_rel = rel.join(&name);
                    if is_dir {
                        if !IGNORED_DIRS.contains(&name.as_str()) {
                            dirs.push((name, entry_rel));
                        }
                    } else {
                        files.push((name, entry_rel));
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        dirs.sort_by(|a, b| a.0.cmp(&b.0));
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut rel_contents: Vec<(String, bool, PathBuf)> =
            Vec::with_capacity(dirs.len() + files.len());
        for (name, entry_rel) in &dirs {
            rel_contents.push((name.clone(), true, entry_rel.clone()));
        }
        for (name, entry_rel) in files {
            rel_contents.push((name, false, entry_rel));
        }
        contents.insert(rel, rel_contents);

        for (_, entry_rel) in dirs {
            stack.push((entry_rel, depth + 1));
        }
    }

    // Phase 2: assemble the nested tree from the flat map. This is pure
    // in-memory work bounded by `max_depth` (<= 20), so sync recursion is
    // safe and keeps the structure straightforward.
    fn assemble(
        rel: &std::path::Path,
        contents: &HashMap<PathBuf, Vec<(String, bool, PathBuf)>>,
    ) -> Vec<FileDirEntry> {
        let Some(children) = contents.get(rel) else {
            return Vec::new();
        };
        children
            .iter()
            .map(|(name, is_dir, entry_rel)| {
                let kids = if *is_dir {
                    Some(assemble(entry_rel, contents))
                } else {
                    None
                };
                FileDirEntry {
                    name: name.clone(),
                    path: entry_rel.to_string_lossy().into_owned().replace('\\', "/"),
                    is_dir: *is_dir,
                    children: kids,
                }
            })
            .collect()
    }

    assemble(start_rel, &contents)
}

pub(super) async fn list_files_handler(
    State(state): State<RouterState>,
    Query(query): Query<ListFilesQuery>,
) -> impl IntoResponse {
    let depth = query.depth.min(20);

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

    let rel = std::path::PathBuf::from(".");
    let entries = walk_directory(&base, &rel, depth).await;

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

    // Stream through `take(MAX_READ_BYTES + 1)` so we can tell "at the
    // limit" from "over the limit" without ever buffering more than
    // `MAX_READ_BYTES + 1` bytes. `read_to_string` would have allocated
    // for the full file up front — the whole point of this handler is
    // to deny that.
    let file = match tokio::fs::File::open(&resolved).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!("failed to open {}: {e}", resolved.display()),
                })),
            )
                .into_response();
        }
    };

    let mut buf: Vec<u8> = Vec::new();
    let cap = MAX_READ_BYTES;
    let mut limited = file.take(cap + 1);
    if let Err(e) = limited.read_to_end(&mut buf).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to read {}: {e}", resolved.display()),
            })),
        )
            .into_response();
    }

    if buf.len() as u64 > cap {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("file exceeds {cap}-byte read cap"),
                "max_bytes": cap,
            })),
        )
            .into_response();
    }

    // Decode as UTF-8 for the JSON payload. Lossy conversion matches
    // the previous `read_to_string` behaviour for clean text files and
    // degrades gracefully on binary input instead of returning a 500.
    let content = String::from_utf8_lossy(&buf).into_owned();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "content": content,
            "path": resolved.to_string_lossy(),
            "bytes": buf.len(),
        })),
    )
        .into_response()
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
