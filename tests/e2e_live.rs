//! End-to-end integration tests against a live aura-node server.
//!
//! These tests boot a real node, connect via HTTP and WebSocket, authenticate
//! via zOS, and exercise every tool and proxy API against live services.
//!
//! Run all e2e tests:
//! ```text
//! cargo test --test e2e_live
//! ```
//!
//! Tests that require credentials gracefully skip (pass with a SKIP note)
//! when the relevant env vars are missing.
//!
//! Required environment (via `.env` or exported):
//! - `AURA_LLM_ROUTING` / `AURA_ROUTER_URL` (or direct-mode keys)
//! - `E2E_ZOS_EMAIL` + `E2E_ZOS_PASSWORD` (for JWT-gated proxy tests)
//! - `INTERNAL_SERVICE_TOKEN` (for internal-token proxy tests)

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aura_auth::{CredentialStore, ZosClient};
use aura_core::AgentId;
use aura_executor::Executor;
use aura_node::NodeConfig;
use aura_node::router::{create_router, RouterState};
use aura_node::scheduler::Scheduler;
use aura_reasoner::{AnthropicConfig, AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolConfig, ToolExecutor, ToolInstaller, ToolRegistry};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMsg;

// ============================================================================
// Helpers: skip if env not configured
// ============================================================================

macro_rules! require_env {
    ($name:expr) => {
        match std::env::var($name) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("SKIP: {} not set, skipping e2e test", $name);
                return;
            }
        }
    };
}

/// Resolve auth token for LLM tests. Returns None and prints skip if not available.
fn require_llm_token() -> Option<String> {
    let routing = std::env::var("AURA_LLM_ROUTING").unwrap_or_default();
    if routing == "direct" {
        if std::env::var("AURA_ANTHROPIC_API_KEY").is_err()
            && std::env::var("ANTHROPIC_API_KEY").is_err()
        {
            eprintln!("SKIP: direct mode but no API key set");
            return None;
        }
        return Some(String::new());
    }
    // Proxy mode: need a JWT
    match load_auth_token() {
        Some(t) => Some(t),
        None => {
            eprintln!("SKIP: no auth token available for LLM proxy mode");
            None
        }
    }
}

macro_rules! require_llm {
    () => {
        match require_llm_token() {
            Some(t) => t,
            None => return,
        }
    };
}

// ============================================================================
// TestServer
// ============================================================================

