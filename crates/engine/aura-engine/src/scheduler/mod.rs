//! Scheduler for dispatching agent workers.
//!
//! # Concurrency Model
//!
//! The scheduler enforces a **single-writer-per-agent** invariant: at most one
//! task may process a given agent's transaction queue at any time.  Different
//! agents are fully independent and can be processed concurrently.
//!
//! ## Store-Backed Processing Claims
//!
//! Each scheduling attempt claims a persisted per-agent processing marker before
//! constructing a [`Kernel`](aura_agent_kernel::Kernel). When [`Scheduler::schedule_agent`] is called:
//!
//! 1. A status check is performed (only `Active` agents proceed).
//! 2. A pending-transaction check avoids claiming processing when the inbox is
//!    empty.
//! 3. A store-level compare-and-set claims processing for the agent.
//! 4. All pending transactions are drained without holding an async mutex.
//!
//! Because the claim is per-agent, concurrent calls for *different* agents never
//! block each other. Concurrent calls for the *same* agent are skipped when a
//! claim already exists, preserving caller semantics without head-of-line
//! blocking behind long LLM/tool work.
//!
//! ## Failure Modes
//!
//! * **Panic while claimed** – the local claim guard releases during unwinding.
//!   However, the agent's partially-processed state may be inconsistent; the
//!   store's atomic-batch guarantees prevent *partial record writes*, but the
//!   agent may have committed fewer entries than intended.
//! * **Process crash while claimed** – the persisted claim can remain set after
//!   a hard crash. A future lease timestamp/owner can recover abandoned claims.

use crate::worker::ProcessedAgent;
use aura_agent::{AgentLoopConfig, AgentLoopEvent};
use aura_agent_kernel::{Executor, ExecutorRouter, KernelConfig, PolicyConfig, RecordObserver};
use aura_core_types::AgentId;
use aura_model::{ModelProvider, ToolDefinition};
use aura_store_db::Store;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::error;

mod identity;
mod schedule;

pub use identity::{AgentIdentity, AgentIdentityRegistry};

/// Per-call overrides for [`Scheduler::schedule_agent_with_options`].
///
/// Bundled into a struct so the scheduling entry point stays within
/// the parameter budget as the override surface grows. The legacy
/// [`Scheduler::schedule_agent_with_overrides`] shim maps its two
/// positional overrides onto this struct with `event_tx: None`.
#[derive(Default)]
pub struct ScheduleOverrides {
    /// The caller owns a cancellable deadline around the entire schedule.
    /// Only bounded child dispatch sets this; normal workers retain their timeout.
    pub caller_owns_deadline: bool,
    /// Explicit per-turn loop config. `None` resolves the config from
    /// the [`AgentIdentityRegistry`].
    pub loop_config: Option<AgentLoopConfig>,
    /// Explicit kernel policy override (subagent dispatch narrows it).
    pub policy: Option<PolicyConfig>,
    /// Optional streaming sink threaded into the worker so the agent
    /// loop runs via `run_with_events`. `None` keeps the non-streaming
    /// path. Used by subagent child runs to make the child observable.
    pub event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
    /// Optional per-agent workspace root override as
    /// `(workspace_base, use_workspace_base_as_root)`. `None` keeps the
    /// scheduler's default `workspace_base/<agent_id>` layout. Subagent
    /// child runs set this to the parent session's project workspace so
    /// the child's sandbox is rooted at the real project tree (and can
    /// read project files) instead of an empty per-id scratch dir.
    pub workspace_override: Option<(std::path::PathBuf, bool)>,
    /// Optional pre-built executor router. `None` keeps the legacy
    /// behavior of building a router from the scheduler's shared
    /// (bare node-level) executor list. Child agent runs inject a
    /// session-equivalent router here — built via
    /// [`crate::child_kernel::ChildKernelFactory`] — so the child gets
    /// the same spawn hooks, caller permissions, and parent chain as a
    /// real top-level turn instead of the bare resolver.
    pub router_override: Option<ExecutorRouter>,
    /// Tool definitions shown to the model for this run. `None` uses
    /// the scheduler's shared list, which omits capability-gated tools.
    pub tools_override: Option<Vec<ToolDefinition>>,
    /// Earlier conversation to continue from. The first dequeued prompt
    /// is appended to it, so a long-lived agent keeps its context
    /// across runs.
    pub history: Vec<aura_model::Message>,
}

