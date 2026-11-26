//! [`Scheduler::schedule_agent_with_options`]: one claimed, kernel-mediated
//! processing run for an agent.

use crate::worker::{process_agent_with_timeout, ProcessedAgent, AGENT_LOOP_TIMEOUT};
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_agent_kernel::{ExecutorRouter, Kernel, PolicyConfig};
use aura_core_types::{AgentId, AgentStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, instrument};

use super::{ScheduleOverrides, Scheduler, SchedulerError};

impl Scheduler {
    /// Schedule processing with a bundled [`ScheduleOverrides`].
    ///
    /// Superset of [`Self::schedule_agent_with_overrides`] that also
    /// carries an optional [`AgentLoopEvent`](aura_agent::AgentLoopEvent) streaming sink. Subagent
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
        if !self.has_work(agent_id)? {
            return Ok(ProcessedAgent::default());
        }

        // Resolve the per-agent loop config BEFORE acquiring the
        // processing claim so a missing registration trips
        // `SchedulerError::AgentNotRegistered` immediately — the
        // failing tx stays in the inbox / `failed_txs` and the
        // operator sees the regression instead of routing
        // anonymous-public traffic.
        let config = self.resolve_loop_config(agent_id, agent_loop_config)?;

        let Some(mut claim) = self.try_processing_claim(agent_id)? else {
            debug!("Agent already processing, skipping");
            return Ok(ProcessedAgent::default());
        };

        debug!("Processing claim acquired, constructing kernel for agent");
        let kernel = self.build_kernel(agent_id, router_override, policy, workspace_override)?;
        let agent_loop = AgentLoop::new(self.with_memory_observer(config, agent_id));
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
        finish_processing(result, release_result)
    }

    /// Whether the agent is active and has pending transactions.
    fn has_work(&self, agent_id: AgentId) -> anyhow::Result<bool> {
        let status = self.store.get_agent_status(agent_id)?;
        if status != AgentStatus::Active {
            debug!(?status, "Agent not active, skipping");
            return Ok(false);
        }

        if !self.store.has_pending_tx(agent_id)? {
            debug!("No pending transactions");
            return Ok(false);
        }
        Ok(true)
    }

    /// The caller's loop config, else the one built from the agent's
    /// registered identity. Fails when neither exists.
    fn resolve_loop_config(
        &self,
        agent_id: AgentId,
        agent_loop_config: Option<AgentLoopConfig>,
    ) -> anyhow::Result<AgentLoopConfig> {
        match agent_loop_config {
            Some(cfg) => Ok(cfg),
            None => match self.identity_registry.get(agent_id) {
                Some(identity) => Ok(identity.into_loop_config()),
                None => {
                    error!(
                        agent_id = %agent_id,
                        "AgentIdentity not registered: refusing to dispatch without explicit model + X-Aura-* identity"
                    );
                    Err(anyhow::Error::new(SchedulerError::AgentNotRegistered {
                        agent_id,
                    }))
                }
            },
        }
    }

    /// Build the per-agent kernel for one run. Call only while holding
    /// the agent's processing claim.
    fn build_kernel(
        &self,
        agent_id: AgentId,
        router_override: Option<ExecutorRouter>,
        policy: Option<PolicyConfig>,
        workspace_override: Option<(PathBuf, bool)>,
    ) -> anyhow::Result<Arc<Kernel>> {
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
        let kernel = Kernel::new(
            self.store.clone(),
            self.provider.clone(),
            router,
            kernel_config,
            agent_id,
        )
        .map_err(|e| anyhow::anyhow!("kernel construction failed: {e}"))?;
        Ok(Arc::new(kernel))
    }

    /// `config` with the memory turn observer added when the scheduler
    /// has a memory manager.
    fn with_memory_observer(
        &self,
        mut config: AgentLoopConfig,
        agent_id: AgentId,
    ) -> AgentLoopConfig {
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
        config
    }
}

/// Log how a run ended and combine its result with the claim release.
fn finish_processing(
    result: anyhow::Result<ProcessedAgent>,
    release_result: anyhow::Result<()>,
) -> anyhow::Result<ProcessedAgent> {
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
