//! Embedded HTTP server for TUI mode with `/health` plus bearer-gated file APIs.
//!
//! The terminal harness runs as whatever user launched it, which means
//! this server — if reachable from a browser on the same host — can
//! speak for that user. Two controls close that hole:
//!
//! 1. **Per-launch bearer token.** A random 32-byte hex token is minted
//!    every time [`start_api_server`] is called and every non-`/health`
//!    request must present `Authorization: Bearer <token>`. The token
//!    is shown once in the TUI chat so the operator can copy it into
//!    tooling. It is never persisted.
//!
//! 2. **Sandboxed file access.** The `/api/files` and `/api/read-file`
//!    handlers route every incoming path through [`aura_exec_tools::Sandbox`],
//!    which canonicalises the workspace root and the candidate path
//!    before comparing prefixes. That catches both plain `../` traversal
//!    and symlinks / junctions whose real target lives outside the
//!    workspace.
//!
//! The directory-walking and capped-read logic lives in
//! [`aura_exec_tools::files_api`] — the TUI server and the aura-node HTTP
//! router share the same implementation so a change to the ignore list
//! or the read cap only needs to land in one place. This module owns
//! the bearer middleware, the sandbox-aware path resolution, and the
//! wire-format mapping from [`aura_exec_tools::files_api::WalkedEntry`] onto
//! the TUI JSON contract.

use aura_auth_kit::bearer::check_bearer;
use aura_exec_tools::files_api::{self, ReadOutcome, WalkedEntry, MAX_READ_BYTES, MAX_WALK_DEPTH};
use aura_exec_tools::Sandbox;
use aura_ui::UiCommand;
use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

/// Default API server port
const API_PORT: u16 = 8080;

/// Fallback ports to try if the default is busy
const FALLBACK_PORTS: &[u16] = &[8081, 8082, 8090, 3000];

/// State shared by the embedded server's handlers.
///
/// Holds the per-launch bearer token and the sandbox used to clamp all
/// file-access paths to the workspace root. Both are wrapped in `Arc`
/// so `axum::Router::with_state` can clone cheaply per request.
#[derive(Clone)]
struct ApiState {
    /// Expected bearer token (constant-time compared against the header).
    expected_token: Arc<String>,
    /// Canonicalising path sandbox scoped to the workspace root.
    sandbox: Arc<Sandbox>,
}

/// Start the embedded API server.
///
/// `workspace_root` is the directory beyond which file-access handlers
/// may not read. When it does not exist the caller is expected to have
/// created it already — otherwise [`Sandbox::new`] will attempt to
/// create it on our behalf.
pub async fn start_api_server(cmd_tx: mpsc::Sender<UiCommand>, workspace_root: PathBuf) {
    let Some(sandbox) = init_sandbox(&cmd_tx, &workspace_root) else {
        return;
    };

    // The embedded TUI API server follows the same `AURA_NODE_REQUIRE_AUTH`
    // gate as the aura-node HTTP server. When auth is disabled we mint
    // no token and skip the bearer middleware.
    let require_auth = auth_required_from_env();
    let token = if require_auth {
        generate_token()
    } else {
        String::new()
    };
    let state = ApiState {
        expected_token: Arc::new(token.clone()),
        sandbox,
    };
    let app = build_router(state, require_auth);

    let ports_to_try = std::iter::once(API_PORT).chain(FALLBACK_PORTS.iter().copied());

    for port in ports_to_try {
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let url = format!("http://{addr}");
                info!(%url, "API server listening");
                announce_listening(&cmd_tx, require_auth, &token, port, &url);

                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, app).await {
                        error!(error = %e, "API server error");
                    }
                });

                return;
            }
            Err(e) => {
                debug!(port = port, error = %e, "Port unavailable, trying next");
            }
        }
    }

    report_all_ports_busy(&cmd_tx);
}