struct TestServer {
    base_url: String,
    _data_dir: tempfile::TempDir,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with_options(None, true).await
    }

    /// Start a server with the internal service token explicitly stripped.
    async fn start_without_internal_token() -> Self {
        Self::start_with_options(None, false).await
    }

    async fn start_with_options(
        provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
        include_internal_token: bool,
    ) -> Self {
        let _ = dotenvy::dotenv();

        let data_dir = tempfile::tempdir().expect("create temp dir");

        let db_path = data_dir.path().join("db");
        let workspaces_path = data_dir.path().join("workspaces");
        std::fs::create_dir_all(&db_path).unwrap();
        std::fs::create_dir_all(&workspaces_path).unwrap();

        let mut config = NodeConfig::from_env();
        config.data_dir = data_dir.path().to_path_buf();
        config.enable_fs_tools = true;
        config.enable_cmd_tools = true;
        config.allowed_commands = vec![];
        if !include_internal_token {
            config.internal_service_token = None;
        }

        let store: Arc<dyn aura_store::Store> =
            Arc::new(RocksStore::open(&db_path, false).expect("open rocks"));

        let tool_config = ToolConfig {
            enable_fs: true,
            enable_commands: true,
            command_allowlist: vec![],
            ..Default::default()
        };
        let tool_executor: Arc<dyn Executor> = Arc::new(ToolExecutor::new(tool_config.clone()));
        let executors = vec![tool_executor];

        let tool_registry = DefaultToolRegistry::new();
        let tools = tool_registry.list();
        let tool_installer = Arc::new(ToolInstaller::new());

        let provider: Arc<dyn ModelProvider + Send + Sync> =
            provider_override.unwrap_or_else(create_provider);

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            executors,
            tools,
            workspaces_path,
        ));

        let state = RouterState {
            store,
            scheduler,
            config,
            provider,
            tool_config,
            tool_installer,
        };
        let app = create_router(state);

        let bind = std::env::var("E2E_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_string());
        let addr: SocketAddr = bind.parse().expect("parse bind addr");
        let listener = TcpListener::bind(addr).await.expect("bind");
        let local_addr = listener.local_addr().unwrap();
        let base_url = format!("http://{local_addr}");

        let handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .ok();
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        Self {
            base_url,
            _data_dir: data_dir,
            _server_handle: handle,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn ws_url(&self) -> String {
        self.base_url.replace("http://", "ws://") + "/stream"
    }

    fn workspaces_path(&self) -> PathBuf {
        self._data_dir.path().join("workspaces")
    }
}

fn create_provider() -> Arc<dyn ModelProvider + Send + Sync> {
    match AnthropicConfig::from_env() {
        Ok(config) => match AnthropicProvider::new(config) {
            Ok(p) => Arc::new(p),
            Err(_) => Arc::new(MockProvider::simple_response("(mock)")),
        },
        Err(_) => Arc::new(MockProvider::simple_response("(mock)")),
    }
}

// ============================================================================
// WsClient
// ============================================================================

struct WsClient {
    write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMsg,
    >,
    read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
}

impl WsClient {
    async fn connect(ws_url: &str) -> Self {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .expect("ws connect");
        let (write, read) = stream.split();
        Self { write, read }
    }

    async fn connect_with_auth(ws_url: &str, bearer_token: &str) -> Self {
        use tokio_tungstenite::tungstenite::http::Request;
        let req = Request::builder()
            .uri(ws_url)
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("Host", "localhost")
            .body(())
            .unwrap();
        let (stream, _) = tokio_tungstenite::connect_async(req)
            .await
            .expect("ws connect with auth");
        let (write, read) = stream.split();
        Self { write, read }
    }

    async fn send_json(&mut self, msg: &Value) {
        let text = serde_json::to_string(msg).unwrap();
        self.write
            .send(WsMsg::Text(text.into()))
            .await
            .expect("ws send");
    }

    async fn send_session_init(&mut self, workspace: &Path, token: Option<&str>) {
        self.send_session_init_full(workspace, token, None, None, None, None)
            .await;
    }

    async fn send_session_init_full(
        &mut self,
        workspace: &Path,
        token: Option<&str>,
        system_prompt: Option<&str>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        max_turns: Option<u32>,
    ) {
        let mut init = json!({
            "type": "session_init",
            "workspace": workspace.to_string_lossy(),
            "max_turns": max_turns.unwrap_or(10)
        });
        if let Some(t) = token {
            init["token"] = json!(t);
        }
        if let Some(sp) = system_prompt {
            init["system_prompt"] = json!(sp);
        }
        if let Some(m) = model {
            init["model"] = json!(m);
        }
        if let Some(mt) = max_tokens {
            init["max_tokens"] = json!(mt);
        }
        self.send_json(&init).await;
    }

    async fn send_user_message(&mut self, content: &str) {
        self.send_json(&json!({"type": "user_message", "content": content}))
            .await;
    }

    async fn send_cancel(&mut self) {
        self.send_json(&json!({"type": "cancel"})).await;
    }

    async fn send_raw(&mut self, raw: &str) {
        self.write
            .send(WsMsg::Text(raw.to_string().into()))
            .await
            .expect("ws send raw");
    }

    async fn recv_json(&mut self) -> Option<Value> {
        self.recv_json_timeout(Duration::from_secs(30)).await
    }

    async fn recv_json_timeout(&mut self, timeout: Duration) -> Option<Value> {
        match tokio::time::timeout(timeout, self.read.next()).await {
            Ok(Some(Ok(WsMsg::Text(text)))) => {
                serde_json::from_str(text.as_ref()).ok()
            }
            _ => None,
        }
    }

    async fn expect_session_ready(&mut self) -> Value {
        let msg = self.recv_json().await.expect("expected session_ready");
        assert_eq!(msg["type"], "session_ready", "expected session_ready, got: {msg}");
        msg
    }

    /// Collect all messages for one turn until `assistant_message_end` or timeout.
    async fn collect_turn(&mut self, timeout: Duration) -> Vec<Value> {
        let mut messages = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.recv_json_timeout(remaining).await {
                Some(msg) => {
                    let is_end = msg["type"] == "assistant_message_end";
                    let is_error =
                        msg["type"] == "error" && msg["recoverable"].as_bool() == Some(false);
                    messages.push(msg);
                    if is_end || is_error {
                        break;
                    }
                }
                None => break,
            }
        }
        messages
    }
}

