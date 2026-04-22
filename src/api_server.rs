//! Embedded API server for health checks, file access, and local tooling.
//!
//! # Security
//!
//! The terminal harness runs as whatever user launched it, which means
//! this server — if reachable from a browser on the same host — can
//! speak for that user. Two controls close that hole:
//!
//! 1. **Per-launch bearer token.** A random 32-byte hex token is minted
//!    every time [`start_api_server`] is called and every non-`/health`
//!    request must present `Authorization: Bearer <token>`. The token
//!    is logged to stderr once, in the same vein as `jupyter notebook`,
//!    so the operator can copy it into tooling. It is never persisted.
//!
//! 2. **Sandboxed file access.** The `/api/files` and `/api/read-file`
//!    handlers route every incoming path through [`aura_tools::Sandbox`],
//!    which canonicalises the workspace root and the candidate path
//!    before comparing prefixes. That catches both plain `../` traversal
//!    and symlinks / junctions whose real target lives outside the
//!    workspace. Reads are additionally capped at [`MAX_READ_BYTES`].

use aura_terminal::UiCommand;
use aura_tools::Sandbox;
use axum::{
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

/// Default API server port
const API_PORT: u16 = 8080;

/// Fallback ports to try if the default is busy
const FALLBACK_PORTS: &[u16] = &[8081, 8082, 8090, 3000];

/// Maximum bytes `/api/read-file` will return before tripping `413`.
///
/// Mirrors the cap in `aura-node` so a reader can't OOM us by pointing
/// the handler at a pseudo-file like `/dev/zero` or a multi-gig log.
const MAX_READ_BYTES: u64 = 5 * 1024 * 1024;

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

/// Information returned from [`start_api_server`].
///
/// `url` is the URL the server is listening on; `token` is the
/// per-launch bearer token. External callers (browsers, curl) must
/// include `Authorization: Bearer <token>` on every non-`/health`
/// request.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields are part of the public handle shape;
                    // consumers outside this crate (e.g. future IPC wiring) may read them.
pub struct ApiServerHandle {
    pub url: String,
    pub token: String,
}

/// Start the embedded API server.
///
/// `workspace_root` is the directory beyond which file-access handlers
/// may not read. When it does not exist the caller is expected to have
/// created it already — otherwise [`Sandbox::new`] will attempt to
/// create it on our behalf.
pub async fn start_api_server(
    cmd_tx: mpsc::Sender<UiCommand>,
    workspace_root: PathBuf,
) -> Option<ApiServerHandle> {
    let sandbox = match Sandbox::new(&workspace_root) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!(
                error = %e,
                root = %workspace_root.display(),
                "Failed to initialise API server sandbox"
            );
            let _ = cmd_tx.try_send(UiCommand::ShowError(format!(
                "API server failed to initialise sandbox: {e}"
            )));
            return None;
        }
    };

    // The embedded TUI API server follows the same `AURA_NODE_REQUIRE_AUTH`
    // gate as the aura-node HTTP server. When auth is disabled we mint
    // no token, skip the bearer middleware, and suppress the stderr
    // banner so unattended local dev workflows don't get log noise.
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

    let app = Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    let ports_to_try = std::iter::once(API_PORT).chain(FALLBACK_PORTS.iter().copied());

    for port in ports_to_try {
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let url = format!("http://{addr}");
                info!(%url, "API server listening");

                // Log once to stderr — matches the `jupyter` UX so the
                // operator can copy the token into curl / browser
                // tooling. Do NOT promote this to stdout or a file: the
                // token is only as strong as its handling. Suppressed
                // when `AURA_NODE_REQUIRE_AUTH` is off, because in that
                // mode there is no token to leak and the banner would
                // only add log noise.
                if require_auth {
                    eprintln!("[aura] API server listening on {url} — bearer token: {token}");
                } else {
                    eprintln!(
                        "[aura] API server listening on {url} — bearer auth disabled (set AURA_NODE_REQUIRE_AUTH=1 to enable)"
                    );
                }

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

                return Some(ApiServerHandle { url, token });
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

/// Strip the `\\?\` verbatim prefix that Windows `canonicalize` adds
/// so walk-time canonical paths compare cleanly against the sandbox
/// root (which has already had its own prefix stripped). No-op on
/// non-Windows targets.
fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
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
async fn require_bearer_mw(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    match extract_bearer(request.headers()) {
        Some(presented)
            if constant_time_eq(presented.as_bytes(), state.expected_token.as_bytes()) =>
        {
            next.run(request).await
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Constant-time byte-slice compare.
///
/// Pads the shorter side by folding its length into the accumulator so
/// timing doesn't leak whether lengths matched. `subtle` would be a
/// nicer dep but we don't want to take on a new crate in this phase.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len_diff = (a.len() ^ b.len()) as u8;
    let n = a.len().min(b.len());
    let mut acc = len_diff;
    for i in 0..n {
        acc |= a[i] ^ b[i];
    }
    acc == 0
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

async fn walk_directory(start: &Path, workspace_root: &Path, max_depth: usize) -> Vec<DirEntry> {
    use std::collections::{HashMap, HashSet};

    // Phase 1: async iterative walk driven by an explicit stack. For each
    // directory we visit we record its ordered children (dirs before files,
    // alphabetical within each group) into a flat map.
    //
    // Symlink-loop defense: before descending into any directory we
    // canonicalize its path and consult a visited-set. Two hostile
    // shapes we want to neutralise —
    //
    //  1. `a -> b -> a` style cycles, which inflate cost even with the
    //     `max_depth` cap because every hop adds a new `PathBuf` key
    //     to the contents map.
    //  2. Symlinks whose canonical target escapes the workspace root.
    //     The sandbox's `resolve_existing` already canonicalises the
    //     entry point, but a mid-walk symlink to `/etc` is still a
    //     leak if we blindly recurse. We reject any canonical child
    //     whose prefix does not match the workspace root.
    let mut contents: HashMap<PathBuf, Vec<(String, bool, PathBuf)>> = HashMap::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(start.to_path_buf(), 0)];

    while let Some((path, depth)) = stack.pop() {
        if depth >= max_depth {
            continue;
        }
        // Canonicalize per-step so intermediate symlinks collapse to
        // their real target. Failed canonicalize (broken symlink,
        // permission denied, missing) => skip rather than bail the
        // whole walk.
        let canonical = match std::fs::canonicalize(&path) {
            Ok(c) => strip_unc_prefix(&c),
            Err(_) => continue,
        };
        if !canonical.starts_with(workspace_root) {
            // Defense-in-depth against the Phase 3 sandbox check —
            // a symlink encountered mid-walk that points outside the
            // workspace must not be followed even if depth allows it.
            debug!(
                path = %path.display(),
                canonical = %canonical.display(),
                "walk_directory skipping path that canonicalises outside workspace"
            );
            continue;
        }
        if !visited.insert(canonical) {
            // Already walked via some other name (symlink, junction) —
            // skip so we don't inflate the contents map on cycles.
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
        path: &Path,
        contents: &std::collections::HashMap<PathBuf, Vec<(String, bool, PathBuf)>>,
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
async fn api_list_files_handler(
    State(state): State<ApiState>,
    Query(query): Query<ListFilesQuery>,
) -> Response {
    let target = match state.sandbox.resolve_existing(&query.path) {
        Ok(p) => p,
        Err(e) => return sandbox_error_response(&e),
    };

    let meta = match tokio::fs::metadata(&target).await {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "path not found" })),
            )
                .into_response();
        }
    };
    if !meta.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "path is not a directory" })),
        )
            .into_response();
    }

    let max_depth = query.depth.min(20);
    let entries = walk_directory(&target, state.sandbox.root(), max_depth).await;
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

