//! Swarm runtime.

use crate::config::SwarmConfig;
use crate::router::{create_router, RouterState};
use crate::scheduler::Scheduler;
use aura_executor::ExecutorRouter;
use aura_kernel::{Kernel, KernelConfig, PolicyConfig};
use aura_reasoner::{HttpReasoner, MockReasoner, Reasoner, ReasonerConfig};
use aura_store::RocksStore;
use aura_tools::{ToolConfig, ToolExecutor};
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

        // Ensure directories exist
        std::fs::create_dir_all(self.config.db_path())?;
        std::fs::create_dir_all(self.config.workspaces_path())?;

        // Create store
        let store = Arc::new(RocksStore::open(
            self.config.db_path(),
            self.config.sync_writes,
        )?);
        info!("Store opened");

        // Create reasoner
        let _reasoner: Arc<dyn Reasoner> = if self.config.reasoner_url.is_empty() {
            warn!("No reasoner URL configured, using mock reasoner");
            Arc::new(MockReasoner::empty())
        } else {
            let reasoner_config = ReasonerConfig {
                gateway_url: self.config.reasoner_url.clone(),
                timeout_ms: self.config.reasoner_timeout_ms,
                max_retries: 2,
            };
            match HttpReasoner::new(reasoner_config) {
                Ok(r) => Arc::new(r),
                Err(e) => {
                    warn!(error = %e, "Failed to create HTTP reasoner, using mock");
                    Arc::new(MockReasoner::empty())
                }
            }
        };

        // Create executor router with tools
        let mut executor = ExecutorRouter::new();
        let tool_config = ToolConfig {
            enable_fs: self.config.enable_fs_tools,
            enable_commands: self.config.enable_cmd_tools,
            command_allowlist: self.config.allowed_commands.clone(),
            ..Default::default()
        };
        executor.add_executor(Arc::new(ToolExecutor::new(tool_config)));
        info!("Executors configured");

        // Create kernel
        let kernel_config = KernelConfig {
            record_window_size: self.config.record_window_size,
            policy: PolicyConfig::default(),
            workspace_base: self.config.workspaces_path(),
            replay_mode: false,
        };

        // We need to use a concrete type for the kernel
        // For now, use MockReasoner since we can't easily use dyn Reasoner
        self.run_with_mock_reasoner(store, executor, kernel_config)
            .await
    }

    async fn run_with_mock_reasoner(
        self,
        store: Arc<RocksStore>,
        executor: ExecutorRouter,
        kernel_config: KernelConfig,
    ) -> anyhow::Result<()> {
        let reasoner = Arc::new(MockReasoner::empty());
        let kernel = Arc::new(Kernel::new(
            store.clone(),
            reasoner,
            executor,
            kernel_config,
        ));

        // Create scheduler
        let scheduler = Arc::new(Scheduler::new(store.clone(), kernel));
        info!("Scheduler ready");

        // Create router
        let state = RouterState {
            store,
            scheduler,
            config: self.config.clone(),
        };
        let app = create_router(state);

        // Start server
        let addr: SocketAddr = self.config.bind_addr.parse()?;
        let listener = TcpListener::bind(addr).await?;
        info!(%addr, "HTTP server listening");

        axum::serve(listener, app).await?;

        Ok(())
    }
}