/// Build the workspace sandbox, reporting a failure to the UI.
fn init_sandbox(cmd_tx: &mpsc::Sender<UiCommand>, workspace_root: &Path) -> Option<Arc<Sandbox>> {
    match Sandbox::new(workspace_root) {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            error!(
                error = %e,
                root = %workspace_root.display(),
                "Failed to initialise API server sandbox"
            );
            let _ = cmd_tx.try_send(UiCommand::ShowError(format!(
                "API server failed to initialise sandbox: {e}"
            )));
            None
        }
    }
}

/// The public `/health` route merged with the (optionally bearer-gated)
/// file routes.
fn build_router(state: ApiState, require_auth: bool) -> Router {
    // `/health` stays anonymous for liveness probes; everything else
    // sits behind the bearer middleware. Using `route_layer` scopes
    // the middleware to the matched routes, so `merge`ing with the
    // public router leaves `/health` untouched.
    let public = Router::new().route("/health", get(api_health_handler));
    let protected = Router::new()
        .route("/api/files", get(api_list_files_handler))
        .route("/api/read-file", get(api_read_file_handler));
    let protected = if require_auth {
        protected.route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_mw,
        ))
    } else {
        protected
    };

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Tell the UI the server is up: token (once), port fallback, status.
fn announce_listening(
    cmd_tx: &mpsc::Sender<UiCommand>,
    require_auth: bool,
    token: &str,
    port: u16,
    url: &str,
) {
    // The TUI owns the screen, so writing to stdout/stderr here
    // would corrupt it. The URL is shown in the header; the
    // token goes into the chat once so the operator can copy
    // it. Do NOT write it to a file: the token is only as
    // strong as its handling.
    if require_auth {
        let _ = cmd_tx.try_send(UiCommand::ShowMessage(aura_ui::events::MessageData {
            role: aura_ui::events::MessageRole::System,
            content: format!("API server bearer token: {token}"),
            is_streaming: false,
            timestamp_ms: None,
            command: None,
            activity: None,
            model: None,
            usage: None,
            sources: Vec::new(),
            boot: None,
        }));
    }

    if port != API_PORT {
        let _ = cmd_tx.try_send(UiCommand::ShowWarning(format!(
            "Port {API_PORT} busy, API server using port {port}"
        )));
    }

    let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
        url: Some(url.to_string()),
        active: true,
    });
}

/// Every candidate port was taken.
fn report_all_ports_busy(cmd_tx: &mpsc::Sender<UiCommand>) {
    warn!("Failed to start API server on any port");
    let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
        url: None,
        active: false,
    });
    let _ = cmd_tx.try_send(UiCommand::ShowError(
        "API server failed to start - all ports busy".to_string(),
    ));
}

/// Whether the embedded API server should enforce bearer-token auth.
///
/// Reads `AURA_NODE_REQUIRE_AUTH` — the same gate that controls the
/// aura-node router — and treats `1` / `true` (case-insensitive) as
/// "enable auth". Any other value, including unset, means auth is
/// disabled. Keeping the TUI API server aligned with `aura-node`
/// means local dev operators can toggle a single env var instead of
/// juggling two.
fn auth_required_from_env() -> bool {
    std::env::var("AURA_NODE_REQUIRE_AUTH").is_ok_and(|v| {
        let trimmed = v.trim();
        trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
    })
}

/// Generate a random 32-byte hex token (~256 bits of entropy).
///
/// Uses two `uuid::Uuid::new_v4()` values concatenated so we don't have
/// to pull in `rand` just for this — `uuid` is already a workspace
/// dependency and v4 UUIDs are cryptographically random on every
/// supported platform.
fn generate_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

