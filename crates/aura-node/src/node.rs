//! Node runtime.

use crate::automaton_bridge::AutomatonBridge;
use crate::config::NodeConfig;
use crate::domain::HttpDomainApi;
use crate::router::{create_router, RouterState};
use crate::scheduler::Scheduler;
use anyhow::Context;
use aura_agent::KernelModelGateway;
use aura_automaton::AutomatonRuntime;
use aura_core::AgentId;
use aura_kernel::{Executor, ExecutorRouter, Kernel, KernelConfig};
use aura_memory::{
    ConsolidationConfig, MemoryManager, ProcedureConfig, RefinerConfig, RetrievalConfig,
    WriteConfig,
};
use aura_skills::{SkillInstallStore, SkillLoader, SkillManager};
use aura_store::RocksStore;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::catalog::ToolProfile;
use aura_tools::domain_tools::{DomainApi, DomainToolExecutor};
use aura_tools::{ToolCatalog, ToolConfig};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::net::TcpListener;
use tracing::info;

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

        let db_path = self.config.db_path();
        tokio::fs::create_dir_all(&db_path)
            .await
            .context("creating database directory")?;
        tokio::fs::create_dir_all(self.config.workspaces_path())
            .await
            .context("creating workspaces directory")?;

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

        let store = Arc::new(
            RocksStore::open(&db_path, self.config.sync_writes).context("opening RocksDB store")?,
        );
        info!("Store opened");

        let tool_config = ToolConfig {
            enable_fs: self.config.enable_fs_tools,
            enable_commands: self.config.enable_cmd_tools,
            command_allowlist: self.config.allowed_commands.clone(),
            allow_shell: self.config.allow_shell,
            ..Default::default()
        };
        if tool_config.enable_commands {
            // Empty `command_allowlist` is the ToolConfig contract for
            // "all commands allowed" — log the effective sidecar policy
            // so operators can confirm the autonomous-mode short-circuit
            // resolved, without a UI status pop.
            info!(
                allowed_commands = ?tool_config.command_allowlist,
                allow_shell = tool_config.allow_shell,
                "aura-node run_command enabled"
            );
        }

        let catalog = Arc::new(ToolCatalog::new());
        info!(static_tools = catalog.static_count(), "Tool catalog ready");

        let domain_api: Arc<dyn DomainApi> = Arc::new(HttpDomainApi::new(
            &self.config.aura_storage_url,
            &self.config.aura_network_url,
            &self.config.orbit_url,
            self.config.aura_os_server_url.clone(),
        )?);
        info!(
            storage_url = %self.config.aura_storage_url,
            os_server_url = ?self.config.aura_os_server_url,
            "Domain API ready (JWT auth)"
        );

        let tools = catalog.visible_tools(ToolProfile::Core, &tool_config);
        let domain_exec = Arc::new(DomainToolExecutor::new(domain_api.clone()));
        let resolver =
            crate::executor_factory::build_tool_resolver(&catalog, &tool_config, Some(domain_exec));
        let resolver: Arc<dyn Executor> = Arc::new(resolver);
        let executors = vec![resolver];
        info!("Executors configured");

        let provider = aura_reasoner::default_provider_from_env().provider;

        // Invariant §3: LLM calls performed by the memory subsystem are
        // recorded via a dedicated "memory service" kernel whose agent log
        // is kept distinct from per-user / per-session agent logs.
        let memory_agent_id = AgentId::generate();
        let memory_store: Arc<dyn aura_store::Store> = store.clone();
        let memory_kernel = Arc::new(
            Kernel::new(
                memory_store,
                provider.clone(),
                ExecutorRouter::new(),
                KernelConfig::default(),
                memory_agent_id,
            )
            .context("building memory-service kernel")?,
        );
        let memory_gateway = Arc::new(KernelModelGateway::new(memory_kernel));
        let memory_manager = Arc::new(MemoryManager::new(
            store.db_handle().clone(),
            memory_gateway,
            RefinerConfig::default(),
            WriteConfig::default(),
            RetrievalConfig::default(),
            ConsolidationConfig::default(),
            ProcedureConfig::default(),
        ));
        info!(
            memory_agent_id = %memory_agent_id,
            "Memory manager ready"
        );

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            executors,
            tools,
            self.config.workspaces_path(),
            Some(Arc::clone(&memory_manager)),
        ));
        info!("Scheduler ready");

        let automaton_runtime = Arc::new(AutomatonRuntime::new());
        let automaton_bridge: Option<Arc<AutomatonBridge>> = Some(Arc::new(
            AutomatonBridge::new(
                automaton_runtime.clone(),
                store.clone() as Arc<dyn aura_store::Store>,
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

        let skill_loader = SkillLoader::with_defaults(Some(self.config.workspaces_path()), None);
        let skill_install_store = Arc::new(SkillInstallStore::new(store.db_handle().clone()));
        let skill_manager_inner =
            SkillManager::with_install_store(skill_loader, skill_install_store);
        let skill_count = skill_manager_inner.list_all().len();
        let skill_manager = Arc::new(RwLock::new(skill_manager_inner));
        info!(skills = skill_count, "Skill manager ready");

        let router_url = std::env::var("AURA_ROUTER_URL").ok();

        let state = RouterState::new(crate::router::RouterStateConfig {
            store,
            scheduler,
            config: self.config.clone(),
            provider,
            tool_config,
            catalog,
            domain_api: Some(domain_api),
            automaton_controller,
            automaton_bridge,
            memory_manager: Some(memory_manager),
            skill_manager: Some(skill_manager),
            router_url,
        });
        let app = create_router(state);

        let addr: SocketAddr = self
            .config
            .bind_addr
            .parse()
            .context("parsing bind address")?;
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding TCP listener on {addr}"))?;
        info!(%addr, "HTTP server listening");

        // `into_make_service_with_connect_info::<SocketAddr>()` is
        // required for the tower_governor `PeerIpKeyExtractor` layered
        // inside `create_router` (phase 9 rate limiting). Without it,
        // every request would be rejected with `UnableToExtractKey`.
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .context("running HTTP server")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_new() {
        let config = NodeConfig::default();
        let node = Node::new(config.clone());
        assert_eq!(node.config.bind_addr, config.bind_addr);
    }

    #[test]
    fn test_node_with_defaults() {
        let node = Node::with_defaults();
        assert_eq!(node.config.bind_addr, "127.0.0.1:8080");
    }

    #[test]
    fn test_node_custom_config() {
        let config = NodeConfig {
            bind_addr: "0.0.0.0:9090".to_string(),
            sync_writes: true,
            record_window_size: 100,
            ..NodeConfig::default()
        };
        let node = Node::new(config);
        assert_eq!(node.config.bind_addr, "0.0.0.0:9090");
        assert!(node.config.sync_writes);
        assert_eq!(node.config.record_window_size, 100);
    }

    #[test]
    fn test_node_config_propagation() {
        let config = NodeConfig {
            data_dir: std::path::PathBuf::from("/custom/data"),
            enable_fs_tools: false,
            enable_cmd_tools: true,
            allowed_commands: vec!["ls".to_string(), "cat".to_string()],
            ..NodeConfig::default()
        };
        let node = Node::new(config);
        assert_eq!(
            node.config.data_dir,
            std::path::PathBuf::from("/custom/data")
        );
        assert!(!node.config.enable_fs_tools);
        assert!(node.config.enable_cmd_tools);
        assert_eq!(node.config.allowed_commands.len(), 2);
    }

    #[test]
    fn test_create_model_provider_returns_something() {
        let _provider = aura_reasoner::default_provider_from_env();
    }
}
