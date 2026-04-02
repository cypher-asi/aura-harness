//! Node runtime.

use crate::automaton_bridge::AutomatonBridge;
use crate::config::NodeConfig;
use crate::domain::HttpDomainApi;
use crate::router::{create_router, RouterState};
use crate::scheduler::Scheduler;
use anyhow::Context;
use aura_automaton::AutomatonRuntime;
use aura_kernel::Executor;
use aura_reasoner::{AnthropicConfig, AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::catalog::ToolProfile;
use aura_tools::domain_tools::{DomainApi, DomainToolExecutor};
use aura_tools::{ToolCatalog, ToolConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, warn};

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
    pub async fn run(self) -> anyhow::Result<()> {
        info!("Starting Aura Node");
        info!(data_dir = ?self.config.data_dir, "Data directory");

        tokio::fs::create_dir_all(self.config.db_path())
            .await
            .context("creating database directory")?;
        tokio::fs::create_dir_all(self.config.workspaces_path())
            .await
            .context("creating workspaces directory")?;

        let store = Arc::new(
            RocksStore::open(self.config.db_path(), self.config.sync_writes)
                .context("opening RocksDB store")?,
        );
        info!("Store opened");

        let tool_config = ToolConfig {
            enable_fs: self.config.enable_fs_tools,
            enable_commands: self.config.enable_cmd_tools,
            command_allowlist: self.config.allowed_commands.clone(),
            ..Default::default()
        };

        let catalog = Arc::new(ToolCatalog::new());
        info!(static_tools = catalog.static_count(), "Tool catalog ready");

        let domain_api: Arc<dyn DomainApi> = Arc::new(HttpDomainApi::new(
            &self.config.aura_storage_url,
            &self.config.aura_network_url,
            &self.config.orbit_url,
        ));
        info!(
            storage_url = %self.config.aura_storage_url,
            "Domain API ready (JWT auth)"
        );

        let tools = catalog.visible_tools(ToolProfile::Core, &tool_config);
        let domain_exec = Arc::new(DomainToolExecutor::new(domain_api.clone()));
        let resolver =
            crate::executor_factory::build_tool_resolver(&catalog, &tool_config, Some(domain_exec));
        let resolver: Arc<dyn Executor> = Arc::new(resolver);
        let executors = vec![resolver];
        info!("Executors configured");

        let provider = Self::create_model_provider();

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            executors,
            tools,
            self.config.workspaces_path(),
        ));
        info!("Scheduler ready");

        let automaton_runtime = Arc::new(AutomatonRuntime::new());
        let automaton_bridge: Option<Arc<AutomatonBridge>> = Some(Arc::new(AutomatonBridge::new(
            automaton_runtime.clone(),
            store.clone() as Arc<dyn aura_store::Store>,
            domain_api.clone(),
            provider.clone(),
            catalog.clone(),
            tool_config.clone(),
        )));
        let automaton_controller: Option<Arc<dyn AutomatonController>> = automaton_bridge
            .clone()
            .map(|b| b as Arc<dyn AutomatonController>);
        if automaton_controller.is_some() {
            info!("Automaton runtime ready");
        }

        let state = RouterState {
            store,
            scheduler,
            config: self.config.clone(),
            provider,
            tool_config,
            catalog,
            domain_api: Some(domain_api),
            automaton_controller,
            automaton_bridge,
            failed_txs: Arc::new(dashmap::DashMap::new()),
            memory_manager: None,
            skill_manager: None,
        };
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

        axum::serve(listener, app)
            .await
            .context("running HTTP server")?;

        Ok(())
    }

    /// Create a `ModelProvider` for WebSocket sessions.
    ///
    /// Tries `AnthropicProvider` from environment, falls back to `MockProvider`.
    fn create_model_provider() -> Arc<dyn ModelProvider + Send + Sync> {
        match AnthropicConfig::from_env() {
            Ok(config) => {
                let mode_label = if config.routing_mode == aura_reasoner::RoutingMode::Proxy {
                    "proxy"
                } else {
                    "direct"
                };
                match AnthropicProvider::new(config) {
                    Ok(provider) => {
                        info!(mode = mode_label, "LLM provider ready ({mode_label} mode)");
                        Arc::new(provider)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to create LLM provider, using mock");
                        Arc::new(MockProvider::simple_response("(mock provider)"))
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "LLM provider not configured, using mock");
                Arc::new(MockProvider::simple_response("(mock provider)"))
            }
        }
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
        let _provider = Node::create_model_provider();
    }
}
