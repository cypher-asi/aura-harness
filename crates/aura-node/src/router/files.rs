use super::*;

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
    children: Option<Vec<FileDirEntry>>,
}

fn walk_directory(
    base: &std::path::Path,
    rel: &std::path::Path,
    depth: usize,
    max_depth: usize,
) -> Vec<FileDirEntry> {
    if depth >= max_depth {
        return Vec::new();
    }
    let abs = base.join(rel);
    let Ok(read_dir) = std::fs::read_dir(&abs) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let entry_rel = rel.join(&name);
        if is_dir {
            if !IGNORED_DIRS.contains(&name.as_str()) {
                dirs.push((name, entry_rel));
            }
        } else {
            files.push((name, entry_rel));
        }
    }

    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut entries = Vec::with_capacity(dirs.len() + files.len());
    for (name, entry_rel) in dirs {
        let children = walk_directory(base, &entry_rel, depth + 1, max_depth);
        entries.push(FileDirEntry {
            name,
            path: entry_rel.to_string_lossy().into_owned().replace('\\', "/"),
            is_dir: true,
            children: Some(children),
        });
    }
    for (name, entry_rel) in files {
        entries.push(FileDirEntry {
            name,
            path: entry_rel.to_string_lossy().into_owned().replace('\\', "/"),
            is_dir: false,
            children: None,
        });
    }
    entries
}

pub(super) async fn list_files_handler(
    State(state): State<RouterState>,
    Query(query): Query<ListFilesQuery>,
) -> impl IntoResponse {
    let root = state.config.file_root();
    let depth = query.depth.min(20);

    let base = if query.path == "." || query.path.is_empty() {
        root.clone()
    } else {
        let path = std::path::Path::new(&query.path);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if !state.config.is_allowed_path(&candidate) {
            return Json(serde_json::json!({ "ok": false, "error": "path escapes workspace" }));
        }
        candidate
    };

    if !base.is_dir() {
        return Json(serde_json::json!({ "ok": false, "error": "path not found" }));
    }

    let rel = std::path::PathBuf::from(".");
    let entries = tokio::task::spawn_blocking(move || walk_directory(&base, &rel, 0, depth))
        .await
        .unwrap_or_default();

    Json(serde_json::json!({ "ok": true, "entries": entries }))
}

#[derive(Debug, Deserialize)]
pub(super) struct ReadFileQuery {
    path: String,
}

pub(super) async fn read_file_handler(
    State(state): State<RouterState>,
    Query(query): Query<ReadFileQuery>,
) -> impl IntoResponse {
    let path = std::path::Path::new(&query.path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.config.file_root().join(path)
    };

    if !state.config.is_allowed_path(&resolved) {
        return Json(serde_json::json!({ "ok": false, "error": "path escapes workspace" }));
    }

    match tokio::fs::read_to_string(&resolved).await {
        Ok(content) => Json(serde_json::json!({
            "ok": true,
            "content": content,
            "path": resolved.to_string_lossy(),
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to read file: {e}"),
        })),
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
