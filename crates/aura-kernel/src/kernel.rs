//! Kernel implementation.
//!
//! ## Processing Invariants
//!
//! Every call to [`Kernel::process`] upholds the following guarantees:
//!
//! 1. **Deterministic context** — the context hash is derived solely from the
//!    incoming transaction and the record window loaded from the store.
//!    Re-processing the same inputs always yields the same context hash.
//!
//! 2. **Complete recording** — every intermediate artifact (proposals, policy
//!    decisions, actions, effects) is captured in the returned [`RecordEntry`]
//!    so the step can be replayed without a live reasoner or executor.
//!
//! 3. **Replay fidelity** — when `replay_mode` is enabled the kernel skips
//!    the reasoner and executor entirely, reading the recorded artifacts
//!    instead. This allows offline verification of the event chain.
//!
//! ## Replay Semantics
//!
//! In replay mode (`KernelConfig::replay_mode = true`):
//! - The reasoner is **not** called; proposals are empty.
//! - Actions are **not** executed; effects are empty.
//! - The record entry is still constructed with a valid context hash so that
//!   downstream consumers can verify chain integrity.

use crate::context::ContextBuilder;
use crate::executor::ExecuteContext;
use crate::policy::{Policy, PolicyConfig};
use crate::ExecutorRouter;
use aura_core::{
    Action, ActionId, Decision, Effect, EffectStatus, ProposalSet, RecordEntry, Transaction,
};
use aura_reasoner::ProposeRequest;
use aura_store::Store;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, warn};

/// Kernel configuration.
#[derive(Debug, Clone)]
pub struct KernelConfig {
    /// Size of record window for context
    pub record_window_size: usize,
    /// Policy configuration
    pub policy: PolicyConfig,
    /// Base workspace directory
    pub workspace_base: PathBuf,
    /// Whether we're in replay mode (skip reasoner/tools)
    pub replay_mode: bool,
    /// Timeout for reasoner proposals in milliseconds.
    pub proposal_timeout_ms: u64,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            record_window_size: 50,
            policy: PolicyConfig::default(),
            workspace_base: PathBuf::from("./workspaces"),
            replay_mode: false,
            proposal_timeout_ms: 120_000,
        }
    }
}

/// Result of processing a transaction.
#[derive(Debug)]
pub struct ProcessResult {
    /// The record entry created
    pub entry: RecordEntry,
    /// Whether any actions failed
    pub had_failures: bool,
}

/// Trait for generating proposals from context.
///
/// This replaces the legacy `Reasoner` trait from aura-reasoner. The kernel
/// only needs `propose()`, so callers can implement this with a real model
/// provider, a mock, or even a closure.
#[async_trait::async_trait]
pub trait Proposer: Send + Sync {
    /// Generate proposals based on context.
    async fn propose(
        &self,
        request: ProposeRequest,
    ) -> Result<aura_core::ProposalSet, aura_reasoner::ReasonerError>;
}

/// The deterministic kernel.
pub struct Kernel<S, R>
where
    S: Store,
    R: Proposer,
{
    store: Arc<S>,
    reasoner: Arc<R>,
    executor: ExecutorRouter,
    policy: Policy,
    config: KernelConfig,
}

