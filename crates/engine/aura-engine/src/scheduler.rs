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
//! constructing a [`Kernel`]. When [`Scheduler::schedule_agent`] is called:
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

use crate::worker::{process_agent_with_timeout, ProcessedAgent, AGENT_LOOP_TIMEOUT};
use aura_agent::{AgentLoop, AgentLoopConfig, AgentLoopEvent};
use aura_agent_kernel::{
    Executor, ExecutorRouter, Kernel, KernelConfig, PolicyConfig, RecordObserver,
};
use aura_core_types::{AgentId, AgentStatus};
use aura_model::{ModelProvider, ModelRequestKind, PromptCacheRetention, ToolDefinition};
use aura_store_db::Store;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument};

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

/// Per-agent identity recorded by the chat WS / automaton bridge / `/tx`
/// and consulted by [`Scheduler::schedule_agent_with_overrides`] when no
/// explicit `agent_loop_config` override is supplied.
///
/// This is the seam that fixes the worker-path regression where `/v1/messages`
/// went out with `claude-opus-4-6` (the pre-rename default) and a stripped
/// `X-Aura-*` envelope, causing `aura-router` to bucket the request as anonymous
/// public traffic and return `429 RATE_LIMITED`.
#[derive(Debug, Clone)]
pub struct AgentIdentity {
    /// Explicit caller effort; inherited by worker and child runs.
    pub reasoning_effort: Option<aura_model::ThinkingEffort>,
    pub upstream_provider_family: Option<String>,
    /// Caller-selected model (e.g. `claude-opus-4-7`). Required.
    pub model: String,
    /// Org UUID forwarded as `X-Aura-Org-Id` on outbound `/v1/messages` calls.
    pub aura_org_id: Option<String>,
    /// Storage session UUID forwarded as `X-Aura-Session-Id`.
    pub aura_session_id: Option<String>,
    /// Project-agent UUID forwarded as `X-Aura-Agent-Id`.
    pub aura_agent_id: Option<String>,
    /// Project UUID forwarded as `X-Aura-Project-Id`.
    pub aura_project_id: Option<String>,
    /// System prompt to feed into the agent loop.
    pub system_prompt: String,
    /// OpenAI-family stable cache key.
    pub prompt_cache_key: Option<String>,
    /// Retention hint paired with `prompt_cache_key`.
    pub prompt_cache_retention: Option<PromptCacheRetention>,
    /// Request contract kind. Chat sessions ship `Chat`; dev-loop / task-run
    /// land `DevLoopBootstrap` (and the loop self-promotes to
    /// `DevLoopContinuation` after the first iteration).
    pub request_kind: ModelRequestKind,
    /// Maximum context window in tokens, used for compaction.
    pub max_context_tokens: usize,
    /// JWT auth token forwarded onto outbound model requests.
    pub auth_token: Option<String>,
    /// Prompt-cache scope of the agent's kernel (see
    /// [`KernelConfig::cache_salt`]). Spawned agents inherit it with the
    /// rest of the identity, so a benchmark run's children share its
    /// cache scope and no other.
    pub cache_salt: Option<String>,
    /// Scratch folder of the agent's commands (see
    /// [`KernelConfig::run_scratch`]). Spawned agents inherit it with the
    /// rest of the identity, so a run's children work in its scratch.
    pub run_scratch: Option<aura_exec_tools::RunScratch>,
}

impl AgentIdentity {
    /// Build the per-agent [`AgentLoopConfig`] consumed by the worker
    /// path. Mirrors the chat-WS path's `Session::agent_loop_config`
    /// shape: every router/billing identifier round-trips, the
    /// caller-selected model is honored, and `request_kind` matches
    /// what the call site declared.
    #[must_use]
    pub fn into_loop_config(self) -> AgentLoopConfig {
        AgentLoopConfig {
            user_thinking_effort: self.reasoning_effort,
            upstream_provider_family: self.upstream_provider_family,
            system_prompt: self.system_prompt,
            max_context_tokens: Some(self.max_context_tokens as u64),
            auth_token: self.auth_token,
            aura_project_id: self.aura_project_id,
            aura_agent_id: self.aura_agent_id,
            aura_session_id: self.aura_session_id,
            aura_org_id: self.aura_org_id,
            prompt_cache_key: self.prompt_cache_key,
            prompt_cache_retention: self.prompt_cache_retention.map(|r| match r {
                PromptCacheRetention::Hours24 => "24h".to_string(),
                PromptCacheRetention::InMemory => "in_memory".to_string(),
            }),
            request_kind: self.request_kind,
            ..AgentLoopConfig::for_agent(self.model)
        }
    }
}

