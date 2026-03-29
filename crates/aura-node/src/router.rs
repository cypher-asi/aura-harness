//! HTTP and WebSocket router for the node API.

use crate::config::NodeConfig;
use crate::scheduler::Scheduler;
use crate::automaton_bridge::AutomatonBridge;
use crate::session::{handle_ws_connection, WsContext};
use crate::terminal;
use aura_core::{AgentId, Transaction, TransactionType};
use aura_reasoner::ModelProvider;
use aura_store::Store;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::domain_tools::DomainApi;
use aura_tools::{ToolCatalog, ToolConfig};
use axum::{
    extract::{ws::WebSocketUpgrade, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{error, info, instrument};

/// Shared state for the router.
pub struct RouterState {
    pub store: Arc<dyn Store>,
    pub scheduler: Arc<Scheduler>,
    pub config: NodeConfig,
    /// Model provider for WebSocket sessions (type-erased).
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Tool configuration for WebSocket sessions.
    pub tool_config: ToolConfig,
    /// Canonical tool catalog (shared across sessions).
    pub catalog: Arc<ToolCatalog>,
    /// Domain API for specs/tasks/project/orbit/network (None if no internal token).
    pub domain_api: Option<Arc<dyn DomainApi>>,
    /// Automaton controller for dev-loop lifecycle (None when domain API unavailable).
    pub automaton_controller: Option<Arc<dyn AutomatonController>>,
    /// Concrete bridge for event subscription (same object as automaton_controller).
    pub automaton_bridge: Option<Arc<AutomatonBridge>>,
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            scheduler: self.scheduler.clone(),
            config: self.config.clone(),
            provider: self.provider.clone(),
            tool_config: self.tool_config.clone(),
            catalog: self.catalog.clone(),
            domain_api: self.domain_api.clone(),
            automaton_controller: self.automaton_controller.clone(),
            automaton_bridge: self.automaton_bridge.clone(),
        }
    }
}

/// Create the router.
pub fn create_router(state: RouterState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/api/files", get(list_files_handler))
        .route("/api/read-file", get(read_file_handler))
        .route("/workspace/resolve", get(resolve_workspace_handler))
        .route("/tx", post(submit_tx_handler))
        .route("/agents/:agent_id/head", get(get_head_handler))
        .route("/agents/:agent_id/record", get(scan_record_handler))
        .route("/ws/terminal", get(terminal_ws_handler))
        .route("/stream", get(ws_upgrade_handler))
        .route(
            "/stream/automaton/:automaton_id",
            get(automaton_ws_handler),
        )
        .route("/automaton/start", post(automaton_start_handler))
        .route("/automaton/list", get(automaton_list_handler))
        .route(
            "/automaton/:automaton_id/status",
            get(automaton_status_handler),
        )
        .route(
            "/automaton/:automaton_id/pause",
            post(automaton_pause_handler),
        )
        .route(
            "/automaton/:automaton_id/stop",
            post(automaton_stop_handler),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// === Terminal WebSocket ===

async fn terminal_ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(terminal::handle_terminal_ws)
}

// === Health ===

/// Return a simple health-check response with version info.
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// === Files ===

const IGNORED_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".next", "dist", "build", ".svn", ".hg", "vendor"];

#[derive(Debug, Deserialize)]
struct ListFilesQuery {
    #[serde(default = "default_files_path")]
    path: String,
    #[serde(default = "default_files_depth")]
    depth: usize,
}

fn default_files_path() -> String { ".".into() }
fn default_files_depth() -> usize { 3 }

#[derive(Debug, Serialize)]
struct FileDirEntry {
    name: String,
    path: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<FileDirEntry>>,
}

