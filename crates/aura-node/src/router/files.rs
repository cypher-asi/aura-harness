use super::*;
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

    match tokio::fs::metadata(&base).await {
        Ok(m) if m.is_dir() => {}
        _ => {
            return Json(serde_json::json!({ "ok": false, "error": "path not found" }));
        }
    }

    let rel = std::path::PathBuf::from(".");
    let entries = walk_directory(&base, &rel, depth).await;

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
            "error": format!("failed to read {}: {e}", resolved.display()),
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