/// Axum middleware enforcing a constant-time bearer-token check.
///
/// `/health` is not behind this layer (liveness / readiness probes
/// should remain anonymous). Every other route requires the
/// `Authorization: Bearer <token>` header. Returns `401 UNAUTHORIZED`
/// on missing, malformed, or wrong token — distinguishing these cases
/// would leak whether a particular token-length probe had succeeded.
///
/// The actual parsing and constant-time compare live in
/// [`aura_auth_kit::bearer::check_bearer`], shared with the aura-node HTTP
/// router so the two servers can't drift on edge cases (empty secrets,
/// prefix-length probing, etc.).
async fn require_bearer_mw(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    match check_bearer(request.headers(), &state.expected_token) {
        Ok(_) => next.run(request).await,
        Err(status) => status.into_response(),
    }
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

/// Wire shape for `/api/files` (TUI variant).
///
/// Paths are reported as absolute strings (matching the pre-consolidation
/// behaviour) because the TUI workspace panel resolves them directly.
/// The `aura-node` HTTP router uses a different DTO with workspace-
/// relative paths — the two JSON contracts are deliberately separate
/// and each caller owns its own mapping away from
/// [`aura_exec_tools::files_api::WalkedEntry`].
#[derive(serde::Serialize)]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Self>>,
}

fn to_dir_entries(entries: Vec<WalkedEntry>) -> Vec<DirEntry> {
    entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.abs_path.to_string_lossy().into_owned(),
            is_dir: e.is_dir,
            children: e.children.map(to_dir_entries),
        })
        .collect()
}

/// `GET /api/files?path=...&depth=...`
///
/// Lists directory contents recursively, returning a tree of
/// [`DirEntry`] objects. Depth is clamped to
/// [`aura_exec_tools::files_api::MAX_WALK_DEPTH`].
async fn api_list_files_handler(
    State(state): State<ApiState>,
    Query(query): Query<ListFilesQuery>,
) -> Response {
    let target = match state.sandbox.resolve_existing(&query.path) {
        Ok(p) => p,
        Err(e) => return sandbox_error_response(&e),
    };

    let Ok(meta) = tokio::fs::metadata(&target).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "path not found" })),
        )
            .into_response();
    };
    if !meta.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "path is not a directory" })),
        )
            .into_response();
    }

    let max_depth = query.depth.min(MAX_WALK_DEPTH);
    let walked = files_api::walk_directory(&target, Some(state.sandbox.root()), max_depth).await;
    let entries = to_dir_entries(walked);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "entries": entries })),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct ReadFileQuery {
    path: String,
}

/// `404` when `target` is missing, `400` when it is not a regular file.
async fn ensure_file(target: &Path) -> Result<(), Response> {
    let Ok(meta) = tokio::fs::metadata(target).await else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "path not found" })),
        )
            .into_response());
    };
    if !meta.is_file() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "path is not a file" })),
        )
            .into_response());
    }
    Ok(())
}

/// `GET /api/read-file?path=...`
///
/// Reads a file and returns its text content, capped at
/// [`aura_exec_tools::files_api::MAX_READ_BYTES`]. Files whose canonical path
/// is outside the sandbox root are refused with `403 Forbidden`; files
/// exceeding the cap return `413 Payload Too Large`.
async fn api_read_file_handler(
    State(state): State<ApiState>,
    Query(query): Query<ReadFileQuery>,
) -> Response {
    let target = match state.sandbox.resolve_existing(&query.path) {
        Ok(p) => p,
        Err(e) => return sandbox_error_response(&e),
    };

    if let Err(response) = ensure_file(&target).await {
        return response;
    }

    match files_api::read_file_capped(&target, MAX_READ_BYTES).await {
        Ok(ReadOutcome::Ok { bytes }) => {
            let content = String::from_utf8_lossy(&bytes).into_owned();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "content": content,
                    "path": target.to_string_lossy(),
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
                "error": aura_auth::redact_error(e.to_string()),
            })),
        )
            .into_response(),
    }
}

/// Map [`aura_exec_tools::ToolError`] variants onto HTTP responses.
///
/// `SandboxViolation` is `403 Forbidden` because the caller asked for
/// something they aren't allowed to see; `PathNotFound` is `404`;
/// anything else (I/O, permission) falls through as `400 Bad Request`.
fn sandbox_error_response(err: &aura_exec_tools::ToolError) -> Response {
    use aura_exec_tools::ToolError;
    match err {
        ToolError::SandboxViolation { .. } => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "ok": false, "error": "path escapes workspace" })),
        )
            .into_response(),
        ToolError::PathNotFound(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "path not found" })),
        )
            .into_response(),
        other => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": other.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests;
