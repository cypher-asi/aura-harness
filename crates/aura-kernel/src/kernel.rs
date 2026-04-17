//! Kernel implementation.
//!
//! ## Processing Invariants
//!
//! Every call to [`Kernel::process_direct`] or [`Kernel::process_dequeued`]
//! upholds the following guarantees:
//!
//! 1. **Deterministic context** — the context hash is derived solely from the
//!    incoming transaction and the record window loaded from the store.
//!    Re-processing the same inputs always yields the same context hash.
//!
//! 2. **Complete recording** — every intermediate artifact (proposals, policy
//!    decisions, actions, effects) is captured in the returned [`RecordEntry`]
//!    so the step can be replayed without a live reasoner or executor.
//!
//! 3. **Monotonic sequencing** — the internal counter guarantees strictly
//!    increasing sequence numbers without requiring the caller to supply them.

use crate::executor::ExecuteContext;
use crate::policy::{Policy, PolicyConfig};
use crate::ExecutorRouter;
use aura_core::{
    Action, ActionId, ActionKind, AgentId, Decision, Effect, EffectStatus, Proposal, ProposalSet,
    RecordEntry, RuntimeCapabilityInstall, ToolCall, ToolProposal, Transaction, TransactionType,
};
use aura_reasoner::{ModelProvider, ModelRequest, ModelResponse, StreamEventStream};
use aura_store::{DequeueToken, Store};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::instrument;

#[cfg(test)]
use tracing::{debug, info, warn};

// ============================================================================
// Configuration
// ============================================================================

/// Kernel configuration.
#[derive(Debug, Clone)]
pub struct KernelConfig {
    /// Size of record window for context
    pub record_window_size: usize,
    /// Policy configuration
    pub policy: PolicyConfig,
    /// Base workspace directory
    pub workspace_base: PathBuf,
    /// When true, use `workspace_base` directly instead of appending `agent_id`.
    pub use_workspace_base_as_root: bool,
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
            use_workspace_base_as_root: false,
            replay_mode: false,
            proposal_timeout_ms: 120_000,
        }
    }
}

// ============================================================================
// Result types
// ============================================================================

/// Output from a single tool execution within the kernel.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Tool use ID (from the model's `tool_use` block).
    pub tool_use_id: String,
    /// Result content (text or error message).
    pub content: String,
    /// Whether the tool execution failed.
    pub is_error: bool,
}

/// Result of processing a transaction.
#[derive(Debug)]
pub struct ProcessResult {
    /// The record entry created
    pub entry: RecordEntry,
    /// Tool output, if a tool was executed or denied
    pub tool_output: Option<ToolOutput>,
    /// Whether any actions failed
    pub had_failures: bool,
    /// Persisted runtime capability snapshot written by this transaction.
    pub runtime_capability_update: Option<RuntimeCapabilityInstall>,
    /// Whether the persisted runtime capability ledger should be cleared.
    pub clear_runtime_capabilities: bool,
}

/// Result of a reasoning call.
#[derive(Debug)]
pub struct ReasonResult {
    /// The record entry created
    pub entry: RecordEntry,
    /// The model response
    pub response: ModelResponse,
}

// ============================================================================
// Streaming handle
// ============================================================================

/// Handle returned alongside a streaming response so the caller can finalize
/// the record entry once the stream completes.
pub struct ReasonStreamHandle {
    kernel_store: Arc<dyn Store>,
    agent_id: AgentId,
    seq_counter: Arc<Mutex<u64>>,
    #[allow(dead_code)]
    config: KernelConfig,
}

impl ReasonStreamHandle {
    fn next_seq(&self) -> u64 {
        let mut seq = self
            .seq_counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = *seq;
        *seq += 1;
        current
    }

    /// Record a successfully completed streaming response.
    ///
    /// # Errors
    /// Returns error if serialization or store append fails.
    pub fn record_completed(
        &self,
        response: &ModelResponse,
    ) -> Result<RecordEntry, crate::KernelError> {
        let seq = self.next_seq();

        let reasoning_payload = serde_json::json!({
            "model": response.trace.model,
            "stop_reason": format!("{:?}", response.stop_reason),
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
        });
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        let entry = RecordEntry::builder(seq, tx)
            .context_hash([0u8; 32])
            .build();

        self.kernel_store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;

        Ok(entry)
    }

    /// Record a failed streaming response.
    ///
    /// # Errors
    /// Returns error if serialization or store append fails.
    pub fn record_failed(&self, error: &str) -> Result<RecordEntry, crate::KernelError> {
        let seq = self.next_seq();

        let reasoning_payload = serde_json::json!({
            "error": error,
            "status": "failed",
        });
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        let entry = RecordEntry::builder(seq, tx)
            .context_hash([0u8; 32])
            .build();

        self.kernel_store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;

        Ok(entry)
    }
}