fn walk_directory(base: &std::path::Path, rel: &std::path::Path, depth: usize, max_depth: usize) -> Vec<FileDirEntry> {
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

async fn list_files_handler(
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
struct ReadFileQuery {
    path: String,
}

async fn read_file_handler(
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
struct ResolveWorkspaceQuery {
    project_name: String,
}

async fn resolve_workspace_handler(
    State(state): State<RouterState>,
    Query(query): Query<ResolveWorkspaceQuery>,
) -> impl IntoResponse {
    let path = state.config.resolve_workspace_for_project(&query.project_name);
    Json(serde_json::json!({
        "path": path.to_string_lossy(),
    }))
}

// === Submit Transaction ===

#[derive(Debug, Deserialize)]
struct SubmitTxRequest {
    agent_id: String,
    kind: String,
    payload: String,
}

#[derive(Debug, Serialize)]
struct SubmitTxResponse {
    accepted: bool,
    tx_id: String,
}

/// Accept a transaction submission, enqueue it, and schedule the agent for processing.
#[instrument(skip(state, request))]
async fn submit_tx_handler(
    State(state): State<RouterState>,
    Json(request): Json<SubmitTxRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&request.agent_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let tx_type = match request.kind.as_str() {
        "user_prompt" => TransactionType::UserPrompt,
        "agent_msg" => TransactionType::AgentMsg,
        "trigger" => TransactionType::Trigger,
        "action_result" => TransactionType::ActionResult,
        "system" => TransactionType::System,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid kind: {}", request.kind),
            ))
        }
    };

    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&request.payload)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid payload encoding: {e}"),
            )
        })?;

    let tx = Transaction::new_chained(agent_id, tx_type, Bytes::from(payload), None);

    state.store.enqueue_tx(&tx).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    info!(hash = %tx.hash, agent_id = %agent_id, "Transaction enqueued");

    let scheduler = state.scheduler.clone();
    tokio::spawn(async move {
        if let Err(e) = scheduler.schedule_agent(agent_id).await {
            error!(error = %e, "Failed to process agent");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitTxResponse {
            accepted: true,
            tx_id: tx.hash.to_hex(),
        }),
    ))
}

// === Get Head ===

#[derive(Debug, Serialize)]
struct GetHeadResponse {
    agent_id: String,
    head_seq: u64,
}

/// Return the current head sequence number for a given agent.
#[instrument(skip(state))]
async fn get_head_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&agent_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let head_seq = state.store.get_head_seq(agent_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Storage error: {e}"),
        )
    })?;

    Ok(Json(GetHeadResponse {
        agent_id: agent_id_hex,
        head_seq,
    }))
}

// === Scan Record ===

#[derive(Debug, Deserialize)]
struct ScanRecordQuery {
    #[serde(default = "default_from_seq")]
    from_seq: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}

const fn default_from_seq() -> u64 {
    1
}

const fn default_limit() -> usize {
    100
}

/// Scan an agent's record from a given sequence number, returning up to `limit` entries.
#[instrument(skip(state))]
async fn scan_record_handler(
    State(state): State<RouterState>,
    Path(agent_id_hex): Path<String>,
    Query(query): Query<ScanRecordQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let agent_id = AgentId::from_hex(&agent_id_hex)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid agent_id: {e}")))?;

    let limit = query.limit.min(1000);

    let entries = state
        .store
        .scan_record(agent_id, query.from_seq, limit)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Storage error: {e}"),
            )
        })?;

    Ok(Json(entries))
}

// === Automaton REST API ===

#[derive(Debug, Deserialize)]
struct AutomatonStartRequest {
    project_id: String,
    #[serde(default)]
    auth_token: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    git_repo_url: Option<String>,
    #[serde(default)]
    git_branch: Option<String>,
}

#[derive(Debug, Serialize)]
struct AutomatonStartResponse {
    automaton_id: String,
    event_stream_url: String,
}

/// Start a dev-loop or single-task automaton.
/// When `task_id` is provided, runs a single task; otherwise starts the full dev loop.
async fn automaton_start_handler(
    headers: HeaderMap,
    State(state): State<RouterState>,
    Json(req): Json<AutomatonStartRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state
        .automaton_bridge
        .as_ref()
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "automaton controller unavailable"})),
            )
        })?;

    let auth_token = req
        .auth_token
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(String::from)
        });

    let workspace_root = req.workspace_root.map(|s| {
        let path = std::path::PathBuf::from(s);
        state.config.resolve_project_path(&path)
    });

    let automaton_id = if let Some(task_id) = req.task_id {
        bridge
            .run_task(
                &req.project_id,
                &task_id,
                workspace_root,
                auth_token,
                req.model,
                req.git_repo_url,
                req.git_branch,
            )
            .await
    } else {
        bridge
            .start_dev_loop(
                &req.project_id,
                workspace_root,
                auth_token,
                req.model,
                req.git_repo_url,
                req.git_branch,
            )
            .await
    }
    .map_err(|e| {
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(AutomatonStartResponse {
            event_stream_url: format!("/stream/automaton/{automaton_id}"),
            automaton_id,
        }),
    ))
}