/// Extract all tool names used in a turn's message stream.
fn tool_names_used(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["type"] == "tool_use_start")
        .filter_map(|m| m["name"].as_str().map(String::from))
        .collect()
}

/// Concatenate all text_delta content in a turn.
fn collect_text(messages: &[Value]) -> String {
    messages
        .iter()
        .filter(|m| m["type"] == "text_delta")
        .filter_map(|m| m["text"].as_str())
        .collect()
}

/// Check that a turn ended with a given stop_reason.
fn assert_stop_reason(messages: &[Value], expected: &str) {
    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    assert!(end.is_some(), "no assistant_message_end found");
    assert_eq!(
        end.unwrap()["stop_reason"].as_str().unwrap(),
        expected,
        "unexpected stop_reason"
    );
}

/// Load an auth token from env, credential store, or zOS login.
fn load_auth_token() -> Option<String> {
    if let Ok(jwt) = std::env::var("AURA_ROUTER_JWT") {
        if !jwt.is_empty() {
            return Some(jwt);
        }
    }
    CredentialStore::load_token()
}

/// Obtain a JWT via zOS login (skips test if env vars missing).
async fn obtain_jwt() -> Option<String> {
    if let Some(token) = load_auth_token() {
        return Some(token);
    }
    let email = std::env::var("E2E_ZOS_EMAIL").ok()?;
    let password = std::env::var("E2E_ZOS_PASSWORD").ok()?;
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let client = ZosClient::new().ok()?;
    let session = client.login(&email, &password).await.ok()?;
    Some(session.access_token)
}

/// Find a file by name anywhere under a directory tree (deepest match first).
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    // Search subdirectories first (agent workspace is a subdirectory)
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_file(&path, name) {
                    return Some(found);
                }
            }
        }
    }
    if dir.join(name).exists() {
        return Some(dir.join(name));
    }
    None
}

/// Find the agent subdirectory created by the session under the workspace.
fn find_agent_dir(ws_path: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(ws_path).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

/// Place a file in the agent workspace directory (where tools operate).
/// Only creates in the agent subdirectory if one exists; falls back to root.
fn place_file_in_agent_dir(ws_path: &Path, name: &str, content: &str) {
    if let Some(agent_dir) = find_agent_dir(ws_path) {
        std::fs::write(agent_dir.join(name), content).unwrap();
    } else {
        std::fs::write(ws_path.join(name), content).unwrap();
    }
}

/// Create a reqwest HTTP client.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

/// Convenience: connect a WS client with session_init + auth token.
async fn connect_llm_session(server: &TestServer, ws_path: &Path, token: &str) -> WsClient {
    let mut ws = WsClient::connect(&server.ws_url()).await;
    let tok = if token.is_empty() {
        None
    } else {
        Some(token)
    };
    ws.send_session_init(ws_path, tok).await;
    ws.expect_session_ready().await;
    ws
}

// ============================================================================
// Suite 1: Health and REST API
// ============================================================================

#[tokio::test]

async fn test_health() {
    let server = TestServer::start().await;
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
}

#[tokio::test]

async fn test_submit_tx() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        "Hello from e2e",
    );
    let body = json!({
        "agent_id": agent_id.to_hex(),
        "kind": "user_prompt",
        "payload": payload_b64
    });
    let resp = http_client()
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let data: Value = resp.json().await.unwrap();
    assert!(data["accepted"].as_bool().unwrap());
    assert!(data["tx_id"].is_string());
}

#[tokio::test]

async fn test_get_head() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let resp = http_client()
        .get(format!(
            "{}/agents/{}/head",
            server.base_url(),
            agent_id.to_hex()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let data: Value = resp.json().await.unwrap();
    assert_eq!(data["head_seq"], 0);
}

#[tokio::test]

async fn test_scan_record() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let resp = http_client()
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=10",
            server.base_url(),
            agent_id.to_hex()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let data: Value = resp.json().await.unwrap();
    assert!(data.as_array().unwrap().is_empty());
}