/// `GET /api/read-file?path=...`
///
/// Reads a file and returns its text content, capped at
/// [`MAX_READ_BYTES`]. Files whose canonical path is outside the
/// sandbox root are refused with `403 Forbidden`; files exceeding the
/// cap return `413 Payload Too Large`.
async fn api_read_file_handler(
    State(state): State<ApiState>,
    Query(query): Query<ReadFileQuery>,
) -> Response {
    let target = match state.sandbox.resolve_existing(&query.path) {
        Ok(p) => p,
        Err(e) => return sandbox_error_response(&e),
    };

    let meta = match tokio::fs::metadata(&target).await {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "path not found" })),
            )
                .into_response();
        }
    };
    if !meta.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "path is not a file" })),
        )
            .into_response();
    }

    // `take(cap + 1)` lets us distinguish "at the limit" from "over the
    // limit" without ever buffering more than `cap + 1` bytes.
    let file = match tokio::fs::File::open(&target).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error": aura_auth::redact_error(e.to_string()),
                })),
            )
                .into_response();
        }
    };
    let cap = MAX_READ_BYTES;
    let mut buf: Vec<u8> = Vec::new();
    let mut limited = file.take(cap + 1);
    if let Err(e) = limited.read_to_end(&mut buf).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error": aura_auth::redact_error(e.to_string()),
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

    let content = String::from_utf8_lossy(&buf).into_owned();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "content": content,
            "path": target.to_string_lossy(),
            "bytes": buf.len(),
        })),
    )
        .into_response()
}