impl<S, R> Kernel<S, R>
where
    S: Store,
    R: Proposer,
{
    /// Create a new kernel.
    #[must_use]
    pub fn new(
        store: Arc<S>,
        reasoner: Arc<R>,
        executor: ExecutorRouter,
        config: KernelConfig,
    ) -> Self {
        let policy = Policy::new(config.policy.clone());
        Self {
            store,
            reasoner,
            executor,
            policy,
            config,
        }
    }

    /// Get the workspace path for an agent.
    fn agent_workspace(&self, agent_id: &aura_core::AgentId) -> PathBuf {
        self.config.workspace_base.join(agent_id.to_hex())
    }

    /// Process a transaction and produce a record entry.
    ///
    /// # Errors
    ///
    /// Returns error if storage operations or proposal processing fails.
    #[instrument(skip(self, tx), fields(agent_id = %tx.agent_id, hash = %tx.hash))]
    pub async fn process(
        &self,
        tx: Transaction,
        next_seq: u64,
    ) -> Result<ProcessResult, crate::KernelError> {
        info!(seq = next_seq, "Processing transaction");

        // 1. Load record window
        let from_seq = next_seq.saturating_sub(self.config.record_window_size as u64);
        let window = self
            .store
            .scan_record(tx.agent_id, from_seq, self.config.record_window_size)
            .map_err(|e| {
                crate::KernelError::Store(format!(
                    "scan_record(agent={}, from_seq={from_seq}): {e}",
                    tx.agent_id
                ))
            })?;
        debug!(window_size = window.len(), "Loaded record window");

        // 2. Build context
        let context = ContextBuilder::new(&tx)
            .map_err(|e| {
                crate::KernelError::Serialization(format!(
                    "serializing transaction {}: {e}",
                    tx.hash
                ))
            })?
            .with_record_window(window)
            .build();

        // 3. Get proposals (skip in replay mode)
        let proposals = if self.config.replay_mode {
            debug!("Replay mode: skipping reasoner");
            ProposalSet::new()
        } else {
            let mut p = self.get_proposals(&tx, &context).await?;
            let max = self.policy.max_proposals();
            if p.proposals.len() > max {
                warn!(
                    count = p.proposals.len(),
                    max,
                    "Truncating proposals to max_proposals limit"
                );
                p.proposals.truncate(max);
            }
            p
        };

        // 4. Apply policy and build actions
        let (actions, decision) = self.apply_policy(&proposals);
        debug!(
            accepted = decision.accepted_action_ids.len(),
            rejected = decision.rejected.len(),
            "Policy applied"
        );

        // 5. Execute actions (skip in replay mode)
        let effects = if self.config.replay_mode {
            debug!("Replay mode: skipping execution");
            vec![]
        } else {
            self.execute_actions(&tx.agent_id, &actions).await?
        };

        // Check for failures
        let had_failures = effects.iter().any(|e| e.status == EffectStatus::Failed);
        if had_failures {
            warn!("Some actions failed");
        }

        // 6. Build record entry
        let entry = RecordEntry::builder(next_seq, tx)
            .context_hash(context.context_hash)
            .proposals(proposals)
            .decision(decision)
            .actions(actions)
            .effects(effects)
            .build();

        info!(seq = next_seq, "Transaction processed");

        Ok(ProcessResult {
            entry,
            had_failures,
        })
    }

    /// Get proposals from the reasoner.
    async fn get_proposals(
        &self,
        tx: &Transaction,
        context: &crate::context::Context,
    ) -> Result<ProposalSet, crate::KernelError> {
        let request = ProposeRequest::new(tx.agent_id, tx.clone())
            .with_record_window(context.record_summaries.clone());

        let timeout_duration = Duration::from_millis(self.config.proposal_timeout_ms);

        match timeout(timeout_duration, self.reasoner.propose(request)).await {
            Ok(Ok(proposals)) => {
                debug!(count = proposals.proposals.len(), "Received proposals");
                Ok(proposals)
            }
            Ok(Err(e)) => {
                error!(error = %e, "Reasoner failed");
                Err(crate::KernelError::Reasoner(e.to_string()))
            }
            Err(_) => {
                error!(
                    timeout_ms = self.config.proposal_timeout_ms,
                    "Reasoner timed out"
                );
                Err(crate::KernelError::Timeout(format!(
                    "Reasoner timed out after {}ms",
                    self.config.proposal_timeout_ms
                )))
            }
        }
    }

    /// Apply policy to proposals and build actions.
    fn apply_policy(&self, proposals: &ProposalSet) -> (Vec<Action>, Decision) {
        let mut actions = Vec::new();
        let mut decision = Decision::new();

        for (idx, proposal) in proposals.proposals.iter().enumerate() {
            let result = self.policy.check(proposal);

            if result.allowed {
                let action_id = ActionId::generate();
                let action = Action::new(action_id, proposal.action_kind, proposal.payload.clone());
                actions.push(action);
                decision.accept(action_id);
            } else {
                #[allow(clippy::cast_possible_truncation)] // proposals count is always small
                decision.reject(idx as u32, result.reason.unwrap_or_default());
            }
        }

        (actions, decision)
    }

    /// Execute actions and collect effects.
    async fn execute_actions(
        &self,
        agent_id: &aura_core::AgentId,
        actions: &[Action],
    ) -> Result<Vec<Effect>, crate::KernelError> {
        let mut effects = Vec::new();
        let workspace = self.agent_workspace(agent_id);

        tokio::fs::create_dir_all(&workspace).await.map_err(|e| {
            crate::KernelError::Internal(format!(
                "failed to create workspace {}: {e}",
                workspace.display()
            ))
        })?;

        for action in actions {
            let ctx = ExecuteContext::new(*agent_id, action.action_id, workspace.clone());

            let effect = self.executor.execute(&ctx, action).await;
            effects.push(effect);
        }

        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_store::RocksStore;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    struct EmptyProposer;

    #[async_trait::async_trait]
    impl Proposer for EmptyProposer {
        async fn propose(
            &self,
            _request: ProposeRequest,
        ) -> Result<ProposalSet, aura_reasoner::ReasonerError> {
            Ok(ProposalSet::new())
        }
    }

    struct FailingProposer {
        call_count: AtomicU64,
    }

    impl FailingProposer {
        fn new() -> Self {
            Self {
                call_count: AtomicU64::new(0),
            }
        }
        fn call_count(&self) -> u64 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Proposer for FailingProposer {
        async fn propose(
            &self,
            _request: ProposeRequest,
        ) -> Result<ProposalSet, aura_reasoner::ReasonerError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(aura_reasoner::ReasonerError::Internal(
                "configured to fail".into(),
            ))
        }
    }

    fn create_test_kernel() -> (Kernel<RocksStore, EmptyProposer>, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let reasoner = Arc::new(EmptyProposer);
        let executor = ExecutorRouter::new();

        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..KernelConfig::default()
        };

        let kernel = Kernel::new(store, reasoner, executor, config);
        (kernel, db_dir, ws_dir)
    }

    #[tokio::test]
    async fn test_process_empty_proposals() {
        let (kernel, _db_dir, _ws_dir) = create_test_kernel();

        let tx = Transaction::user_prompt(aura_core::AgentId::generate(), "test");
        let result = kernel.process(tx, 1).await.unwrap();

        assert_eq!(result.entry.seq, 1);
        assert!(result.entry.actions.is_empty());
        assert!(!result.had_failures);
    }

    #[tokio::test]
    async fn test_process_reasoner_failure_returns_error() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let reasoner = Arc::new(FailingProposer::new());
        let executor = ExecutorRouter::new();

        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..KernelConfig::default()
        };

        let kernel = Kernel::new(store, reasoner.clone(), executor, config);
        let tx = Transaction::user_prompt(aura_core::AgentId::generate(), "test");
        let result = kernel.process(tx, 1).await;

        assert!(result.is_err());
        assert_eq!(reasoner.call_count(), 1);
    }

    #[tokio::test]
    async fn test_replay_mode() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let reasoner = Arc::new(FailingProposer::new());
        let executor = ExecutorRouter::new();

        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            replay_mode: true,
            ..KernelConfig::default()
        };

        let kernel = Kernel::new(store, reasoner.clone(), executor, config);

        let tx = Transaction::user_prompt(aura_core::AgentId::generate(), "test");
        let result = kernel.process(tx, 1).await.unwrap();

        assert_eq!(result.entry.seq, 1);
        assert_eq!(reasoner.call_count(), 0);
    }
}