// ============================================================================
// Kernel (concrete type with dynamic dispatch)
// ============================================================================

/// The deterministic kernel.
///
/// Uses `Arc<dyn Store>` and `Arc<dyn ModelProvider>` for dynamic dispatch,
/// removing the generic type parameters from the Phase-1 design.
pub struct Kernel {
    store: Arc<dyn Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    executor: ExecutorRouter,
    policy: Policy,
    config: KernelConfig,
    /// Agent this kernel instance is bound to.
    pub agent_id: AgentId,
    seq: Arc<Mutex<u64>>,
}

impl Kernel {
    /// Create a new kernel bound to a specific agent.
    ///
    /// Reads the current head sequence from the store so the internal counter
    /// starts at `head_seq + 1`.
    ///
    /// # Errors
    /// Returns error if the store cannot be read.
    pub fn new(
        store: Arc<dyn Store>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        executor: ExecutorRouter,
        config: KernelConfig,
        agent_id: AgentId,
    ) -> Result<Self, crate::KernelError> {
        let head_seq = store
            .get_head_seq(agent_id)
            .map_err(|e| crate::KernelError::Store(format!("get_head_seq: {e}")))?;
        let policy = Policy::new(config.policy.clone());
        Ok(Self {
            store,
            provider,
            executor,
            policy,
            config,
            agent_id,
            seq: Arc::new(Mutex::new(head_seq + 1)),
        })
    }

    /// Get a reference to the underlying store.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    // -----------------------------------------------------------------------
    // Sequence helpers
    // -----------------------------------------------------------------------

    fn next_seq(&self) -> u64 {
        let mut seq = self
            .seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = *seq;
        *seq += 1;
        current
    }

    fn reserve_seq_range(&self, count: usize) -> u64 {
        let mut seq = self
            .seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = *seq;
        *seq += count as u64;
        base
    }

    fn agent_workspace(&self) -> PathBuf {
        if self.config.use_workspace_base_as_root {
            self.config.workspace_base.clone()
        } else {
            self.config.workspace_base.join(self.agent_id.to_hex())
        }
    }

    // -----------------------------------------------------------------------
    // Context helpers
    // -----------------------------------------------------------------------

    fn compute_context_hash(
        tx: &Transaction,
        window: &[RecordEntry],
    ) -> Result<[u8; 32], crate::KernelError> {
        let tx_bytes = serde_json::to_vec(tx)
            .map_err(|e| crate::KernelError::Serialization(format!("serialize tx: {e}")))?;
        let mut hasher = aura_core::hash::Hasher::new();
        hasher.update(&tx_bytes);
        for entry in window {
            hasher.update(&entry.context_hash);
        }
        Ok(hasher.finalize())
    }

    fn load_window(&self, next_seq: u64) -> Result<Vec<RecordEntry>, crate::KernelError> {
        let from_seq = next_seq.saturating_sub(self.config.record_window_size as u64);
        self.store
            .scan_record(self.agent_id, from_seq, self.config.record_window_size)
            .map_err(|e| crate::KernelError::Store(format!("scan_record: {e}")))
    }

    fn load_runtime_capabilities(
        &self,
    ) -> Result<Option<RuntimeCapabilityInstall>, crate::KernelError> {
        self.store
            .get_runtime_capabilities(self.agent_id)
            .map_err(|e| crate::KernelError::Store(format!("get_runtime_capabilities: {e}")))
    }

    // -----------------------------------------------------------------------
    // Public processing methods
    // -----------------------------------------------------------------------

    /// Process a transaction from a direct (non-inbox) source.
    ///
    /// # Errors
    /// Returns error if processing or storage fails.
    pub async fn process_direct(
        &self,
        tx: Transaction,
    ) -> Result<ProcessResult, crate::KernelError> {
        let seq = self.next_seq();
        let result = self.process_tx(&tx, seq).await?;
        self.store
            .append_entry_direct_with_runtime_capabilities(
                self.agent_id,
                seq,
                &result.entry,
                result.runtime_capability_update.as_ref(),
                result.clear_runtime_capabilities,
            )
            .map_err(|e| {
                crate::KernelError::Store(format!(
                    "append_entry_direct_with_runtime_capabilities: {e}"
                ))
            })?;
        Ok(result)
    }

