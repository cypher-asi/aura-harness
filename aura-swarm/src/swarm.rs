//! Swarm runtime.

use crate::config::SwarmConfig;
use crate::router::{create_router, RouterState};
use crate::scheduler::Scheduler;
use aura_executor::Executor;
use aura_reasoner::{AnthropicConfig, AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolConfig, ToolExecutor, ToolRegistry};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// The Aura Swarm runtime.
pub struct Swarm {
    config: SwarmConfig,
}

impl Swarm {
    /// Create a new swarm with the given config.
    #[must_use]
    pub const fn new(config: SwarmConfig) -> Self {
        Self { config }
    }

    /// Create a swarm with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(SwarmConfig::default())
    }

    /// Run the swarm.
    ///
    /// # Errors
    /// Returns error if the swarm fails to start.
    pub async fn run(self) -> anyhow::Result<()> {
        info!("Starting Aura Swarm");
        info!(data_dir = ?self.config.data_dir, "Data directory");

        tokio::fs::create_dir_all(self.config.db_path()).await?;
        tokio::fs::create_dir_all(self.config.workspaces_path()).await?;

        let store = Arc::new(RocksStore::open(
            self.config.db_path(),
            self.config.sync_writes,
        )?);
        info!("Store opened");

        let tool_config = ToolConfig {
            enable_fs: self.config.enable_fs_tools,
            enable_commands: self.config.enable_cmd_tools,
            command_allowlist: self.config.allowed_commands.clone(),
            ..Default::default()
        };
        let tool_executor: Arc<dyn Executor> = Arc::new(ToolExecutor::new(tool_config.clone()));
        let executors = vec![tool_executor];
        info!("Executors configured");

        let tool_registry = DefaultToolRegistry::new();
        let tools = tool_registry.list();

        let provider = Self::create_model_provider();

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            executors,
            tools,
            self.config.workspaces_path(),
        ));
        info!("Scheduler ready");

        let state = RouterState {
            store,
            scheduler,
            config: self.config.clone(),
            provider,
            tool_config,
        };
        let app = create_router(state);

        let addr: SocketAddr = self.config.bind_addr.parse()?;
        let listener = TcpListener::bind(addr).await?;
        info!(%addr, "HTTP server listening");

        axum::serve(listener, app).await?;

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
    fn test_swarm_new() {
        let config = SwarmConfig::default();
        let swarm = Swarm::new(config.clone());
        assert_eq!(swarm.config.bind_addr, config.bind_addr);
    }

    #[test]
    fn test_swarm_with_defaults() {
        let swarm = Swarm::with_defaults();
        assert_eq!(swarm.config.bind_addr, "127.0.0.1:8080");
    }

    #[test]
    fn test_swarm_custom_config() {
        let config = SwarmConfig {
            bind_addr: "0.0.0.0:9090".to_string(),
            sync_writes: true,
            record_window_size: 100,
            ..SwarmConfig::default()
        };
        let swarm = Swarm::new(config);
        assert_eq!(swarm.config.bind_addr, "0.0.0.0:9090");
        assert!(swarm.config.sync_writes);
        assert_eq!(swarm.config.record_window_size, 100);
    }

    #[test]
    fn test_swarm_config_propagation() {
        let config = SwarmConfig {
            data_dir: std::path::PathBuf::from("/custom/data"),
            enable_fs_tools: false,
            enable_cmd_tools: true,
            allowed_commands: vec!["ls".to_string(), "cat".to_string()],
            ..SwarmConfig::default()
        };
        let swarm = Swarm::new(config);
        assert_eq!(
            swarm.config.data_dir,
            std::path::PathBuf::from("/custom/data")
        );
        assert!(!swarm.config.enable_fs_tools);
        assert!(swarm.config.enable_cmd_tools);
        assert_eq!(swarm.config.allowed_commands.len(), 2);
    }

    #[test]
    fn test_create_model_provider_returns_something() {
        let _provider = Swarm::create_model_provider();
    }
}