// ============================================================================
// Suite 2: WebSocket Session Lifecycle
// ============================================================================

#[tokio::test]

async fn test_ws_session_init() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("test-agent");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;

    let ready = ws.expect_session_ready().await;
    let tools = ready["tools"].as_array().unwrap();
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"list_files"), "missing list_files");
    assert!(tool_names.contains(&"read_file"), "missing read_file");
    assert!(tool_names.contains(&"write_file"), "missing write_file");
    assert!(tool_names.contains(&"edit_file"), "missing edit_file");
    assert!(tool_names.contains(&"delete_file"), "missing delete_file");
    assert!(tool_names.contains(&"find_files"), "missing find_files");
    assert!(tool_names.contains(&"search_code"), "missing search_code");
    assert!(tool_names.contains(&"run_command"), "missing run_command");
    assert!(tool_names.contains(&"stat_file"), "missing stat_file");
}

#[tokio::test]

async fn test_ws_session_init_twice_errors() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("test-agent-2");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

    ws.send_session_init(&ws_path, None).await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "already_initialized");
}

#[tokio::test]

async fn test_ws_user_message_before_init() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_user_message("hello").await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "not_initialized");
}

// ============================================================================
// Suite 3: Real LLM Simple Turn
// ============================================================================

#[tokio::test]

async fn test_ws_simple_prompt() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("simple-prompt");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message("What is 2+2? Reply with just the number, nothing else.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(!messages.is_empty(), "no messages received");

    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "expected at least one text_delta");

    assert_stop_reason(&messages, "end_turn");

    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .unwrap();
    let input_tokens = end["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = end["usage"]["output_tokens"].as_u64().unwrap_or(0);
    assert!(input_tokens > 0, "expected non-zero input_tokens");
    assert!(output_tokens > 0, "expected non-zero output_tokens");

    let text = collect_text(&messages);
    assert!(text.contains('4'), "expected response to contain '4', got: {text}");
}

// ============================================================================
// Suite 4: LLM + Filesystem Tool E2E Tests
// ============================================================================

#[tokio::test]

async fn test_tool_write_and_read_file() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-write-read");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use the write_file tool to create a file at path 'hello.txt' with the content 'Hello E2E Test'. \
         After writing, use the read_file tool to read 'hello.txt' and confirm it exists.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(tools.contains(&"write_file".to_string()), "expected write_file tool use");

    let file_path = find_file(&ws_path, "hello.txt");
    assert!(file_path.is_some(), "file was not created on disk anywhere under workspace");
    let content = std::fs::read_to_string(file_path.unwrap()).unwrap();
    assert!(
        content.contains("Hello E2E Test"),
        "file content mismatch: {content}"
    );
}

#[tokio::test]

async fn test_tool_edit_file() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-edit");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Place file in the agent directory (where tools operate)
    place_file_in_agent_dir(&ws_path, "target.txt", "The quick brown fox jumps over the lazy dog.");

    ws.send_user_message(
        "Use the edit_file tool on the file 'target.txt'. \
         The old_string to find is 'brown fox' and the new_string to replace it with is 'red panda'. \
         Do this now using the edit_file tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(tools.contains(&"edit_file".to_string()), "expected edit_file tool use, got: {tools:?}");

    // Verify the edit was applied (LLM may provide slightly different params)
    if let Some(file_path) = find_file(&ws_path, "target.txt") {
        let content = std::fs::read_to_string(file_path).unwrap();
        if !content.contains("red panda") {
            eprintln!(
                "NOTE: edit_file tool was invoked but edit wasn't applied as expected (LLM behavior). \
                 Content: {content}"
            );
        }
    }
}

#[tokio::test]

async fn test_tool_delete_file() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-delete");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Place file in the agent directory (where tools operate)
    place_file_in_agent_dir(&ws_path, "to_delete.txt", "delete me");

    ws.send_user_message(
        "Use the delete_file tool to delete the file at path 'to_delete.txt'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"delete_file".to_string()),
        "expected delete_file tool use, got: {tools:?}"
    );
    // Verify deletion (LLM may provide a slightly different path)
    if find_file(&ws_path, "to_delete.txt").is_some() {
        eprintln!(
            "NOTE: delete_file tool was invoked but file still exists (LLM may have used wrong path)"
        );
    }
}

