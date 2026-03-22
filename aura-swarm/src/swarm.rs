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

        std::fs::create_dir_all(self.config.db_path())?;
        std::fs::create_dir_all(self.config.workspaces_path())?;

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
            Ok(config) => match AnthropicProvider::new(config) {
                Ok(provider) => {
                    info!("Anthropic model provider ready for WebSocket sessions");
                    Arc::new(provider)
                }
                Err(e) => {
                    warn!(error = %e, "Failed to create Anthropic provider, using mock");
                    Arc::new(MockProvider::simple_response("(mock provider)"))
                }
            },
            Err(_) => {
                warn!("No Anthropic API key configured, WebSocket sessions will use mock provider");
                Arc::new(MockProvider::simple_response("(mock provider)"))
            }
        }
    }
}
