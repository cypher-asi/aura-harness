//! Embedded API server for health checks, file access, and local tooling.

use aura_terminal::UiCommand;
use axum::extract::Query;
use axum::{routing::get, Json, Router};
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

/// Default API server port
const API_PORT: u16 = 8080;

/// Fallback ports to try if the default is busy
const FALLBACK_PORTS: &[u16] = &[8081, 8082, 8090, 3000];

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

/// Start the embedded API server.
pub async fn start_api_server(cmd_tx: mpsc::Sender<UiCommand>) -> Option<String> {
    let app = Router::new()
        .route("/health", get(api_health_handler))
        .route("/api/files", get(api_list_files_handler))
        .route("/api/read-file", get(api_read_file_handler))
        .layer(TraceLayer::new_for_http());

    let ports_to_try = std::iter::once(API_PORT).chain(FALLBACK_PORTS.iter().copied());

    for port in ports_to_try {
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let url = format!("http://{addr}");
                info!(%url, "API server listening");

                if port != API_PORT {
                    let _ = cmd_tx.try_send(UiCommand::ShowWarning(format!(
                        "Port {API_PORT} busy, API server using port {port}"
                    )));
                }

                let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
                    url: Some(url.clone()),
                    active: true,
                });

                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, app).await {
                        error!(error = %e, "API server error");
                    }
                });

                return Some(url);
            }
            Err(e) => {
                debug!(port = port, error = %e, "Port unavailable, trying next");
            }
        }
    }

    warn!("Failed to start API server on any port");
    let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
        url: None,
        active: false,
    });
    let _ = cmd_tx.try_send(UiCommand::ShowError(
        "API server failed to start - all ports busy".to_string(),
    ));
    None
}

/// Health check endpoint handler.
async fn api_health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[derive(serde::Deserialize)]
struct ListFilesQuery {
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

#[derive(serde::Serialize)]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Self>>,
}

async fn walk_directory(start: &std::path::Path, max_depth: usize) -> Vec<DirEntry> {
    use std::collections::HashMap;
    use std::path::PathBuf;

    // Phase 1: async iterative walk driven by an explicit stack. For each
    // directory we visit we record its ordered children (dirs before files,
    // alphabetical within each group) into a flat map.
    let mut contents: HashMap<PathBuf, Vec<(String, bool, PathBuf)>> = HashMap::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(start.to_path_buf(), 0)];

    while let Some((path, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        let mut read_dir = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        let mut files: Vec<(String, PathBuf)> = Vec::new();

        loop {
            match read_dir.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    let entry_path = entry.path();
                    let is_dir = match entry.file_type().await {
                        Ok(ft) => ft.is_dir(),
                        Err(_) => false,
                    };
                    if is_dir {
                        if !IGNORED_DIRS.contains(&name.as_str()) {
                            dirs.push((name, entry_path));
                        }
                    } else {
                        files.push((name, entry_path));
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
        for (name, entry_path) in &dirs {
            rel_contents.push((name.clone(), true, entry_path.clone()));
        }
        for (name, entry_path) in files {
            rel_contents.push((name, false, entry_path));
        }
        contents.insert(path, rel_contents);

        for (_, entry_path) in dirs {
            stack.push((entry_path, depth + 1));
        }
    }

    // Phase 2: pure in-memory tree assembly bounded by `max_depth` (<= 20),
    // so sync recursion is safe and keeps the shape simple.
    fn assemble(
        path: &std::path::Path,
        contents: &HashMap<PathBuf, Vec<(String, bool, PathBuf)>>,
    ) -> Vec<DirEntry> {
        let Some(children) = contents.get(path) else {
            return Vec::new();
        };
        children
            .iter()
            .map(|(name, is_dir, entry_path)| {
                let kids = if *is_dir {
                    Some(assemble(entry_path, contents))
                } else {
                    None
                };
                DirEntry {
                    name: name.clone(),
                    path: entry_path.to_string_lossy().into_owned(),
                    is_dir: *is_dir,
                    children: kids,
                }
            })
            .collect()
    }

    assemble(start, &contents)
}

/// `GET /api/files?path=...&depth=...`
///
/// Lists directory contents recursively, returning a tree of `DirEntry` objects.
async fn api_list_files_handler(Query(query): Query<ListFilesQuery>) -> Json<serde_json::Value> {
    let target = std::path::Path::new(&query.path);
    let meta = match tokio::fs::metadata(target).await {
        Ok(m) => m,
        Err(_) => {
            return Json(serde_json::json!({ "ok": false, "error": "path not found" }));
        }
    };
    if !meta.is_dir() {
        return Json(serde_json::json!({ "ok": false, "error": "path is not a directory" }));
    }

    let max_depth = query.depth.min(20);
    let entries = walk_directory(target, max_depth).await;
    Json(serde_json::json!({ "ok": true, "entries": entries }))
}

#[derive(serde::Deserialize)]
struct ReadFileQuery {
    path: String,
}

/// `GET /api/read-file?path=...`
///
/// Reads a file and returns its text content.
async fn api_read_file_handler(Query(query): Query<ReadFileQuery>) -> Json<serde_json::Value> {
    let target = std::path::Path::new(&query.path);
    let meta = match tokio::fs::metadata(target).await {
        Ok(m) => m,
        Err(_) => {
            return Json(serde_json::json!({ "ok": false, "error": "path not found" }));
        }
    };
    if !meta.is_file() {
        return Json(serde_json::json!({ "ok": false, "error": "path is not a file" }));
    }

    match tokio::fs::read_to_string(&query.path).await {
        Ok(content) => {
            Json(serde_json::json!({ "ok": true, "content": content, "path": query.path }))
        }
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}