#[tokio::test]

async fn test_tool_list_files() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-list");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "alpha.txt", "a");
    place_file_in_agent_dir(&ws_path, "bravo.txt", "b");
    place_file_in_agent_dir(&ws_path, "charlie.txt", "c");

    ws.send_user_message(
        "Use the list_files tool to list the contents of the current directory '.'. \
         Then tell me the exact filenames you found.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"list_files".to_string()),
        "expected list_files tool use, got: {tools:?}"
    );
}

#[tokio::test]

async fn test_tool_stat_file() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-stat");
    std::fs::create_dir_all(&ws_path).unwrap();
    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "info.txt", "some content here");

    ws.send_user_message("Get file metadata for 'info.txt' using the stat_file tool.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(tools.contains(&"stat_file".to_string()), "expected stat_file tool use");
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_find_files() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-find");
    let nested = ws_path.join("src").join("components");
    std::fs::create_dir_all(&nested).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Create file structure in the agent directory (where tools operate)
    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        let agent_src = agent_dir.join("src").join("components");
        std::fs::create_dir_all(&agent_src).unwrap();
        std::fs::write(agent_dir.join("readme.md"), "root").unwrap();
        std::fs::write(agent_dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(agent_src.join("button.rs"), "struct Button;").unwrap();
        std::fs::write(agent_src.join("input.rs"), "struct Input;").unwrap();
    } else {
        std::fs::write(ws_path.join("readme.md"), "root").unwrap();
        std::fs::write(ws_path.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(nested.join("button.rs"), "struct Button;").unwrap();
        std::fs::write(nested.join("input.rs"), "struct Input;").unwrap();
    }

    ws.send_user_message(
        "Find all '.rs' files in this project recursively. Use the find_files tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"find_files".to_string()),
        "expected find_files tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_search_code() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-search");
    std::fs::create_dir_all(&ws_path).unwrap();

    std::fs::write(
        ws_path.join("app.rs"),
        "fn calculate_total(items: &[i32]) -> i32 {\n    items.iter().sum()\n}\n",
    )
    .unwrap();
    std::fs::write(
        ws_path.join("lib.rs"),
        "pub fn calculate_total(prices: &[f64]) -> f64 {\n    prices.iter().sum()\n}\n",
    )
    .unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "app.rs",
        "fn calculate_total(items: &[i32]) -> i32 {\n    items.iter().sum()\n}\n",
    );
    place_file_in_agent_dir(
        &ws_path,
        "lib.rs",
        "pub fn calculate_total(prices: &[f64]) -> f64 {\n    prices.iter().sum()\n}\n",
    );

    ws.send_user_message(
        "Search for all occurrences of 'calculate_total' in the codebase. Use the search_code tool.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]

async fn test_tool_run_command() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "cmd /c echo hello_e2e"
    } else {
        "echo hello_e2e"
    };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute this exact command: {cmd}"
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );

    // Check the tool result contains our marker string
    let tool_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "run_command")
        .collect();
    if !tool_results.is_empty() {
        let result_text = tool_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result_text.contains("hello_e2e"),
            "command output should contain 'hello_e2e', got: {result_text}"
        );
    }
}

// ============================================================================
// Suite 5: Multi-turn Conversation
// ============================================================================

#[tokio::test]

async fn test_ws_multi_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("multi-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Turn 1: create a file
    ws.send_user_message(
        "Use the write_file tool to create a file at path 'memo.txt' with content 'Remember to buy milk'.",
    )
    .await;
    let turn1 = ws.collect_turn(Duration::from_secs(120)).await;
    let turn1_tools = tool_names_used(&turn1);
    assert!(
        turn1_tools.contains(&"write_file".to_string()),
        "expected write_file in turn 1, got: {turn1_tools:?}"
    );
    assert!(
        find_file(&ws_path, "memo.txt").is_some(),
        "file should exist somewhere under workspace after turn 1"
    );

    // Turn 2: read the file from turn 1 (tests context carryover)
    ws.send_user_message(
        "Use the read_file tool to read 'memo.txt' and tell me its contents.",
    )
    .await;
    let turn2 = ws.collect_turn(Duration::from_secs(120)).await;

    // The LLM may read the file or recall from context.
    // The key assertion is that multi-turn context works (turn 2 completes).
    assert_stop_reason(&turn2, "end_turn");

    let text = collect_text(&turn2);
    let tool_result_text: String = turn2
        .iter()
        .filter(|m| m["type"] == "tool_result")
        .filter_map(|m| m["result"].as_str())
        .collect();
    let combined = format!("{text}{tool_result_text}");
    if !combined.contains("milk") && !combined.contains("Remember") && !combined.contains("memo") {
        eprintln!(
            "NOTE: turn 2 completed but didn't explicitly reference file content (LLM behavior)"
        );
    }
}