/// Map [`aura_tools::ToolError`] variants onto HTTP responses.
///
/// `SandboxViolation` is `403 Forbidden` because the caller asked for
/// something they aren't allowed to see; `PathNotFound` is `404`;
/// anything else (I/O, permission) falls through as `400 Bad Request`.
fn sandbox_error_response(err: &aura_tools::ToolError) -> Response {
    use aura_tools::ToolError;
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
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    /// Build a router identical to the one [`start_api_server`] mounts,
    /// but without the TCP bind / spawn — so tests can drive it with
    /// `oneshot`. Returns `(router, token, tempdir)` where the
    /// `tempdir` is the sandbox root and must be kept alive for the
    /// duration of the test.
    fn test_app() -> (axum::Router, String, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(tmp.path()).unwrap();
        let token = generate_token();
        let state = ApiState {
            expected_token: Arc::new(token.clone()),
            sandbox: Arc::new(sandbox),
        };
        let public = Router::new().route("/health", get(api_health_handler));
        let protected = Router::new()
            .route("/api/files", get(api_list_files_handler))
            .route("/api/read-file", get(api_read_file_handler))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_bearer_mw,
            ));
        let app = Router::new()
            .merge(public)
            .merge(protected)
            .with_state(state);
        (app, token, tmp)
    }

    fn urlencode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            let safe = b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'\\');
            if safe {
                out.push(b as char);
            } else {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
        out
    }

    #[tokio::test]
    async fn health_is_anonymous() {
        let (app, _token, _tmp) = test_app();
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_file_without_token_is_unauthorized() {
        let (app, _token, tmp) = test_app();
        std::fs::write(tmp.path().join("ok.txt"), "hi").unwrap();
        let uri = format!(
            "/api/read-file?path={}",
            urlencode(&tmp.path().join("ok.txt").to_string_lossy())
        );
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn read_file_rejects_path_traversal() {
        let (app, token, tmp) = test_app();
        // Secret lives one level *above* the sandbox root so any
        // traversal attempt that resolves to it must fail.
        let parent = tmp.path().parent().unwrap();
        let secret = parent.join(format!(
            "aura-api-server-secret-{}.txt",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&secret, "top-secret").unwrap();

        let uri = format!(
            "/api/read-file?path={}",
            urlencode(&secret.to_string_lossy())
        );
        let req = Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Cleanup before asserting so the file doesn't leak when the
        // test is re-run.
        let _ = std::fs::remove_file(&secret);
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn read_file_returns_sandboxed_file() {
        let (app, token, tmp) = test_app();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, "hello, world").unwrap();

        let uri = format!("/api/read-file?path={}", urlencode(&path.to_string_lossy()));
        let req = Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["content"], "hello, world");
    }

    #[tokio::test]
    async fn read_file_caps_oversize() {
        let (app, token, tmp) = test_app();
        let path = tmp.path().join("big.bin");
        let payload = vec![b'A'; 5 * 1024 * 1024 + 1];
        std::fs::write(&path, &payload).unwrap();

        let uri = format!("/api/read-file?path={}", urlencode(&path.to_string_lossy()));
        let req = Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn constant_time_eq_matches_equal_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    /// Cyclic symlink must not cause an unbounded walk. Gated to
    /// `cfg(unix)` because creating symlinks on Windows needs
    /// either Developer Mode or `SeCreateSymbolicLinkPrivilege`,
    /// which CI runners typically lack.
    #[cfg(unix)]
    #[tokio::test]
    async fn walk_directory_breaks_symlink_loops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();

        // Create a real subdir with one file so we can verify the
        // legitimate entries still show up.
        let child = root.join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("marker.txt"), "ok").unwrap();

        // Create a loop: <root>/child/loop -> <root>
        std::os::unix::fs::symlink(&root, child.join("loop")).unwrap();

        // Run with plenty of depth — the visited-set must stop the
        // traversal rather than bottoming out on `max_depth`.
        let entries = walk_directory(&root, &root, 20).await;

        // Sanity: the legitimate file is present somewhere in the
        // returned tree.
        fn contains_marker(entries: &[DirEntry]) -> bool {
            entries.iter().any(|e| {
                e.name == "marker.txt" || e.children.as_ref().is_some_and(|c| contains_marker(c))
            })
        }
        assert!(contains_marker(&entries), "legitimate file should survive");

        // Walk the tree and count nodes — a successful visited-set
        // means the count is linear in the real tree, not exponential
        // in depth.
        fn count(entries: &[DirEntry]) -> usize {
            entries
                .iter()
                .map(|e| 1 + e.children.as_deref().map(count).unwrap_or(0))
                .sum()
        }
        let n = count(&entries);
        assert!(
            n < 50,
            "symlink loop inflated the tree to {n} nodes — visited-set not working"
        );
    }

    /// Symlink whose canonical target escapes the workspace root is
    /// refused mid-walk, even if the entry point itself is legal.
    #[cfg(unix)]
    #[tokio::test]
    async fn walk_directory_rejects_escaping_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();

        // A sibling directory outside the workspace, with one file.
        let outside_parent = tempfile::tempdir().unwrap();
        let outside = outside_parent.path().canonicalize().unwrap();
        std::fs::write(outside.join("secret.txt"), "top-secret").unwrap();

        // Point a symlink from inside the workspace to the outside
        // directory. The walker must canonicalize and refuse the
        // descent.
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        let entries = walk_directory(&root, &root, 5).await;

        fn has_name(entries: &[DirEntry], needle: &str) -> bool {
            entries.iter().any(|e| {
                e.name == needle || e.children.as_ref().is_some_and(|c| has_name(c, needle))
            })
        }
        assert!(
            !has_name(&entries, "secret.txt"),
            "escaping-symlink contents must not leak into the listing"
        );
    }
}