/// Get the status of a running automaton.
async fn automaton_status_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    match bridge.get_status(&automaton_id) {
        Some(info) => Ok(Json(serde_json::to_value(&info).unwrap_or_default())),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("automaton {automaton_id} not found")})),
        )),
    }
}

/// List all running automatons.
async fn automaton_list_handler(
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    let list = bridge.list_automatons();
    Ok(Json(
        serde_json::to_value(&list).unwrap_or(serde_json::json!([])),
    ))
}

/// Pause a running automaton.
async fn automaton_pause_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    bridge.pause_by_id(&automaton_id).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    Ok(Json(serde_json::json!({"ok": true, "automaton_id": automaton_id, "status": "paused"})))
}

/// Stop a running automaton.
async fn automaton_stop_handler(
    State(state): State<RouterState>,
    Path(automaton_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let bridge = state.automaton_bridge.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "automaton controller unavailable"})),
        )
    })?;

    bridge.stop_by_id(&automaton_id).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    Ok(Json(serde_json::json!({"ok": true, "automaton_id": automaton_id, "status": "stopped"})))
}

// === WebSocket ===

/// Upgrade an HTTP connection to a WebSocket for interactive agent sessions.
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<RouterState>,
) -> impl IntoResponse {
    let auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from);

    let ctx = WsContext {
        workspace_base: state.config.workspaces_path(),
        provider: state.provider.clone(),
        tool_config: state.tool_config.clone(),
        auth_token,
        catalog: state.catalog.clone(),
        domain_api: state.domain_api.clone(),
        automaton_controller: state.automaton_controller.clone(),
        project_base: state.config.project_base.clone(),
    };
    ws.on_upgrade(move |socket| handle_ws_connection(socket, ctx))
}

/// WebSocket endpoint for streaming automaton events.
///
/// Clients connect to `/stream/automaton/:automaton_id` to receive real-time
/// events from a running automaton (dev loop, task run, etc.).
/// Requires a Bearer token in the Authorization header (same as the chat WS).
async fn automaton_ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Path(automaton_id): Path<String>,
    State(state): State<RouterState>,
) -> impl IntoResponse {
    let _auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from);

    ws.on_upgrade(move |socket| {
        handle_automaton_ws(socket, automaton_id, state.automaton_bridge)
    })
}