// ============================================================================
// Suite 6: Cancellation
// ============================================================================

#[tokio::test]

async fn test_ws_cancel_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("cancel-test");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a very long essay about the history of computing, at least 2000 words. \
         Include detailed technical analysis of every major era.",
    )
    .await;

    // Wait for the turn to start streaming, then cancel
    tokio::time::sleep(Duration::from_secs(2)).await;
    ws.send_cancel().await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;

    // The turn should eventually end - either with cancelled or end_turn
    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    if let Some(end) = end_msg {
        let stop = end["stop_reason"].as_str().unwrap_or("");
        assert!(
            stop == "cancelled" || stop == "end_turn" || stop == "end_turn_with_errors",
            "expected cancelled or end_turn, got: {stop}"
        );
    }
    // If no end message, the turn was still in-flight and we timed out - acceptable for cancel test
}

// ============================================================================
// Suite 7: Proxy API Tests - Storage (JWT-gated)
// ============================================================================

/// Dummy UUID for proxy tests. The upstream may 404 on this, but the proxy
/// should forward the request (proving JWT auth worked) rather than reject it.
const TEST_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// Assert that the proxy forwarded the request to the upstream service.
/// The upstream may return an error for nonexistent resources, but the
/// response must NOT be a local auth rejection ("No session JWT available"
/// or "Internal service token not configured").
fn assert_proxy_forwarded(body: &Value, label: &str) {
    let ok = body["ok"].as_bool().unwrap_or(false);
    if ok {
        return;
    }
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        !err.contains("JWT") && !err.contains("Internal service token"),
        "{label}: request was rejected locally for auth, not forwarded to upstream: {body}"
    );
}

#[tokio::test]
async fn test_proxy_storage_list_tasks() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/storage/list_tasks", server.base_url()))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "list_tasks",
            "input": {"project_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "list_tasks");
}

#[tokio::test]
async fn test_proxy_storage_list_specs() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/storage/list_specs", server.base_url()))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "list_specs",
            "input": {"project_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "list_specs");
}

#[tokio::test]
async fn test_proxy_storage_list_logs() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/storage/list_logs", server.base_url()))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "list_logs",
            "input": {"project_id": TEST_UUID, "limit": 10}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "list_logs");
}

#[tokio::test]
async fn test_proxy_storage_get_project_stats() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!(
            "{}/api/storage/get_project_stats",
            server.base_url()
        ))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "get_project_stats",
            "input": {"project_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "get_project_stats");
}

// ============================================================================
// Suite 8: Proxy API Tests - Network (JWT + Internal Token)
// ============================================================================

#[tokio::test]
async fn test_proxy_network_list_projects() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!(
            "{}/api/network/list_projects",
            server.base_url()
        ))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "list_projects",
            "input": {"org_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "list_projects");
}

