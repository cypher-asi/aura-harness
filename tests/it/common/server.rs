//! In-process `TestServer` booting the node router on a local port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aura_agent_kernel::Executor;
use aura_exec_tools::catalog::ToolProfile;
use aura_exec_tools::{ToolCatalog, ToolConfig, ToolResolver};
use aura_model::{AnthropicConfig, AnthropicProvider, MockProvider, ModelProvider};
use aura_node::test_support::{create_router, RouterState, RouterStateConfig, Scheduler};
use aura_node::NodeConfig;
use aura_store_db::RocksStore;
use tokio::net::TcpListener;

use super::auth::E2E_TEST_BEARER;

pub struct TestServer {
    base_url: String,
    base_ws_url: String,
    _data_dir: tempfile::TempDir,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with_options(None).await
    }

    /// Boot a `TestServer` with bearer auth *disabled*, matching the
    /// default `AURA_NODE_REQUIRE_AUTH` behaviour. Used by
    /// [`TestServer::start_without_auth`] callers that want to
    /// verify the router still accepts unauthenticated requests when
    /// the gate is off.
    pub async fn start_without_auth() -> Self {
        Self::start_inner(None, false).await
    }

    pub async fn start_with_options(
        provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
    ) -> Self {
        Self::start_inner(provider_override, true).await
    }

    async fn start_inner(
        provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
        require_auth: bool,
    ) -> Self {
        let _ = aura_config::env_file::load();

        let data_dir = tempfile::tempdir().expect("create temp dir");

        let db_path = data_dir.path().join("db");
        let workspaces_path = data_dir.path().join("workspaces");
        std::fs::create_dir_all(&db_path).unwrap();
        std::fs::create_dir_all(&workspaces_path).unwrap();

        let config = test_node_config(data_dir.path(), require_auth);
        let state = build_router_state(&db_path, workspaces_path, config, provider_override);
        let app = create_router(state);

        let (base_url, base_ws_url, handle) = serve(app).await;

        Self {
            base_url,
            base_ws_url,
            _data_dir: data_dir,
            _server_handle: handle,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Base WS URL (without any path). Use [`Self::stream_url`] to
    /// build the `/stream/:run_id` URL for a specific run, or
    /// [`open_chat_run`] / [`open_chat_run_anonymous`] to do both
    /// the `POST /v1/run` + WS attach in one call.
    pub fn base_ws_url(&self) -> &str {
        &self.base_ws_url
    }

    pub fn stream_url(&self, run_id: &str) -> String {
        format!("{}/stream/{run_id}", self.base_ws_url)
    }

    pub fn workspaces_path(&self) -> PathBuf {
        self._data_dir.path().join("workspaces")
    }
}

pub(crate) fn test_node_config(data_dir: &Path, require_auth: bool) -> NodeConfig {
    // Built from the defaults, never `NodeConfig::from_env`: the test
    // server reads no node settings from the machine it runs on. A cloud
    // machine sets `AURA_CLOUD_NODE`, `AURA_NODE_REQUIRE_AUTH` and
    // `AURA_WORKSPACE` for its own node, which turned this server into a
    // cloud node answering 503 where the tests expect 400. A developer's
    // shell can set `AURA_PROJECT_BASE` (path validation),
    // `AURA_OS_SERVER_URL` (`send_to_agent` calls out to it) or
    // `AURA_ALLOW_UNRESTRICTED_FULL_ACCESS`, which the suites assert
    // against too. See `hermetic_config_ignores_the_machines_node_settings`.
    let mut config = NodeConfig {
        data_dir: data_dir.to_path_buf(),
        ..NodeConfig::default()
    };
    // Phase 4 (security audit): the router's bearer middleware now
    // does a constant-time compare against `config.auth_token`.
    // Force the deterministic test value so `http_client()` and
    // `WsClient::connect()` (both of which send `E2E_TEST_BEARER`)
    // pass through, regardless of whether the ambient environment
    // has `AURA_NODE_AUTH_TOKEN` set to something else.
    //
    // Auth is *off by default* at the node level — tests that rely
    // on it have to opt in. `start_with_options` / `start` flip
    // `require_auth = true` so the router re-attaches
    // `require_bearer_mw`; [`TestServer::start_without_auth`] leaves the
    // gate closed to exercise the default path.
    config.require_auth = require_auth;
    if require_auth {
        config.auth_token = E2E_TEST_BEARER.to_string();
    } else {
        config.auth_token.clear();
    }
    config
}

fn build_router_state(
    db_path: &Path,
    workspaces_path: PathBuf,
    config: NodeConfig,
    provider_override: Option<Arc<dyn ModelProvider + Send + Sync>>,
) -> RouterState {
    let store: Arc<dyn aura_store_db::Store> =
        Arc::new(RocksStore::open(db_path, false).expect("open rocks"));

    let tool_config = ToolConfig {
        command: aura_exec_tools::CommandPolicy {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let catalog = Arc::new(ToolCatalog::new());
    let tools = catalog.visible_tools(ToolProfile::Core, &tool_config);
    let resolver: Arc<dyn Executor> =
        Arc::new(ToolResolver::new(catalog.clone(), tool_config.clone()));
    let executors = vec![resolver];

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        provider_override.unwrap_or_else(create_provider);

    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        executors,
        tools,
        workspaces_path,
        None,
    ));

    RouterState::new(RouterStateConfig {
        store,
        scheduler,
        config,
        provider,
        tool_config,
        catalog,
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        secrets_vault: None,
        process_store: None,
        trigger_registrar: None,
        router_url: None,
    })
}

/// Bind the listener, spawn `axum::serve` and return
/// `(base_url, base_ws_url, handle)` once the server had a moment to start.
async fn serve(app: axum::Router) -> (String, String, tokio::task::JoinHandle<()>) {
    let bind = std::env::var("E2E_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let addr: SocketAddr = bind.parse().expect("parse bind addr");
    let listener = TcpListener::bind(addr).await.expect("bind");
    let local_addr = listener.local_addr().unwrap();
    let base_url = format!("http://{local_addr}");
    let base_ws_url = format!("ws://{local_addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    (base_url, base_ws_url, handle)
}

pub fn create_provider() -> Arc<dyn ModelProvider + Send + Sync> {
    match AnthropicProvider::new(AnthropicConfig::from_env()) {
        Ok(p) => Arc::new(p),
        Err(_) => Arc::new(MockProvider::simple_response("(mock)")),
    }
}

/// Create a TestServer that always uses MockProvider.
pub async fn start_mock_server() -> TestServer {
    let mock: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("Hello from mock provider."));
    TestServer::start_with_options(Some(mock)).await
}

/// The node settings a machine's environment may carry, each set to a
/// value the test server must not take.
const HOSTILE_ENV: &[(&str, &str)] = &[
    ("AURA_CLOUD_NODE", "1"),
    ("AURA_NODE_REQUIRE_AUTH", "1"),
    ("AURA_NODE_AUTH_TOKEN", "machine-token-0123456789"),
    ("AURA_WORKSPACE", "/machine/workspace"),
    ("AURA_DATA_DIR", "/machine/data"),
    ("AURA_LISTEN_ADDR", "0.0.0.0:8080"),
    ("AURA_PROJECT_BASE", "/machine/projects"),
    ("AURA_OS_SERVER_URL", "http://machine:19847"),
    ("AURA_STORAGE_URL", "http://machine-storage"),
    ("AURA_NETWORK_URL", "http://machine-network"),
    ("AURA_SWARM_INTERNAL_TOKEN", "machine-swarm-token"),
    ("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS", "1"),
    ("SYNC_WRITES", "1"),
    ("RECORD_WINDOW_SIZE", "7"),
];

/// Run under [`HOSTILE_ENV`] by
/// [`hermetic_config_ignores_the_machines_node_settings`], which sets the
/// variables for a child process only, as the tests share one.
#[test]
#[ignore = "run by hermetic_config_ignores_the_machines_node_settings"]
fn hermetic_config_probe() {
    let dir = tempfile::tempdir().unwrap();
    let defaults = NodeConfig::default();
    for require_auth in [true, false] {
        let config = test_node_config(dir.path(), require_auth);
        assert!(!config.cloud_node);
        assert_eq!(config.workspace, None);
        assert_eq!(config.data_dir, dir.path());
        assert_eq!(config.require_auth, require_auth);
        let token = if require_auth { E2E_TEST_BEARER } else { "" };
        assert_eq!(config.auth_token, token);
        assert_eq!(config.project_base, None);
        assert_eq!(config.aura_os_server_url, None);
        assert_eq!(config.aura_storage_url, defaults.aura_storage_url);
        assert_eq!(config.aura_network_url, defaults.aura_network_url);
        assert_eq!(config.swarm_internal_token, None);
        assert_eq!(
            config.allow_unrestricted_full_access,
            defaults.allow_unrestricted_full_access
        );
        assert_eq!(config.sync_writes, defaults.sync_writes);
        assert_eq!(config.record_window_size, defaults.record_window_size);
        assert_eq!(config.bind_addr, defaults.bind_addr);
    }
}

#[test]
fn hermetic_config_ignores_the_machines_node_settings() {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            "common::server::hermetic_config_probe",
            "--ignored",
        ])
        .envs(HOSTILE_ENV.iter().copied())
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && shown.contains("1 passed"),
        "{shown}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