    /// Process a transaction dequeued from the inbox.
    ///
    /// # Errors
    /// Returns error if processing or storage fails.
    pub async fn process_dequeued(
        &self,
        tx: Transaction,
        token: DequeueToken,
    ) -> Result<ProcessResult, crate::KernelError> {
        let seq = self.next_seq();
        let result = self.process_tx(&tx, seq).await?;
        self.store
            .append_entry_dequeued_with_runtime_capabilities(
                self.agent_id,
                seq,
                &result.entry,
                token,
                result.runtime_capability_update.as_ref(),
                result.clear_runtime_capabilities,
            )
            .map_err(|e| {
                crate::KernelError::Store(format!(
                    "append_entry_dequeued_with_runtime_capabilities: {e}"
                ))
            })?;
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Internal dispatch
    // -----------------------------------------------------------------------

    async fn process_tx(
        &self,
        tx: &Transaction,
        seq: u64,
    ) -> Result<ProcessResult, crate::KernelError> {
        let window = self.load_window(seq)?;
        let context_hash = Self::compute_context_hash(tx, &window)?;

        match tx.tx_type {
            TransactionType::ToolProposal => {
                self.process_tool_proposal(tx, seq, context_hash).await
            }
            TransactionType::SessionStart => {
                self.policy.clear_session_approvals();
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: true,
                })
            }
            TransactionType::System => {
                let runtime_capability_update = Self::runtime_capability_update_from_tx(tx)
                    .map_err(|e| {
                        crate::KernelError::Serialization(format!(
                            "deserialize capability install: {e}"
                        ))
                    })?;
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update,
                    clear_runtime_capabilities: false,
                })
            }
            _ => {
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                })
            }
        }
    }

    fn runtime_capability_update_from_tx(
        tx: &Transaction,
    ) -> Result<Option<RuntimeCapabilityInstall>, serde_json::Error> {
        if tx.tx_type != TransactionType::System {
            return Ok(None);
        }

        let payload = match serde_json::from_slice::<serde_json::Value>(&tx.payload) {
            Ok(payload) => payload,
            Err(_) => return Ok(None),
        };
        let is_capability_install = payload
            .get("system_kind")
            .and_then(serde_json::Value::as_str)
            == Some("capability_install");

        if is_capability_install {
            serde_json::from_value(payload).map(Some)
        } else {
            Ok(None)
        }
    }

    // -----------------------------------------------------------------------
    // Tool proposal processing
    // -----------------------------------------------------------------------

    #[instrument(skip(self, tx), fields(seq))]
    async fn process_tool_proposal(
        &self,
        tx: &Transaction,
        seq: u64,
        context_hash: [u8; 32],
    ) -> Result<ProcessResult, crate::KernelError> {
        let proposal: ToolProposal = serde_json::from_slice(&tx.payload).map_err(|e| {
            crate::KernelError::Serialization(format!("deserialize ToolProposal: {e}"))
        })?;

        let tool_use_id = proposal.tool_use_id.clone();
        let tool_name = proposal.tool.clone();

        let kernel_proposal = Proposal::new(
            ActionKind::Delegate,
            serde_json::to_vec(&ToolCall::new(&proposal.tool, proposal.args.clone()))
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?,
        );

        let runtime_capabilities = self.load_runtime_capabilities()?;
        let policy_result = self.policy.check_tool_with_runtime_capabilities(
            &tool_name,
            &proposal.args,
            runtime_capabilities.as_ref(),
        );

        if policy_result.allowed {
            let action_id = ActionId::generate();
            let action = Action::new(
                action_id,
                ActionKind::Delegate,
                kernel_proposal.payload.clone(),
            );

            let workspace = self.agent_workspace();
            tokio::fs::create_dir_all(&workspace)
                .await
                .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;
            let ctx = ExecuteContext::new(self.agent_id, action_id, workspace);
            let effect = self.executor.execute(&ctx, &action).await;

            let had_failures = effect.status == EffectStatus::Failed;
            let output_content = String::from_utf8_lossy(&effect.payload).to_string();

            let mut decision = Decision::new();
            decision.accept(action_id);

            let mut proposals = ProposalSet::new();
            proposals.proposals.push(kernel_proposal);

            let entry = RecordEntry::builder(seq, tx.clone())
                .context_hash(context_hash)
                .proposals(proposals)
                .decision(decision)
                .actions(vec![action])
                .effects(vec![effect])
                .build();

            Ok(ProcessResult {
                entry,
                tool_output: Some(ToolOutput {
                    tool_use_id,
                    content: output_content,
                    is_error: had_failures,
                }),
                had_failures,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
            })
        } else {
            let mut decision = Decision::new();
            #[allow(clippy::cast_possible_truncation)]
            decision.reject(0, policy_result.reason.as_deref().unwrap_or("denied"));

            let mut proposals = ProposalSet::new();
            proposals.proposals.push(kernel_proposal);

            let denial_reason = policy_result
                .reason
                .unwrap_or_else(|| "Policy denied".to_string());

            let entry = RecordEntry::builder(seq, tx.clone())
                .context_hash(context_hash)
                .proposals(proposals)
                .decision(decision)
                .build();

            Ok(ProcessResult {
                entry,
                tool_output: Some(ToolOutput {
                    tool_use_id,
                    content: denial_reason,
                    is_error: true,
                }),
                had_failures: false,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
            })
        }
    }

    // -----------------------------------------------------------------------
    // Batch tool processing
    // -----------------------------------------------------------------------

    /// Process a batch of tool proposals, executing approved tools in parallel.
    ///
    /// # Errors
    /// Returns error if serialization, execution, or storage fails.
    #[allow(clippy::too_many_lines)]
    pub async fn process_tools(
        &self,
        tool_proposals: Vec<ToolProposal>,
    ) -> Result<Vec<ProcessResult>, crate::KernelError> {
        if tool_proposals.is_empty() {
            return Ok(vec![]);
        }

        // Classify each proposal
        let mut approved = Vec::new();
        let mut denied = Vec::new();
        let runtime_capabilities = self.load_runtime_capabilities()?;

        for proposal in &tool_proposals {
            let result = self.policy.check_tool_with_runtime_capabilities(
                &proposal.tool,
                &proposal.args,
                runtime_capabilities.as_ref(),
            );
            if result.allowed {
                approved.push(proposal);
            } else {
                denied.push((proposal, result));
            }
        }

        // Prepare workspace
        let workspace = self.agent_workspace();
        tokio::fs::create_dir_all(&workspace)
            .await
            .map_err(|e| crate::KernelError::Internal(format!("create workspace: {e}")))?;

        // Build actions and contexts for all approved tools
        let mut exec_contexts: Vec<ExecuteContext> = Vec::new();
        let mut exec_actions: Vec<(&ToolProposal, Action)> = Vec::new();

        for proposal in &approved {
            let action_id = ActionId::generate();
            let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
            let payload = serde_json::to_vec(&tool_call)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
            let action = Action::new(action_id, ActionKind::Delegate, payload);
            let ctx = ExecuteContext::new(self.agent_id, action_id, workspace.clone());

            exec_contexts.push(ctx);
            exec_actions.push((proposal, action));
        }

        // Execute in parallel — borrow from the collected Vecs so lifetimes work
        let exec_futures: Vec<_> = exec_contexts
            .iter()
            .zip(exec_actions.iter())
            .map(|(ctx, (_, action))| self.executor.execute(ctx, action))
            .collect();

        let effects: Vec<Effect> = futures_util::future::join_all(exec_futures).await;

        // Reserve contiguous sequences for ALL results
        let total = tool_proposals.len();
        let base_seq = self.reserve_seq_range(total);

        // Build results in input order
        let mut results = Vec::with_capacity(total);
        let mut entries = Vec::with_capacity(total);
        let mut approved_idx = 0;
        let mut denied_idx = 0;

        for (i, proposal) in tool_proposals.iter().enumerate() {
            let seq = base_seq + i as u64;
            let tx = Transaction::tool_proposal(self.agent_id, proposal)
                .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

            let window = self.load_window(seq)?;
            let context_hash = Self::compute_context_hash(&tx, &window)?;
            let tool_use_id = proposal.tool_use_id.clone();

            // Check if this proposal was approved (match order)
            let was_approved = self
                .policy
                .check_tool_with_runtime_capabilities(
                    &proposal.tool,
                    &proposal.args,
                    runtime_capabilities.as_ref(),
                )
                .allowed;

            if was_approved {
                let (_, action) = &exec_actions[approved_idx];
                let effect = &effects[approved_idx];
                approved_idx += 1;

                let had_failures = effect.status == EffectStatus::Failed;
                let output_content = String::from_utf8_lossy(&effect.payload).to_string();

                let mut decision = Decision::new();
                decision.accept(action.action_id);

                let kernel_proposal = Proposal::new(ActionKind::Delegate, action.payload.clone());
                let mut proposal_set = ProposalSet::new();
                proposal_set.proposals.push(kernel_proposal);

                let entry = RecordEntry::builder(seq, tx)
                    .context_hash(context_hash)
                    .proposals(proposal_set)
                    .decision(decision)
                    .actions(vec![action.clone()])
                    .effects(vec![effect.clone()])
                    .build();

                entries.push(entry.clone());
                results.push(ProcessResult {
                    entry,
                    tool_output: Some(ToolOutput {
                        tool_use_id,
                        content: output_content,
                        is_error: had_failures,
                    }),
                    had_failures,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                });
            } else {
                let (_, policy_result) = &denied[denied_idx];
                denied_idx += 1;

                let denial_reason = policy_result
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Policy denied".to_string());
                let mut decision = Decision::new();
                decision.reject(0, &denial_reason);

                let tool_call = ToolCall::new(&proposal.tool, proposal.args.clone());
                let payload_bytes = serde_json::to_vec(&tool_call)
                    .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;
                let kernel_proposal = Proposal::new(ActionKind::Delegate, payload_bytes);
                let mut proposal_set = ProposalSet::new();
                proposal_set.proposals.push(kernel_proposal);

                let entry = RecordEntry::builder(seq, tx)
                    .context_hash(context_hash)
                    .proposals(proposal_set)
                    .decision(decision)
                    .build();

                entries.push(entry.clone());
                results.push(ProcessResult {
                    entry,
                    tool_output: Some(ToolOutput {
                        tool_use_id,
                        content: denial_reason,
                        is_error: true,
                    }),
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                });
            }
        }

        // Atomic batch write
        self.store
            .append_entries_batch(self.agent_id, base_seq, &entries)
            .map_err(|e| crate::KernelError::Store(format!("append_entries_batch: {e}")))?;

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Reasoning
    // -----------------------------------------------------------------------

    /// Call the model provider and record the result.
    ///
    /// # Errors
    /// Returns error if the model call or storage fails.
    pub async fn reason(&self, request: ModelRequest) -> Result<ReasonResult, crate::KernelError> {
        let seq = self.next_seq();

        let response = self
            .provider
            .complete(request)
            .await
            .map_err(|e| crate::KernelError::Reasoner(e.to_string()))?;

        let reasoning_payload = serde_json::json!({
            "model": response.trace.model,
            "stop_reason": format!("{:?}", response.stop_reason),
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
        });
        let payload_bytes = serde_json::to_vec(&reasoning_payload)
            .map_err(|e| crate::KernelError::Serialization(e.to_string()))?;

        let tx = Transaction::new_chained(
            self.agent_id,
            TransactionType::Reasoning,
            payload_bytes,
            None,
        );

        let window = self.load_window(seq)?;
        let context_hash = Self::compute_context_hash(&tx, &window)?;

        let entry = RecordEntry::builder(seq, tx)
            .context_hash(context_hash)
            .build();

        self.store
            .append_entry_direct(self.agent_id, seq, &entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;

        Ok(ReasonResult { entry, response })
    }

    /// Start a streaming reasoning call.
    ///
    /// Returns a handle for finalizing the record entry and the event stream.
    ///
    /// # Errors
    /// Returns error if the model call fails.
    pub async fn reason_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<(ReasonStreamHandle, StreamEventStream), crate::KernelError> {
        let stream = self
            .provider
            .complete_streaming(request)
            .await
            .map_err(|e| crate::KernelError::Reasoner(e.to_string()))?;

        let handle = ReasonStreamHandle {
            kernel_store: self.store.clone(),
            agent_id: self.agent_id,
            seq_counter: self.seq.clone(),
            config: self.config.clone(),
        };

        Ok((handle, stream))
    }
}

// ============================================================================
// Legacy module — keeps the old generic Kernel alive for existing tests
// ============================================================================

#[cfg(test)]
pub mod legacy {
    use super::*;
    use aura_core::{Action, ActionId, Decision, Effect, EffectStatus, ProposalSet, Transaction};
    use aura_reasoner::ProposeRequest;
    use tokio::time::{timeout, Duration};

    #[async_trait::async_trait]
    pub trait Proposer: Send + Sync {
        async fn propose(
            &self,
            request: ProposeRequest,
        ) -> Result<ProposalSet, aura_reasoner::ReasonerError>;
    }

    pub struct LegacyKernel<S: Store, R: Proposer> {
        store: Arc<S>,
        reasoner: Arc<R>,
        executor: ExecutorRouter,
        policy: Policy,
        config: KernelConfig,
    }

    impl<S: Store, R: Proposer> LegacyKernel<S, R> {
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

        fn agent_workspace(&self, agent_id: &AgentId) -> PathBuf {
            if self.config.use_workspace_base_as_root {
                self.config.workspace_base.clone()
            } else {
                self.config.workspace_base.join(agent_id.to_hex())
            }
        }

        pub async fn process(
            &self,
            tx: Transaction,
            next_seq: u64,
        ) -> Result<ProcessResult, crate::KernelError> {
            info!(seq = next_seq, "Processing transaction (legacy)");

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

            let context = crate::context::ContextBuilder::new(&tx)
                .map_err(|e| {
                    crate::KernelError::Serialization(format!(
                        "serializing transaction {}: {e}",
                        tx.hash
                    ))
                })?
                .with_record_window(window)
                .build();

            let proposals = if self.config.replay_mode {
                debug!("Replay mode: skipping reasoner");
                ProposalSet::new()
            } else {
                let mut p = self.get_proposals(&tx, &context).await?;
                let max = self.policy.max_proposals();
                if p.proposals.len() > max {
                    warn!(
                        count = p.proposals.len(),
                        max, "Truncating proposals to max_proposals limit"
                    );
                    p.proposals.truncate(max);
                }
                p
            };

            let (actions, decision) = self.apply_policy(&proposals);
            debug!(
                accepted = decision.accepted_action_ids.len(),
                rejected = decision.rejected.len(),
                "Policy applied"
            );

            let effects = if self.config.replay_mode {
                debug!("Replay mode: skipping execution");
                vec![]
            } else {
                self.execute_actions(&tx.agent_id, &actions).await?
            };

            let had_failures = effects.iter().any(|e| e.status == EffectStatus::Failed);
            if had_failures {
                warn!("Some actions failed");
            }

            let entry = RecordEntry::builder(next_seq, tx)
                .context_hash(context.context_hash)
                .proposals(proposals)
                .decision(decision)
                .actions(actions)
                .effects(effects)
                .build();

            info!(seq = next_seq, "Transaction processed (legacy)");

            Ok(ProcessResult {
                entry,
                tool_output: None,
                had_failures,
                runtime_capability_update: None,
                clear_runtime_capabilities: false,
            })
        }

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
                    tracing::error!(error = %e, "Reasoner failed");
                    Err(crate::KernelError::Reasoner(e.to_string()))
                }
                Err(_) => {
                    tracing::error!(
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

        fn apply_policy(&self, proposals: &ProposalSet) -> (Vec<Action>, Decision) {
            let mut actions = Vec::new();
            let mut decision = Decision::new();

            for (idx, proposal) in proposals.proposals.iter().enumerate() {
                let result = self.policy.check(proposal);

                if result.allowed {
                    let action_id = ActionId::generate();
                    let action =
                        Action::new(action_id, proposal.action_kind, proposal.payload.clone());
                    actions.push(action);
                    decision.accept(action_id);
                } else {
                    #[allow(clippy::cast_possible_truncation)]
                    decision.reject(idx as u32, result.reason.unwrap_or_default());
                }
            }

            (actions, decision)
        }

        async fn execute_actions(
            &self,
            agent_id: &AgentId,
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
}

// ============================================================================
// Legacy tests (adapted from the original generic kernel)
// ============================================================================

#[cfg(test)]
mod legacy_tests {
    use super::legacy::*;
    use super::*;
    use aura_store::RocksStore;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    struct EmptyProposer;

    #[async_trait::async_trait]
    impl Proposer for EmptyProposer {
        async fn propose(
            &self,
            _request: aura_reasoner::ProposeRequest,
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
            _request: aura_reasoner::ProposeRequest,
        ) -> Result<ProposalSet, aura_reasoner::ReasonerError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(aura_reasoner::ReasonerError::Internal(
                "configured to fail".into(),
            ))
        }
    }

    fn create_test_kernel() -> (LegacyKernel<RocksStore, EmptyProposer>, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();

        let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let reasoner = Arc::new(EmptyProposer);
        let executor = ExecutorRouter::new();

        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..KernelConfig::default()
        };

        let kernel = LegacyKernel::new(store, reasoner, executor, config);
        (kernel, db_dir, ws_dir)
    }

    #[tokio::test]
    async fn test_process_empty_proposals() {
        let (kernel, _db_dir, _ws_dir) = create_test_kernel();

        let tx = Transaction::user_prompt(AgentId::generate(), "test");
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

        let kernel = LegacyKernel::new(store, reasoner.clone(), executor, config);
        let tx = Transaction::user_prompt(AgentId::generate(), "test");
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

        let kernel = LegacyKernel::new(store, reasoner.clone(), executor, config);

        let tx = Transaction::user_prompt(AgentId::generate(), "test");
        let result = kernel.process(tx, 1).await.unwrap();

        assert_eq!(result.entry.seq, 1);
        assert_eq!(reasoner.call_count(), 0);
    }
}

// ============================================================================
// New kernel tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{
        InstalledIntegrationDefinition, InstalledToolCapability,
        InstalledToolIntegrationRequirement, RuntimeCapabilityInstall, SystemKind,
    };
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_new_kernel() -> (Kernel, TempDir, TempDir) {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test response"));
        let executor = ExecutorRouter::new();
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            ..KernelConfig::default()
        };
        let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
        (kernel, db_dir, ws_dir)
    }

    #[tokio::test]
    async fn test_process_direct_user_prompt() {
        let (kernel, _db, _ws) = create_new_kernel();
        let tx = Transaction::user_prompt(kernel.agent_id, "hello");
        let result = kernel.process_direct(tx).await.unwrap();
        assert_eq!(result.entry.seq, 1);
        assert!(!result.had_failures);
        assert!(result.tool_output.is_none());
    }

    #[tokio::test]
    async fn test_process_direct_increments_seq() {
        let (kernel, _db, _ws) = create_new_kernel();
        let tx1 = Transaction::user_prompt(kernel.agent_id, "first");
        let r1 = kernel.process_direct(tx1).await.unwrap();
        assert_eq!(r1.entry.seq, 1);

        let tx2 = Transaction::user_prompt(kernel.agent_id, "second");
        let r2 = kernel.process_direct(tx2).await.unwrap();
        assert_eq!(r2.entry.seq, 2);
    }

    #[test]
    fn test_agent_workspace_defaults_to_agent_subdirectory() {
        let (kernel, _db, ws_dir) = create_new_kernel();
        assert_eq!(
            kernel.agent_workspace(),
            ws_dir.path().join(kernel.agent_id.to_hex())
        );
    }

    #[test]
    fn test_agent_workspace_can_use_workspace_base_directly() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test response"));
        let executor = ExecutorRouter::new();
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            use_workspace_base_as_root: true,
            ..KernelConfig::default()
        };
        let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();

        assert_eq!(kernel.agent_workspace(), ws_dir.path());
    }

    #[tokio::test]
    async fn test_reason_records_and_returns_response() {
        let (kernel, _db, _ws) = create_new_kernel();
        let request = ModelRequest::builder("test-model", "system prompt")
            .message(aura_reasoner::Message::user("hello"))
            .build();
        let result = kernel.reason(request).await.unwrap();
        assert_eq!(result.entry.seq, 1);
        assert!(!result.response.message.content.is_empty());
    }

    #[tokio::test]
    async fn test_sequence_across_process_and_reason() {
        let (kernel, _db, _ws) = create_new_kernel();

        let tx = Transaction::user_prompt(kernel.agent_id, "prompt");
        let r1 = kernel.process_direct(tx).await.unwrap();
        assert_eq!(r1.entry.seq, 1);

        let request = ModelRequest::builder("test-model", "system")
            .message(aura_reasoner::Message::user("test"))
            .build();
        let r2 = kernel.reason(request).await.unwrap();
        assert_eq!(r2.entry.seq, 2);

        let tx2 = Transaction::new_chained(
            kernel.agent_id,
            TransactionType::AgentMsg,
            "response".as_bytes().to_vec(),
            None,
        );
        let r3 = kernel.process_direct(tx2).await.unwrap();
        assert_eq!(r3.entry.seq, 3);
    }

    #[tokio::test]
    async fn test_session_start_clears_policy_session_approvals() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test response"));
        let executor = ExecutorRouter::new();
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            policy: PolicyConfig::default()
                .with_tool_permission("guarded_tool", crate::policy::PermissionLevel::AskOnce),
            ..KernelConfig::default()
        };
        let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
        kernel.policy.approve_for_session("guarded_tool");
        assert!(kernel.policy.is_session_approved("guarded_tool"));

        kernel
            .process_direct(Transaction::session_start(agent_id))
            .await
            .unwrap();

        assert!(!kernel.policy.is_session_approved("guarded_tool"));
    }

    #[tokio::test]
    async fn test_tool_proposal_denied_without_required_integration() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test response"));
        let executor = ExecutorRouter::new();
        let mut policy = PolicyConfig::default();
        policy.add_allowed_tool("brave_search_web");
        policy.set_tool_integration_requirements([(
            "brave_search_web".to_string(),
            InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            },
        )]);
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            policy,
            ..KernelConfig::default()
        };
        let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
        let proposal = ToolProposal::new(
            "tool-use-1",
            "brave_search_web",
            serde_json::json!({ "query": "aura os" }),
        );
        let tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();

        let result = kernel.process_direct(tx).await.unwrap();

        assert!(result
            .tool_output
            .as_ref()
            .is_some_and(|output| output.is_error));
        assert!(result
            .tool_output
            .as_ref()
            .and_then(|output| Some(output.content.contains("installed integration")))
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn test_capability_install_persists_runtime_capability_ledger() {
        let (kernel, _db, _ws) = create_new_kernel();
        let runtime_capabilities = RuntimeCapabilityInstall {
            system_kind: SystemKind::CapabilityInstall,
            scope: "session".to_string(),
            session_id: Some("session-1".to_string()),
            installed_integrations: vec![InstalledIntegrationDefinition {
                integration_id: "integration-brave-1".to_string(),
                name: "Brave Search".to_string(),
                provider: "brave_search".to_string(),
                kind: "workspace_integration".to_string(),
                metadata: HashMap::new(),
            }],
            installed_tools: vec![InstalledToolCapability {
                name: "brave_search_web".to_string(),
                required_integration: Some(InstalledToolIntegrationRequirement {
                    integration_id: None,
                    provider: Some("brave_search".to_string()),
                    kind: Some("workspace_integration".to_string()),
                }),
            }],
        };
        let tx = Transaction::new_chained(
            kernel.agent_id,
            TransactionType::System,
            serde_json::to_vec(&runtime_capabilities).unwrap(),
            None,
        );

        kernel.process_direct(tx).await.unwrap();

        let persisted = kernel
            .store()
            .get_runtime_capabilities(kernel.agent_id)
            .unwrap();
        assert_eq!(persisted, Some(runtime_capabilities));
    }

    #[tokio::test]
    async fn test_session_start_clears_persisted_runtime_capability_ledger() {
        let (kernel, _db, _ws) = create_new_kernel();
        let runtime_capabilities = RuntimeCapabilityInstall {
            system_kind: SystemKind::CapabilityInstall,
            scope: "session".to_string(),
            session_id: Some("session-1".to_string()),
            installed_integrations: vec![],
            installed_tools: vec![InstalledToolCapability {
                name: "brave_search_web".to_string(),
                required_integration: None,
            }],
        };
        let capability_tx = Transaction::new_chained(
            kernel.agent_id,
            TransactionType::System,
            serde_json::to_vec(&runtime_capabilities).unwrap(),
            None,
        );
        kernel.process_direct(capability_tx).await.unwrap();
        assert!(kernel
            .store()
            .get_runtime_capabilities(kernel.agent_id)
            .unwrap()
            .is_some());

        kernel
            .process_direct(Transaction::session_start(kernel.agent_id))
            .await
            .unwrap();

        assert_eq!(
            kernel
                .store()
                .get_runtime_capabilities(kernel.agent_id)
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn test_tool_proposal_uses_persisted_runtime_capability_ledger() {
        let db_dir = TempDir::new().unwrap();
        let ws_dir = TempDir::new().unwrap();
        let agent_id = AgentId::generate();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("test response"));
        let executor = ExecutorRouter::new();
        let mut policy = PolicyConfig::default();
        policy.add_allowed_tool("brave_search_web");
        policy.set_installed_integrations([InstalledIntegrationDefinition {
            integration_id: "integration-brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: HashMap::new(),
        }]);
        policy.set_tool_integration_requirements([(
            "brave_search_web".to_string(),
            InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            },
        )]);
        let config = KernelConfig {
            workspace_base: ws_dir.path().to_path_buf(),
            policy,
            ..KernelConfig::default()
        };
        let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();

        let empty_runtime_capabilities = RuntimeCapabilityInstall {
            system_kind: SystemKind::CapabilityInstall,
            scope: "session".to_string(),
            session_id: Some("session-1".to_string()),
            installed_integrations: vec![],
            installed_tools: vec![],
        };
        let capability_tx = Transaction::new_chained(
            kernel.agent_id,
            TransactionType::System,
            serde_json::to_vec(&empty_runtime_capabilities).unwrap(),
            None,
        );
        kernel.process_direct(capability_tx).await.unwrap();

        let proposal = ToolProposal::new(
            "tool-use-1",
            "brave_search_web",
            serde_json::json!({ "query": "aura os" }),
        );
        let tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();
        let result = kernel.process_direct(tx).await.unwrap();

        assert!(result
            .tool_output
            .as_ref()
            .is_some_and(|output| output.is_error));
        assert!(result
            .tool_output
            .as_ref()
            .map(|output| output.content.contains("kernel runtime capability ledger"))
            .unwrap_or(false));
    }
}