/// Errors surfaced by [`Scheduler::schedule_agent_with_overrides`]
/// after Step 2 of the worker-routing-identity fix. Existing callers
/// receive these wrapped in `anyhow::Error`; tests can downcast to
/// match on the typed variant.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// No [`AgentIdentity`] is registered for an agent that has pending
    /// transactions and no per-call `agent_loop_config` override. The
    /// failing tx stays in the queue / moves to `failed_txs` so the
    /// operator sees the regression instead of receiving a `429
    /// RATE_LIMITED` from `aura-router`.
    #[error(
        "scheduler: no AgentIdentity registered for agent {agent_id} — \
         the WS session, automaton bridge, or `/tx` caller must register \
         model + X-Aura-* identity before scheduling. Refusing to fall \
         back silently."
    )]
    AgentNotRegistered { agent_id: AgentId },
}

/// Local guard that makes store-backed processing claims release on every
/// normal scheduler exit path, with `Drop` as a panic safety net.
pub struct ProcessingClaim {
    store: Arc<dyn Store>,
    agent_id: AgentId,
    released: bool,
}

impl ProcessingClaim {
    fn try_new(store: Arc<dyn Store>, agent_id: AgentId) -> anyhow::Result<Option<Self>> {
        if !store.try_claim_agent_processing(agent_id)? {
            return Ok(None);
        }
        Ok(Some(Self {
            store,
            agent_id,
            released: false,
        }))
    }

    /// Release the underlying store claim. Idempotent.
    pub fn release(&mut self) -> anyhow::Result<()> {
        if !self.released {
            self.store.release_agent_processing(self.agent_id)?;
            self.released = true;
        }
        Ok(())
    }
}

impl Drop for ProcessingClaim {
    fn drop(&mut self) {
        if !self.released {
            if let Err(error) = self.store.release_agent_processing(self.agent_id) {
                error!(agent_id = %self.agent_id, %error, "Failed to release processing claim");
            }
            self.released = true;
        }
    }
}

/// Scheduler for managing agent workers.
pub struct Scheduler {
    // TODO(phase2-followup): Invariant §10 — bind to `Arc<dyn ReadStore>`
    // once `Kernel::new` accepts a `(ReadStore, WriteHook)` pair. The
    // scheduler itself never calls `append_entry_*`; it only forwards the
    // store handle when constructing per-agent kernels.
    store: Arc<dyn Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Per-agent identity registry. Populated by the chat WS path,
    /// the automaton bridge, and foreground subagent dispatch. The
    /// scheduler consults it to build the `AgentLoopConfig` for
    /// each turn — there is no longer a process-wide default.
    identity_registry: Arc<AgentIdentityRegistry>,
    executors: Vec<Arc<dyn Executor>>,
    tools: Vec<ToolDefinition>,
    kernel_config: KernelConfig,
    memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
}

