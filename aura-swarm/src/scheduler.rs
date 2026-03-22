//! Scheduler for dispatching agent workers.

use crate::worker::process_agent;
use aura_agent::{AgentLoop, AgentLoopConfig, KernelToolExecutor};
use aura_core::AgentId;
use aura_executor::{Executor, ExecutorRouter};
use aura_reasoner::{ModelProvider, ToolDefinition};
use aura_store::{AgentStatus, Store};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument};

/// Per-agent lock for single-writer guarantee.
type AgentLock = Arc<Mutex<()>>;

/// Scheduler for managing agent workers.
pub struct Scheduler {
    store: Arc<dyn Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    agent_loop: AgentLoop,
    executors: Vec<Arc<dyn Executor>>,
    tools: Vec<ToolDefinition>,
    workspace_base: PathBuf,
    agent_locks: DashMap<AgentId, AgentLock>,
}

impl Scheduler {
    /// Create a new scheduler.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        executors: Vec<Arc<dyn Executor>>,
        tools: Vec<ToolDefinition>,
        workspace_base: PathBuf,
    ) -> Self {
        let config = AgentLoopConfig::default();
        Self {
            store,
            provider,
            agent_loop: AgentLoop::new(config),
            executors,
            tools,
            workspace_base,
            agent_locks: DashMap::new(),
        }
    }

    /// Get or create lock for an agent.
    fn get_lock(&self, agent_id: AgentId) -> AgentLock {
        self.agent_locks
            .entry(agent_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Build an `ExecutorRouter` from the shared executor list.
    fn build_executor_router(&self) -> ExecutorRouter {
        ExecutorRouter::with_executors(self.executors.clone())
    }

    /// Schedule processing for an agent.
    ///
    /// This will acquire the agent lock and process all pending transactions.
    #[instrument(skip(self), fields(agent_id = %agent_id))]
    pub async fn schedule_agent(&self, agent_id: AgentId) -> anyhow::Result<u64> {
        let status = self.store.get_agent_status(agent_id)?;
        if status != AgentStatus::Active {
            debug!(?status, "Agent not active, skipping");
            return Ok(0);
        }

        if !self.store.has_pending_tx(agent_id)? {
            debug!("No pending transactions");
            return Ok(0);
        }

        let lock = self.get_lock(agent_id);
        let _guard = lock.lock().await;

        debug!("Lock acquired, processing");

        let workspace = self.workspace_base.join(agent_id.to_hex());
        let router = self.build_executor_router();
        let kernel_executor = KernelToolExecutor::new(router, agent_id, workspace);

        match process_agent(
            agent_id,
            self.store.clone(),
            self.provider.clone(),
            &self.agent_loop,
            &kernel_executor,
            &self.tools,
        )
        .await
        {
            Ok(count) => {
                info!(processed = count, "Agent processing complete");
                Ok(count)
            }
            Err(e) => {
                error!(error = %e, "Agent processing failed");
                Err(e)
            }
        }
    }

    /// Check if an agent is currently being processed.
    ///
    /// Returns `true` if the agent's lock is held (processing in progress).
    #[must_use]
    #[allow(dead_code)]
    pub fn is_agent_busy(&self, agent_id: AgentId) -> bool {
        self.agent_locks
            .get(&agent_id)
            .is_some_and(|lock| lock.try_lock().is_err())
    }
}
