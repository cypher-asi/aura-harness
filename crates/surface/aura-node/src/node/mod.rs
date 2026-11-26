//! Node runtime.

mod listener;
mod services;

use crate::config::NodeConfig;
use crate::gateway::{create_router, RouterState};
use anyhow::Context;
use aura_agent::KernelModelGateway;
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_context_memory::{
    ConsolidationConfig, MemoryConfig, MemoryManager, RefinerConfig, DEFAULT_MEMORY_MODEL,
};
use aura_core_types::AgentId;
use aura_engine::automaton::AutomatonBridge;
use aura_engine::scheduler::Scheduler;
use aura_engine_automaton::AutomatonRuntime;
use aura_exec_tools::automaton_tools::AutomatonController;
use aura_exec_tools::domain_tools::DomainApi;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_store_db::{RocksStore, SealCipher};
use listener::{bind_listener, shutdown_signal};
use services::Provider;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

fn configured_memory_model(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_MEMORY_MODEL)
        .to_string()
}

fn memory_provider_overrides(model: &str) -> aura_model::SessionOverrides {
    aura_model::SessionOverrides {
        default_model: Some(model.to_string()),
        // Memory traffic must not inherit a global cross-provider fallback.
        // Keeping the fallback identical also lets the provider deduplicate it.
        fallback_model: Some(model.to_string()),
        ..Default::default()
    }
}

/// The Aura Node runtime.
pub struct Node {
    config: NodeConfig,
}

impl Node {
    /// Create a new node with the given config.
    #[must_use]
    pub const fn new(config: NodeConfig) -> Self {
        Self { config }
    }

    /// Create a node with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(NodeConfig::default())
    }

    /// Run the node.
    ///
    /// # Errors
    /// Returns error if the node fails to start.
    pub async fn run(mut self) -> anyhow::Result<()> {
        info!("Starting Aura Node");
        info!(data_dir = ?self.config.data_dir, "Data directory");

        // Bind the TCP listener FIRST so port collisions fail in
        // milliseconds instead of after the full init sequence
        // (RocksDB open, tool catalog, domain API HTTP client,
        // memory manager, scheduler, automaton runtime, skill
        // loader). Pre-fix the bind was the last step on the happy
        // path, so a stale `aura-node.exe` orphaned from a previous
        // dev run would burn through ~1s of init logs only to fail
        // with a cryptic "Only one usage of each socket address"
        // error. Holding the listener across init also reserves the
        // port so a race between two startups can't both succeed
        // init only to have the later one die at the bind.
        let addr: SocketAddr = self
            .config
            .bind_addr
            .parse()
            .context("parsing bind address")?;
        let listener = bind_listener(addr).await?;

        let db_path = self.create_directories().await?;
        self.resolve_auth_token()?;
        let (store, seal_cipher) = self.open_store(&db_path).await?;

        let plans = Arc::new(crate::cloud_node::PlanResolver::default());
        let state = self.build_router_state(store, seal_cipher.as_ref(), &plans)?;
        state.cloud_node().attach_plans(Arc::clone(&plans));
        if self.config.cloud_node {
            start_claim_socket(&state, &self.config.data_dir, addr.port());
        }
        let app = create_router(state);

        info!(%addr, "HTTP server listening");
        serve(listener, app).await
    }

    /// Create the database and workspaces directories; returns the db path.
    async fn create_directories(&self) -> anyhow::Result<std::path::PathBuf> {
        let db_path = self.config.db_path();
        tokio::fs::create_dir_all(&db_path)
            .await
            .context("creating database directory")?;
        tokio::fs::create_dir_all(self.config.workspaces_path())
            .await
            .context("creating workspaces directory")?;
        Ok(db_path)
    }

    fn resolve_auth_token(&mut self) -> anyhow::Result<()> {
        // Security audit — phase 4. Resolve the bearer secret BEFORE we
        // build the router so every protected route sees the same
        // token. `resolve_auth_token` prefers `AURA_NODE_AUTH_TOKEN`,
        // then a persisted `$data_dir/auth_token` file, then mints a
        // new one (and prints it to stderr exactly once). See
        // `crate::config::resolve_auth_token` for the source-order
        // spec. The token is deliberately *not* logged via `tracing`.
        //
        // Gated on `require_auth` (AURA_NODE_REQUIRE_AUTH env) which
        // defaults to `false`; when disabled we clear the token rather
        // than leaving the `"test"` default in memory, so any code
        // path that accidentally compares against it fails closed.
        if self.config.require_auth {
            self.config.auth_token = crate::config::resolve_auth_token(&self.config.data_dir)
                .context("resolving aura-node auth token")?;
        } else {
            self.config.auth_token.clear();
        }
        Ok(())
    }

    async fn open_store(
        &self,
        db_path: &std::path::Path,
    ) -> anyhow::Result<(Arc<RocksStore>, Option<Arc<SealCipher>>)> {
        // Swarm TEE phase 5 (attest-boot): when AURA_STATE_ENCRYPTION=sealed,
        // fetch the per-agent DEK from the CoCo CDH/KBS (or the dev-mode key
        // file) BEFORE opening or serving any state. This either returns a
        // value cipher or fails the whole startup — sealed mode never falls
        // back to plaintext. In plaintext mode (env unset) `seal_cipher` is
        // `None` and everything below behaves exactly as before.
        // R2: this also runs the encrypt-in-place migration when a legacy
        // agent's plaintext state is found at `db_path` on a sealed boot.
        let seal_cipher = crate::sealing::prepare_state_sealing(&self.config.data_dir, db_path)
            .await
            .context("preparing sealed state mode (refusing to serve without the state DEK)")?;

        let store = Arc::new(
            RocksStore::open_sealed(db_path, self.config.sync_writes, seal_cipher.clone())
                .context("opening RocksDB store")?,
        );
        info!(sealed = seal_cipher.is_some(), "Store opened");
        Ok((store, seal_cipher))
    }
}