impl Scheduler {
    /// Create a new scheduler with a freshly-allocated identity
    /// registry. Most production wiring pairs the scheduler with the
    /// runtime's shared registry via [`Self::with_identity_registry`].
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        executors: Vec<Arc<dyn Executor>>,
        tools: Vec<ToolDefinition>,
        workspace_base: PathBuf,
        memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
    ) -> Self {
        Self::with_identity_registry(
            store,
            provider,
            executors,
            tools,
            workspace_base,
            memory_manager,
            AgentIdentityRegistry::new(),
        )
    }

    /// Create a new scheduler that shares an existing
    /// [`AgentIdentityRegistry`]. Production wiring uses this so the
    /// chat-WS, automaton bridge, and worker paths all observe the
    /// same per-agent identity bundle.
    #[must_use]
    pub fn with_identity_registry(
        store: Arc<dyn Store>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        executors: Vec<Arc<dyn Executor>>,
        tools: Vec<ToolDefinition>,
        workspace_base: PathBuf,
        memory_manager: Option<Arc<aura_context_memory::MemoryManager>>,
        identity_registry: Arc<AgentIdentityRegistry>,
    ) -> Self {
        let kernel_config = KernelConfig {
            workspace_base,
            ..KernelConfig::default()
        };
        Self {
            store,
            provider,
            identity_registry,
            executors,
            tools,
            kernel_config,
            memory_manager,
        }
    }

    /// Hand every kernel this scheduler builds an observer for the
    /// entries it appends (see [`KernelConfig::record_observer`]).
    #[must_use]
    pub fn with_record_observer(mut self, observer: Arc<dyn RecordObserver>) -> Self {
        self.kernel_config.record_observer = Some(observer);
        self
    }

    /// Store the full payloads of AuditedLite effects in `store` (see
    /// [`KernelConfig::snapshot_store`]).
    #[must_use]
    pub fn with_snapshot_store(
        mut self,
        store: Arc<dyn aura_store_snapshot::SnapshotStore>,
    ) -> Self {
        self.kernel_config.snapshot_store = store;
        self
    }

    /// The observer set by [`Self::with_record_observer`], for callers
    /// that append entries outside a kernel.
    #[must_use]
    pub fn record_observer(&self) -> Option<&Arc<dyn RecordObserver>> {
        self.kernel_config.record_observer.as_ref()
    }

    /// Borrow the shared identity registry — used by callers that own
    /// the scheduler `Arc` and need to register/look up identities
    /// (chat WS, automaton bridge, subagent dispatch, tests).
    #[must_use]
    pub fn identity_registry(&self) -> &Arc<AgentIdentityRegistry> {
        &self.identity_registry
    }

    /// Attempt to claim exclusive processing for a non-scheduler direct append.
    pub fn try_processing_claim(
        &self,
        agent_id: AgentId,
    ) -> anyhow::Result<Option<ProcessingClaim>> {
        ProcessingClaim::try_new(self.store.clone(), agent_id)
    }

    /// Wait for exclusive processing, used by short direct append paths that
    /// previously awaited the per-agent mutex.
    pub async fn processing_claim(&self, agent_id: AgentId) -> anyhow::Result<ProcessingClaim> {
        loop {
            if let Some(claim) = self.try_processing_claim(agent_id)? {
                return Ok(claim);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Build an `ExecutorRouter` from the shared executor list.
    fn build_executor_router(&self) -> ExecutorRouter {
        ExecutorRouter::with_executors(self.executors.clone())
    }

    /// Schedule processing for an agent.
    ///
    /// Constructs a per-agent [`Kernel`](aura_agent_kernel::Kernel) and routes all transactions through
    /// kernel-mediated processing.
    ///
    /// No `#[instrument]` here: this is a one-line shim that immediately
    /// delegates to [`Self::schedule_agent_with_overrides`], which owns
    /// the canonical `agent{id=...}` span so the prefix chain doesn't
    /// duplicate the `agent_id` field.
    pub async fn schedule_agent(&self, agent_id: AgentId) -> anyhow::Result<u64> {
        self.schedule_agent_with_overrides(agent_id, None, None)
            .await
            .map(|processed| processed.processed)
    }

    /// Schedule processing and return the last loop result.
    ///
    /// Subagent dispatch uses this to run a child with narrowed policy and a
    /// per-kind loop configuration while preserving the single-writer claim.
    ///
    /// Span name `agent{id=…}` is the canonical root of the runtime →
    /// agent-loop span chain; downstream spans (`worker`, `task{id}`,
    /// `turn`, `sampling`, `complete{model}`) all inherit from it so
    /// the visible prefix in the structured console transcript reads
    /// `agent{id}:worker:task{id}:turn{T}:sampling{I}:complete{model}`
    /// — every level adds new information.
    pub async fn schedule_agent_with_overrides(
        &self,
        agent_id: AgentId,
        agent_loop_config: Option<AgentLoopConfig>,
        policy: Option<PolicyConfig>,
    ) -> anyhow::Result<ProcessedAgent> {
        self.schedule_agent_with_options(
            agent_id,
            ScheduleOverrides {
                loop_config: agent_loop_config,
                policy,
                caller_owns_deadline: false,
                ..ScheduleOverrides::default()
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests;
