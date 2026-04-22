//! Scheduler for dispatching agent workers.
//!
//! # Concurrency Model
//!
//! The scheduler enforces a **single-writer-per-agent** invariant: at most one
//! task may process a given agent's transaction queue at any time.  Different
//! agents are fully independent and can be processed concurrently.
//!
//! ## Per-Agent Locking
//!
//! Each agent is assigned an [`Arc<Mutex<()>>`](tokio::sync::Mutex) stored in a
//! [`DashMap`].  When [`Scheduler::schedule_agent`] is called:
//!
//! 1. A status check is performed (only `Active` agents proceed).
//! 2. A pending-transaction check avoids acquiring the lock when the inbox is
//!    empty.
//! 3. The per-agent mutex is acquired (`lock().await`).
//! 4. All pending transactions are drained inside the critical section.
//!
//! Because the lock is per-agent, concurrent calls for *different* agents never
//! block each other.  Concurrent calls for the *same* agent are serialised:
//! the second caller awaits the mutex until the first finishes.
//!
//! ## Failure Modes
//!
//! * **Panic while lock is held** – if the worker panics inside the critical
//!   section, the [`tokio::sync::Mutex`] becomes *poisoned-free* (unlike
//!   `std::sync::Mutex`, Tokio's mutex does **not** poison on panic).  The lock
//!   is simply released when the `MutexGuard` is dropped during unwinding, so
//!   the next caller can acquire it normally.  However, the agent's partially-
//!   processed state may be inconsistent; the store's atomic-batch guarantees
//!   prevent *partial record writes*, but the agent may have committed fewer
//!   entries than intended.
//! * **Lock map growth** – agent locks are never removed from the `DashMap`.
//!   For long-running swarms with many transient agents, this could accumulate
//!   memory.  A periodic eviction sweep is a potential future improvement.

use crate::worker::process_agent;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_core::{AgentId, AgentStatus};
use aura_kernel::{ApprovalRegistry, Executor, ExecutorRouter, Kernel, KernelConfig};
use aura_reasoner::{ModelProvider, ToolDefinition};
use aura_store::Store;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument};

/// Per-agent lock for single-writer guarantee.
type AgentLock = Arc<Mutex<()>>;

/// Scheduler for managing agent workers.
pub struct Scheduler {
    // TODO(phase2-followup): Invariant §10 — bind to `Arc<dyn ReadStore>`
    // once `Kernel::new` accepts a `(ReadStore, WriteHook)` pair. The
    // scheduler itself never calls `append_entry_*`; it only forwards the
    // store handle when constructing per-agent kernels.
    store: Arc<dyn Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    agent_loop_config: AgentLoopConfig,
    executors: Vec<Arc<dyn Executor>>,
    tools: Vec<ToolDefinition>,
    agent_locks: DashMap<AgentId, AgentLock>,
    kernel_config: KernelConfig,
    memory_manager: Option<Arc<aura_memory::MemoryManager>>,
    /// Shared single-use approvals. Every per-agent [`Kernel`] the
    /// scheduler builds receives a handle to this registry so an
    /// approval granted via the HTTP API survives the kernel that was
    /// active when the proposal was first denied (Phase 6, security
    /// audit).
    approvals: ApprovalRegistry,
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
        memory_manager: Option<Arc<aura_memory::MemoryManager>>,
    ) -> Self {
        let kernel_config = KernelConfig {
            workspace_base,
            ..KernelConfig::default()
        };
        Self {
            store,
            provider,
            agent_loop_config: AgentLoopConfig::default(),
            executors,
            tools,
            agent_locks: DashMap::new(),
            kernel_config,
            memory_manager,
            approvals: ApprovalRegistry::new(),
        }
    }

    /// Return a handle to the shared [`ApprovalRegistry`] the scheduler
    /// injects into each kernel it constructs. The router's
    /// `POST /tool-approval` handler uses this to register one-shot
    /// approvals without having to reach through a specific kernel.
    #[must_use]
    pub fn approval_registry(&self) -> ApprovalRegistry {
        self.approvals.clone()
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
    /// Constructs a per-agent [`Kernel`] and routes all transactions through
    /// kernel-mediated processing.
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

        debug!("Lock acquired, constructing kernel for agent");

        let router = self.build_executor_router();
        let kernel = Arc::new(
            Kernel::new_with_approvals(
                self.store.clone(),
                self.provider.clone(),
                router,
                self.kernel_config.clone(),
                agent_id,
                self.approvals.clone(),
            )
            .map_err(|e| anyhow::anyhow!("kernel construction failed: {e}"))?,
        );

        let mut config = self.agent_loop_config.clone();
        if let Some(ref mm) = self.memory_manager {
            config.observers.push(mm.turn_observer(agent_id, None));
        }
        let agent_loop = AgentLoop::new(config);

        match process_agent(agent_id, kernel, &agent_loop, &self.tools).await {
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
    /// Retained for use by future status/health endpoints.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_agent_busy(&self, agent_id: AgentId) -> bool {
        self.agent_locks
            .get(&agent_id)
            .is_some_and(|lock| lock.try_lock().is_err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;

    fn create_test_scheduler() -> (Scheduler, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test"));
        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
        (scheduler, dir)
    }

    #[test]
    fn test_scheduler_creation() {
        let (_scheduler, _dir) = create_test_scheduler();
    }

    #[tokio::test]
    async fn test_schedule_agent_no_pending() {
        let (scheduler, _dir) = create_test_scheduler();
        let agent_id = AgentId::generate();
        let result = scheduler.schedule_agent(agent_id).await.unwrap();
        assert_eq!(result, 0, "No pending txs should process 0");
    }

    #[tokio::test]
    async fn test_schedule_paused_agent_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test"));
        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let agent_id = AgentId::generate();
        store
            .set_agent_status(agent_id, AgentStatus::Paused)
            .unwrap();

        let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
        let result = scheduler.schedule_agent(agent_id).await.unwrap();
        assert_eq!(result, 0, "Paused agents should be skipped");
    }

    #[tokio::test]
    async fn test_schedule_dead_agent_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test"));
        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let agent_id = AgentId::generate();
        store.set_agent_status(agent_id, AgentStatus::Dead).unwrap();

        let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
        let result = scheduler.schedule_agent(agent_id).await.unwrap();
        assert_eq!(result, 0, "Dead agents should be skipped");
    }

    #[test]
    fn test_is_agent_busy_false_by_default() {
        let (scheduler, _dir) = create_test_scheduler();
        let agent_id = AgentId::generate();
        assert!(!scheduler.is_agent_busy(agent_id));
    }

    #[test]
    fn test_get_lock_returns_same_lock_for_same_agent() {
        let (scheduler, _dir) = create_test_scheduler();
        let agent_id = AgentId::generate();
        let lock1 = scheduler.get_lock(agent_id);
        let lock2 = scheduler.get_lock(agent_id);
        assert!(Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn test_get_lock_different_agents_different_locks() {
        let (scheduler, _dir) = create_test_scheduler();
        let a1 = AgentId::generate();
        let a2 = AgentId::generate();
        let lock1 = scheduler.get_lock(a1);
        let lock2 = scheduler.get_lock(a2);
        assert!(!Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn test_build_executor_router() {
        let (scheduler, _dir) = create_test_scheduler();
        let _router = scheduler.build_executor_router();
    }
}