async fn handle_automaton_ws(
    socket: axum::extract::ws::WebSocket,
    automaton_id: String,
    bridge: Option<Arc<AutomatonBridge>>,
) {
    use axum::extract::ws::Message as WsMessage;
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();

    let bridge = match bridge {
        Some(b) => b,
        None => {
            let msg = serde_json::json!({"type": "error", "message": "automaton controller unavailable"}).to_string();
            let _: Result<(), _> = ws_tx.send(WsMessage::Text(msg)).await;
            return;
        }
    };

    let mut rx = match bridge.subscribe_events(&automaton_id) {
        Some(rx) => rx,
        None => {
            let msg = serde_json::json!({"type": "error", "message": format!("automaton {automaton_id} not found or already finished")}).to_string();
            let _: Result<(), _> = ws_tx.send(WsMessage::Text(msg)).await;
            return;
        }
    };

    info!(automaton_id = %automaton_id, "Automaton event stream connected");

    // Drain the read side so the WebSocket layer can process ping/pong
    // and close frames. Without this the connection may be dropped by
    // intermediaries that expect pong responses.
    let drain_aid = automaton_id.clone();
    let drain_handle = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(WsMessage::Close(_)) | Err(_) => break,
                _ => continue,
            }
        }
        tracing::debug!(automaton_id = %drain_aid, "Automaton WS read side closed");
    });

    loop {
        match rx.recv().await {
            Ok(event) => {
                let is_done = matches!(event, aura_automaton::AutomatonEvent::Done);
                match serde_json::to_string(&event) {
                    Ok(json) => {
                        if ws_tx.send(WsMessage::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to serialize automaton event");
                    }
                }
                if is_done {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                let msg = serde_json::json!({"type": "warning", "message": format!("dropped {n} events (client too slow)")});
                let _ = ws_tx.send(WsMessage::Text(msg.to_string())).await;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    drain_handle.abort();
    info!(automaton_id = %automaton_id, "Automaton event stream disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::AgentId;
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;
    use axum::body::Body;
    use axum::http::Request;
    use tower_dev::util::ServiceExt;

    fn test_router_state(store: Arc<dyn Store>) -> RouterState {
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("mock"));
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            vec![],
            vec![],
            std::path::PathBuf::from("/tmp/workspaces"),
        ));
        RouterState {
            store,
            scheduler,
            config: NodeConfig::default(),
            provider,
            tool_config: ToolConfig::default(),
            catalog: Arc::new(ToolCatalog::new()),
            domain_api: None,
            automaton_controller: None,
            automaton_bridge: None,
        }
    }

    fn create_test_store() -> Arc<dyn Store> {
        let dir = tempfile::tempdir().unwrap();
        Arc::new(RocksStore::open(dir.path(), false).unwrap())
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["version"].is_string());
    }

    #[tokio::test]
    async fn test_submit_tx_valid() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let payload_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "Hello agent");

        let body = serde_json::json!({
            "agent_id": agent_id.to_hex(),
            "kind": "user_prompt",
            "payload": payload_b64
        });

        let req = Request::builder()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["accepted"].as_bool().unwrap());
        assert!(json["tx_id"].is_string());
    }

    #[tokio::test]
    async fn test_submit_tx_invalid_agent_id() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let body = serde_json::json!({
            "agent_id": "not-hex",
            "kind": "user_prompt",
            "payload": "aGVsbG8="
        });

        let req = Request::builder()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_submit_tx_invalid_kind() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let body = serde_json::json!({
            "agent_id": agent_id.to_hex(),
            "kind": "invalid_kind",
            "payload": "aGVsbG8="
        });

        let req = Request::builder()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_submit_tx_invalid_base64() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let body = serde_json::json!({
            "agent_id": agent_id.to_hex(),
            "kind": "user_prompt",
            "payload": "!!! not base64 !!!"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_head_new_agent() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let req = Request::builder()
            .uri(format!("/agents/{}/head", agent_id.to_hex()))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["head_seq"], 0);
    }

    #[tokio::test]
    async fn test_get_head_invalid_agent_id() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let req = Request::builder()
            .uri("/agents/zzz-bad/head")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_scan_record_empty() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let req = Request::builder()
            .uri(format!("/agents/{}/record", agent_id.to_hex()))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_scan_record_with_query_params() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let agent_id = AgentId::generate();
        let req = Request::builder()
            .uri(format!(
                "/agents/{}/record?from_seq=5&limit=10",
                agent_id.to_hex()
            ))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_scan_record_invalid_agent() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let req = Request::builder()
            .uri("/agents/bad-hex/record")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_submit_tx_all_kinds() {
        let kinds = [
            "user_prompt",
            "agent_msg",
            "trigger",
            "action_result",
            "system",
        ];

        for kind in kinds {
            let store = create_test_store();
            let state = test_router_state(store);
            let app = create_router(state);

            let agent_id = AgentId::generate();
            let payload_b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("payload for {kind}"),
            );

            let body = serde_json::json!({
                "agent_id": agent_id.to_hex(),
                "kind": kind,
                "payload": payload_b64
            });

            let req = Request::builder()
                .method("POST")
                .uri("/tx")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::ACCEPTED,
                "kind '{kind}' should be accepted"
            );
        }
    }

    #[tokio::test]
    async fn test_nonexistent_route_returns_404() {
        let store = create_test_store();
        let state = test_router_state(store);
        let app = create_router(state);

        let req = Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