/// Build the memory manager, whose LLM calls go through a dedicated
/// memory-service kernel.
fn build_memory_manager(
    store: &Arc<RocksStore>,
    seal_cipher: Option<&Arc<SealCipher>>,
) -> anyhow::Result<Arc<MemoryManager>> {
    let memory_model = configured_memory_model(std::env::var("AURA_MEMORY_MODEL").ok().as_deref());
    let memory_provider =
        aura_model::with_session_overrides(memory_provider_overrides(&memory_model))
            .context("building dedicated memory model provider")?
            .provider;

    // Invariant §3: LLM calls performed by the memory subsystem are
    // recorded via a dedicated "memory service" kernel whose agent log
    // is kept distinct from per-user / per-session agent logs.
    let memory_agent_id = AgentId::generate();
    let memory_store: Arc<dyn aura_store_db::Store> = store.clone();
    let memory_kernel = Arc::new(
        Kernel::new(
            memory_store,
            memory_provider,
            ExecutorRouter::new(),
            KernelConfig::default(),
            memory_agent_id,
        )
        .context("building memory-service kernel")?,
    );
    let memory_gateway = Arc::new(KernelModelGateway::new(memory_kernel));
    let config = MemoryConfig {
        refiner: RefinerConfig {
            model: memory_model.clone(),
            ..Default::default()
        },
        consolidation: ConsolidationConfig {
            model: memory_model.clone(),
            ..Default::default()
        },
        ..MemoryConfig::default()
    };
    let memory_manager = Arc::new(MemoryManager::with_cipher(
        store.db_handle().clone(),
        seal_cipher.cloned(),
        memory_gateway,
        config,
    ));
    info!(
        memory_agent_id = %memory_agent_id,
        memory_model = %memory_model,
        "Memory manager ready"
    );
    Ok(memory_manager)
}

/// Optional automaton bridge plus the controller view of it.
type Automaton = (
    Option<Arc<AutomatonBridge>>,
    Option<Arc<dyn AutomatonController>>,
);

/// Build the automaton runtime bridge over the node store.
fn build_automaton(
    store: &Arc<RocksStore>,
    domain_api: &Arc<dyn DomainApi>,
    provider: &Provider,
    catalog: &Arc<ToolCatalog>,
    tool_config: &ToolConfig,
    scheduler: &Arc<Scheduler>,
) -> Automaton {
    let automaton_runtime = Arc::new(AutomatonRuntime::new());
    let automaton_bridge: Option<Arc<AutomatonBridge>> = Some(Arc::new(
        AutomatonBridge::new(
            automaton_runtime.clone(),
            store.clone() as Arc<dyn aura_store_db::Store>,
            domain_api.clone(),
            provider.clone(),
            catalog.clone(),
            tool_config.clone(),
        )
        .with_scheduler(scheduler.clone()),
    ));
    let automaton_controller: Option<Arc<dyn AutomatonController>> = automaton_bridge
        .clone()
        .map(|b| b as Arc<dyn AutomatonController>);
    if automaton_controller.is_some() {
        info!("Automaton runtime ready");
    }
    (automaton_bridge, automaton_controller)
}

/// Serve HTTP until a shutdown signal arrives.
async fn serve(listener: TcpListener, app: axum::Router) -> anyhow::Result<()> {
    // `into_make_service_with_connect_info::<SocketAddr>()` is
    // required for the tower_governor `PeerIpKeyExtractor` layered
    // inside `create_router` (phase 9 rate limiting). Without it,
    // every request would be rejected with `UnableToExtractKey`.
    //
    // `with_graceful_shutdown(shutdown_signal())` is the other
    // half of the port-leak fix: pre-fix the server awaited
    // forever with no signal handler, so on Windows a Ctrl-C in
    // the foreground `cargo run` console did not propagate to
    // the child `aura-node.exe` (Windows console Ctrl-C
    // semantics are not the Unix signal-group propagation that
    // cargo relies on elsewhere), the child orphaned, and the
    // next `cargo run` collided on port 8080. Listening for
    // Ctrl-C / Ctrl-Break (Windows) and SIGTERM (Unix) lets
    // axum drain in-flight requests and close the listener so
    // the port releases as soon as the user stops the dev run.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("running HTTP server")?;

    Ok(())
}

/// Serve the cloud node's claim socket (`aura node claim --stdin`).
#[cfg(unix)]
fn start_claim_socket(state: &RouterState, data_dir: &std::path::Path, port: u16) {
    let node = state.cloud_node();
    let path = crate::cloud_node::claim_socket_path(data_dir);
    tokio::spawn(async move {
        if let Err(err) = crate::cloud_node::serve_claims(node, path, port).await {
            tracing::error!(error = %err, "cloud node claim socket failed; the hub cannot claim this node");
        }
    });
}

#[cfg(not(unix))]
fn start_claim_socket(_state: &RouterState, _data_dir: &std::path::Path, _port: u16) {
    tracing::warn!("cloud nodes need a Unix socket for their claim; AURA_CLOUD_NODE ignored");
}

#[cfg(test)]
mod tests;
