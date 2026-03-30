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
pub(crate) async fn start_api_server(cmd_tx: mpsc::Sender<UiCommand>) -> Option<String> {
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
    children: Option<Vec<DirEntry>>,
}

fn dir_first_then_name(a: &std::fs::DirEntry, b: &std::fs::DirEntry) -> std::cmp::Ordering {
    let a_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
    let b_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
    b_dir
        .cmp(&a_dir)
        .then_with(|| a.file_name().cmp(&b.file_name()))
}

fn build_dir_entry(item: std::fs::DirEntry, depth: usize, max_depth: usize) -> Option<DirEntry> {
    let name = item.file_name().to_string_lossy().into_owned();
    if name.starts_with('.') {
        return None;
    }
    let item_path = item.path();
    let is_dir = item.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
    if is_dir && IGNORED_DIRS.contains(&name.as_str()) {
        return None;
    }
    let children = if is_dir {
        Some(walk_directory(&item_path, depth + 1, max_depth))
    } else {
        None
    };
    Some(DirEntry {
        name,
        path: item_path.to_string_lossy().into_owned(),
        is_dir,
        children,
    })
}

fn walk_directory(path: &std::path::Path, depth: usize, max_depth: usize) -> Vec<DirEntry> {
    if depth >= max_depth {
        return Vec::new();
    }
    let Ok(read_dir) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut items: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
    items.sort_by(dir_first_then_name);
    items
        .into_iter()
        .filter_map(|item| build_dir_entry(item, depth, max_depth))
        .collect()
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
    let target_owned = target.to_path_buf();
    let entries = tokio::task::spawn_blocking(move || walk_directory(&target_owned, 0, max_depth))
        .await
        .unwrap_or_default();
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