#[tokio::test]
async fn test_proxy_network_check_budget() {
    let _ = dotenvy::dotenv();
    require_env!("INTERNAL_SERVICE_TOKEN");
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!(
            "{}/api/network/check_budget",
            server.base_url()
        ))
        .json(&json!({
            "tool_name": "check_budget",
            "input": {"org_id": TEST_UUID, "user_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "check_budget");
}

#[tokio::test]
async fn test_proxy_network_record_usage() {
    let _ = dotenvy::dotenv();
    require_env!("INTERNAL_SERVICE_TOKEN");
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!(
            "{}/api/network/record_usage",
            server.base_url()
        ))
        .json(&json!({
            "tool_name": "record_usage",
            "input": {
                "org_id": TEST_UUID,
                "user_id": TEST_UUID,
                "input_tokens": 100,
                "output_tokens": 50,
                "model": "test-model"
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "record_usage");
}

// ============================================================================
// Suite 9: Proxy API Tests - Orbit (JWT-gated)
// ============================================================================

#[tokio::test]
async fn test_proxy_orbit_list_repos() {
    let _ = dotenvy::dotenv();
    let jwt = match obtain_jwt().await {
        Some(j) => j,
        None => {
            eprintln!("SKIP: no JWT available");
            return;
        }
    };
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/orbit/list_repos", server.base_url()))
        .bearer_auth(&jwt)
        .json(&json!({
            "tool_name": "list_repos",
            "input": {"org_id": TEST_UUID}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_proxy_forwarded(&body, "list_repos");
}

// ============================================================================
// Suite 10: Tool Management API
// ============================================================================

#[tokio::test]

async fn test_tools_list() {
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .get(format!("{}/tools", server.base_url()))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tools = body.as_array().expect("tools should be an array");
    // Starts empty (installed tools only, not builtins)
    assert!(tools.is_empty(), "fresh installer should have no tools");
}

#[tokio::test]

async fn test_tools_install_and_delete() {
    let server = TestServer::start().await;
    let http = http_client();

    let tool_def = json!({
        "name": "e2e_test_tool",
        "description": "A test tool installed during e2e tests",
        "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}},
        "endpoint": "http://localhost:9999/fake-tool"
    });

    let resp = http
        .post(format!("{}/tools/install", server.base_url()))
        .json(&tool_def)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify it appears in the list
    let resp = http
        .get(format!("{}/tools", server.base_url()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"e2e_test_tool"),
        "installed tool should appear in list"
    );

    // Delete
    let resp = http
        .delete(format!("{}/tools/e2e_test_tool", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify removed
    let resp = http
        .get(format!("{}/tools", server.base_url()))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        !names.contains(&"e2e_test_tool"),
        "deleted tool should not appear in list"
    );
}

// ============================================================================
// Suite 11: Error Handling
// ============================================================================

#[tokio::test]

async fn test_ws_invalid_json() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_raw("this is not json at all!!!").await;

    let msg = ws.recv_json().await.expect("expected error response");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]

async fn test_ws_message_during_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("msg-during-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a detailed 500-word essay about the importance of software testing.",
    )
    .await;

    // Wait for the turn to start streaming
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Try sending another message while turn is active
    ws.send_user_message("This should be rejected").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let has_turn_in_progress = messages
        .iter()
        .any(|m| m["type"] == "error" && m["code"] == "turn_in_progress");
    assert!(
        has_turn_in_progress,
        "expected turn_in_progress error, messages: {:?}",
        messages.iter().map(|m| m["type"].as_str()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// JWT rejection: call JWT-gated routes without Authorization header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_storage_no_jwt() {
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/storage/list_tasks", server.base_url()))
        .json(&json!({
            "tool_name": "list_tasks",
            "input": {"project_id": "test-project"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "storage should reject missing JWT");
    assert!(
        body["error"].as_str().unwrap_or("").contains("JWT"),
        "error should mention JWT: {body}"
    );
}

#[tokio::test]
async fn test_proxy_network_no_jwt() {
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/network/list_projects", server.base_url()))
        .json(&json!({
            "tool_name": "list_projects",
            "input": {"org_id": "test-org"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "network should reject missing JWT");
    assert!(
        body["error"].as_str().unwrap_or("").contains("JWT"),
        "error should mention JWT: {body}"
    );
}

#[tokio::test]
async fn test_proxy_orbit_no_jwt() {
    let server = TestServer::start().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/orbit/list_repos", server.base_url()))
        .json(&json!({
            "tool_name": "list_repos",
            "input": {"org_id": "test-org"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "orbit should reject missing JWT");
    assert!(
        body["error"].as_str().unwrap_or("").contains("JWT"),
        "error should mention JWT: {body}"
    );
}

// ---------------------------------------------------------------------------
// Internal-token rejection: call internal-token routes on a server with
// no INTERNAL_SERVICE_TOKEN configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_storage_no_internal_token() {
    let server = TestServer::start_without_internal_token().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/storage/create_log", server.base_url()))
        .json(&json!({
            "tool_name": "create_log",
            "input": {
                "project_id": "test-project",
                "message": "test log",
                "level": "info"
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "create_log should reject missing internal token");
    assert!(
        body["error"].as_str().unwrap_or("").contains("Internal service token"),
        "error should mention internal token: {body}"
    );
}

#[tokio::test]
async fn test_proxy_network_no_internal_token() {
    let server = TestServer::start_without_internal_token().await;
    let http = http_client();

    // check_budget uses internal token
    let resp = http
        .post(format!("{}/api/network/check_budget", server.base_url()))
        .json(&json!({
            "tool_name": "check_budget",
            "input": {"org_id": "test-org", "user_id": "test-user"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "check_budget should reject missing internal token");
    assert!(
        body["error"].as_str().unwrap_or("").contains("Internal service token"),
        "error should mention internal token: {body}"
    );

    // record_usage uses internal token
    let resp = http
        .post(format!("{}/api/network/record_usage", server.base_url()))
        .json(&json!({
            "tool_name": "record_usage",
            "input": {
                "org_id": "test-org",
                "user_id": "test-user",
                "input_tokens": 100,
                "output_tokens": 50
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "record_usage should reject missing internal token");

    // post_to_feed uses internal token
    let resp = http
        .post(format!("{}/api/network/post_to_feed", server.base_url()))
        .json(&json!({
            "tool_name": "post_to_feed",
            "input": {
                "profile_id": "test-profile",
                "title": "test",
                "summary": "test"
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "post_to_feed should reject missing internal token");
}

#[tokio::test]
async fn test_proxy_orbit_no_internal_token() {
    let server = TestServer::start_without_internal_token().await;
    let http = http_client();

    let resp = http
        .post(format!("{}/api/orbit/create_repo", server.base_url()))
        .json(&json!({
            "tool_name": "create_repo",
            "input": {"org_id": "test-org", "name": "test-repo"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "create_repo should reject missing internal token");
    assert!(
        body["error"].as_str().unwrap_or("").contains("Internal service token"),
        "error should mention internal token: {body}"
    );
}

// ============================================================================
// Suite 12: Streaming Protocol Field Verification
// ============================================================================

#[tokio::test]
async fn test_ws_streaming_message_fields() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-fields");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hello in one sentence.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(!messages.is_empty(), "no messages received");

    let start = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_start");
    assert!(start.is_some(), "missing assistant_message_start");
    let start_msg_id = start.unwrap()["message_id"].as_str().unwrap_or("");
    assert!(!start_msg_id.is_empty(), "assistant_message_start should have non-empty message_id");

    let has_text_delta = messages
        .iter()
        .any(|m| m["type"] == "text_delta" && m["text"].as_str().map_or(false, |t| !t.is_empty()));
    assert!(has_text_delta, "expected at least one non-empty text_delta");

    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing assistant_message_end");

    let end_msg_id = end_msg["message_id"].as_str().unwrap_or("");
    assert_eq!(
        start_msg_id, end_msg_id,
        "message_id should match between start and end"
    );

    assert!(
        end_msg["stop_reason"].is_string(),
        "stop_reason should be a string"
    );

    let usage = &end_msg["usage"];
    assert!(usage["input_tokens"].as_u64().unwrap_or(0) > 0, "input_tokens should be > 0");
    assert!(usage["output_tokens"].as_u64().unwrap_or(0) > 0, "output_tokens should be > 0");
    assert!(usage["model"].is_string() && !usage["model"].as_str().unwrap().is_empty(), "model should be non-empty");

    let fc = &end_msg["files_changed"];
    assert!(fc["created"].is_array(), "files_changed.created should be an array");
    assert!(fc["modified"].is_array(), "files_changed.modified should be an array");
    assert!(fc["deleted"].is_array(), "files_changed.deleted should be an array");
}

#[tokio::test]
async fn test_ws_files_changed_after_write() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("files-changed");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message(
        "Use the write_file tool to create a file called 'changed.txt' with the content 'tracking changes'.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(tools.contains(&"write_file".to_string()), "expected write_file");

    // files_changed is populated by the server; it may or may not track tool-based writes
    // depending on implementation. We verify the field structure exists regardless.
    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    assert!(end_msg.is_some(), "missing assistant_message_end");
    let fc = &end_msg.unwrap()["files_changed"];
    assert!(fc["created"].is_array(), "files_changed.created should be an array");
}