/// In-memory registry of [`AgentIdentity`] entries.
///
/// Populated in three places:
/// - `Session::apply_chat_runtime_request` (chat run bootstrap) — registers
///   the session model + IDs before the first turn dispatches.
/// - `automaton_bridge::start_dev_loop_with_capabilities` /
///   `run_task_with_capabilities` — registers the dev-loop / task-run identity
///   alongside the existing `AgentRunnerConfig` plumbing (commit `d12fe29`).
/// - `AgentHost::spawn_agent` — registers a spawned agent's identity after
///   allocating the child id, so its turns go out with the resolved model +
///   parent IDs.
///
/// The HTTP `/tx` and `/agents/:id/tool_permissions` paths do **not** carry
/// per-call identity. They rely on the entry registered by the upstream
/// session / automaton bootstrap; if no entry exists,
/// [`Scheduler::schedule_agent_with_overrides`] returns
/// [`SchedulerError::AgentNotRegistered`] (and emits a structured
/// `error!` line) instead of falling back silently. The failing tx
/// stays in the inbox / `failed_txs` so the operator sees the
/// regression rather than a 429.
#[derive(Default)]
pub struct AgentIdentityRegistry {
    inner: DashMap<AgentId, AgentIdentity>,
}

impl AgentIdentityRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register or replace the identity for `agent_id`. Replacing on
    /// chat-run bootstrap re-keys are intentional — reconnecting an existing
    /// run must be allowed to refresh the model + IDs without leaking the
    /// previous bundle.
    pub fn register(&self, agent_id: AgentId, identity: AgentIdentity) {
        self.inner.insert(agent_id, identity);
    }

    /// Look up the registered identity. Returns a clone so the caller
    /// can build the per-turn `AgentLoopConfig` without holding a
    /// `DashMap` ref across the scheduling await point.
    #[must_use]
    pub fn get(&self, agent_id: AgentId) -> Option<AgentIdentity> {
        self.inner.get(&agent_id).map(|entry| entry.value().clone())
    }

    /// The cache salt registered for `agent_id`, if any.
    #[must_use]
    pub fn cache_salt(&self, agent_id: AgentId) -> Option<String> {
        self.inner
            .get(&agent_id)
            .and_then(|entry| entry.value().cache_salt.clone())
    }

    /// The scratch folder registered for `agent_id`, if any.
    #[must_use]
    pub fn run_scratch(&self, agent_id: AgentId) -> Option<aura_exec_tools::RunScratch> {
        self.inner
            .get(&agent_id)
            .and_then(|entry| entry.value().run_scratch.clone())
    }

    /// Clear the entry — primarily used by tests asserting the
    /// hard-fail path. Production code holds entries for the lifetime
    /// of the parent session / automaton.
    pub fn unregister(&self, agent_id: AgentId) {
        self.inner.remove(&agent_id);
    }
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
    /// Constructs a per-agent [`Kernel`] and routes all transactions through
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

    /// Schedule processing with a bundled [`ScheduleOverrides`].
    ///
    /// Superset of [`Self::schedule_agent_with_overrides`] that also
    /// carries an optional [`AgentLoopEvent`] streaming sink. Subagent
    /// dispatch uses this to make a child run observable as its own
    /// live thread while still returning the terminal result inline.
    #[instrument(name = "agent", skip(self, overrides), fields(id = %agent_id))]
    pub async fn schedule_agent_with_options(
        &self,
        agent_id: AgentId,
        overrides: ScheduleOverrides,
    ) -> anyhow::Result<ProcessedAgent> {
        let ScheduleOverrides {
            caller_owns_deadline,
            loop_config: agent_loop_config,
            policy,
            event_tx,
            workspace_override,
            router_override,
            tools_override,
            history,
        } = overrides;
        let status = self.store.get_agent_status(agent_id)?;
        if status != AgentStatus::Active {
            debug!(?status, "Agent not active, skipping");
            return Ok(ProcessedAgent::default());
        }

        if !self.store.has_pending_tx(agent_id)? {
            debug!("No pending transactions");
            return Ok(ProcessedAgent::default());
        }

        // Resolve the per-agent loop config BEFORE acquiring the
        // processing claim so a missing registration trips
        // `SchedulerError::AgentNotRegistered` immediately — the
        // failing tx stays in the inbox / `failed_txs` and the
        // operator sees the regression instead of routing
        // anonymous-public traffic.
        let config = match agent_loop_config {
            Some(cfg) => cfg,
            None => match self.identity_registry.get(agent_id) {
                Some(identity) => identity.into_loop_config(),
                None => {
                    error!(
                        agent_id = %agent_id,
                        "AgentIdentity not registered: refusing to dispatch without explicit model + X-Aura-* identity"
                    );
                    return Err(anyhow::Error::new(SchedulerError::AgentNotRegistered {
                        agent_id,
                    }));
                }
            },
        };

        let Some(mut claim) = self.try_processing_claim(agent_id)? else {
            debug!("Agent already processing, skipping");
            return Ok(ProcessedAgent::default());
        };

        debug!("Processing claim acquired, constructing kernel for agent");

        // Child subagent runs inject a session-equivalent router (built
        // via `ChildKernelFactory`); everything else falls back to the
        // scheduler's shared bare executor list.
        let router = router_override.unwrap_or_else(|| self.build_executor_router());
        let mut kernel_config = self.kernel_config.clone();
        if let Some(policy) = policy {
            kernel_config.policy = policy;
        }
        if let Some((workspace_base, use_as_root)) = workspace_override {
            kernel_config.workspace_base = workspace_base;
            kernel_config.use_workspace_base_as_root = use_as_root;
        }
        // The salt is the agent's, whatever loop config the caller passed,
        // so a spawned agent's calls stay in its root's cache scope.
        kernel_config.cache_salt = self.identity_registry.cache_salt(agent_id);
        kernel_config.run_scratch = self.identity_registry.run_scratch(agent_id);
        // The store-backed claim is the scheduler-side mitigation for
        // `Kernel::next_seq`'s read-before-append window: only one scheduler
        // kernel is constructed for an agent while a claim is held.
        // TODO(phase1-seq-reservation): replace this mitigation with a
        // store-backed sequence reservation if non-claim writers grow.
        let kernel = Arc::new(
            Kernel::new(
                self.store.clone(),
                self.provider.clone(),
                router,
                kernel_config,
                agent_id,
            )
            .map_err(|e| anyhow::anyhow!("kernel construction failed: {e}"))?,
        );

        let mut config = config;
        if let Some(ref mm) = self.memory_manager {
            config
                .observers
                .push(crate::memory_observer::MemoryTurnObserver::new(
                    Arc::clone(mm),
                    agent_id,
                    None,
                    Vec::new(),
                    None,
                ));
        }
        let agent_loop = AgentLoop::new(config);
        let tools = tools_override.as_deref().unwrap_or(&self.tools);

        let timeout = (!caller_owns_deadline).then_some(AGENT_LOOP_TIMEOUT);
        let result = process_agent_with_timeout(
            agent_id,
            kernel,
            &agent_loop,
            tools,
            event_tx,
            timeout,
            history,
        )
        .await;
        let release_result = claim.release();

        match (result, release_result) {
            (Ok(processed), Ok(())) => {
                info!(processed = processed.processed, "Agent processing complete");
                Ok(processed)
            }
            (Ok(_), Err(e)) => {
                error!(error = %e, "Agent processing claim release failed");
                Err(e)
            }
            (Err(e), release) => {
                if let Err(release_error) = release {
                    error!(error = %release_error, "Agent processing claim release failed after error");
                }
                error!(error = %e, "Agent processing failed");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;
