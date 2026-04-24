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
//!
//! The implementation is split across sibling files for readability:
//! - [`process`] — top-level `process_direct` / `process_dequeued` and the
//!   shared `process_tx` dispatcher plus the `System` capability-install
//!   decoder.
//! - [`tools`]   — `process_tool_proposal` and the batch `process_tools`.
//! - [`reason`]  — `reason` / `reason_streaming` and their timeout wrappers.
//! - [`stream`]  — the `ReasonStreamHandle` finalization handle.
//! - [`tests`]   — integration tests that exercise the full `Kernel` surface.

use crate::policy::{ApprovalRegistry, Policy, PolicyConfig};
use crate::ExecutorRouter;
use async_trait::async_trait;
use aura_core::{AgentId, RecordEntry, RuntimeCapabilityInstall, ToolState};
use aura_reasoner::ModelProvider;
use aura_store::Store;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod process;
mod reason;
mod stream;
#[cfg(test)]
mod tests;
mod tools;

pub use stream::ReasonStreamHandle;

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
    /// Per-tool execution timeout in milliseconds. Each individual tool in
    /// a batch is wrapped in a `tokio::time::timeout` with this budget; on
    /// expiration a failed `Effect` is emitted and the batch continues.
    pub tool_timeout_ms: u64,
    /// Live approval bridge for tri-state `ask` tool calls. When absent,
    /// `ask` resolves to a headless deny.
    pub tool_approval_prompter: Option<Arc<dyn ToolApprovalPrompter>>,
    /// Originating user id used when a live approval response is remembered
    /// forever into persisted user tool defaults.
    pub originating_user_id: Option<String>,
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
            tool_timeout_ms: 120_000,
            tool_approval_prompter: None,
            originating_user_id: None,
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
    /// Phase 6 (security audit): `true` when the kernel produced this
    /// output because the policy raised
    /// [`crate::PolicyVerdict::RequireApproval`] and no matching
    /// single-use approval was registered. Callers (router, agent
    /// loop) use this flag to surface a `423 Locked` response instead
    /// of a generic failure.
    pub approval_required: Option<ApprovalRequiredInfo>,
}

/// Details about a tool invocation that was denied because it needs an
/// out-of-band operator approval. Set on [`ToolOutput::approval_required`]
/// when the policy returns [`crate::PolicyVerdict::RequireApproval`] and
/// no matching entry exists in the [`ApprovalRegistry`].
#[derive(Debug, Clone)]
pub struct ApprovalRequiredInfo {
    /// Tool name, e.g. `"run_command"`.
    pub tool: String,
    /// Blake3 hash of the canonical JSON args the agent wanted to run.
    /// Hex-encoded in the API layer.
    pub args_hash: [u8; 32],
    /// Structured live prompt metadata for tri-state `ask` prompts.
    pub prompt: Option<PendingToolPrompt>,
}

/// Scope for remembering a live approval response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApprovalRemember {
    /// Do not cache; the next call prompts again.
    Once,
    /// Cache for the current session.
    Session,
    /// Persist to the originating user's defaults.
    Forever,
}

/// Live approval response returned by a [`ToolApprovalPrompter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolApprovalResponse {
    pub decision: ToolState,
    pub remember: ToolApprovalRemember,
}

/// Structured prompt metadata emitted when a tool resolves to `ask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingToolPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub agent_id: AgentId,
    pub remember_options: Vec<ToolApprovalRemember>,
}

/// Error returned by a live approval bridge.
#[derive(Debug, thiserror::Error)]
pub enum ToolApprovalError {
    #[error("approval prompt could not be delivered")]
    DeliveryFailed,
    #[error("approval prompt was cancelled")]
    Cancelled,
    #[error("{0}")]
    Internal(String),
}

/// Bridge from the deterministic kernel to an attached interactive client.
#[async_trait]
pub trait ToolApprovalPrompter: Send + Sync + std::fmt::Debug {
    async fn prompt(
        &self,
        prompt: PendingToolPrompt,
    ) -> Result<ToolApprovalResponse, ToolApprovalError>;
}

/// Decision produced by [`Kernel::process_tool_proposal`] for a single
/// tool call. Surfaced on [`ProcessResult`] so HTTP routers (and any
/// other caller) can distinguish "needs operator sign-off" from
/// "permanently denied" without pattern-matching on the error string.
#[derive(Debug, Clone)]
pub enum ToolDecision {
    /// Tool call was authorized and executed.
    Allowed,
    /// Tool call was permanently denied by policy. No approval will
    /// unlock it.
    Denied {
        /// Human-readable reason pulled from the policy engine.
        reason: String,
    },
    /// Tool call is awaiting an out-of-band operator approval. The
    /// caller should surface `args_hash` so an authenticated operator
    /// can register it via `Kernel::grant_approval` (or the
    /// `POST /tool-approval` HTTP endpoint).
    NeedsApproval {
        /// Human-readable reason, e.g.
        /// `"Tool 'run_command' requires approval for each use"`.
        reason: String,
        /// Blake3 hash of the canonical JSON args. Exposed to
        /// authenticated operators in the `423 Locked` response.
        args_hash: [u8; 32],
        /// Structured live prompt metadata for tri-state `ask` prompts.
        prompt: Option<PendingToolPrompt>,
    },
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
    /// Structured policy decision, set when this `ProcessResult` came
    /// from a tool-proposal path. `None` for non-tool transactions.
    pub tool_decision: Option<ToolDecision>,
}

/// Result of a reasoning call.
#[derive(Debug)]
pub struct ReasonResult {
    /// The record entry created
    pub entry: RecordEntry,
    /// The model response
    pub response: aura_reasoner::ModelResponse,
}

// ============================================================================
// Kernel (concrete type with dynamic dispatch)
// ============================================================================

/// The deterministic kernel.
///
/// Uses `Arc<dyn Store>` and `Arc<dyn ModelProvider>` for dynamic dispatch,
/// removing the generic type parameters from the Phase-1 design.
pub struct Kernel {
    pub(super) store: Arc<dyn Store>,
    pub(super) provider: Arc<dyn ModelProvider + Send + Sync>,
    pub(super) executor: ExecutorRouter,
    pub(super) policy: Policy,
    pub(super) config: KernelConfig,
    /// Agent this kernel instance is bound to.
    pub agent_id: AgentId,
    pub(super) seq: Arc<Mutex<u64>>,
    /// Shared store of pending single-use approvals consulted when the
    /// policy raises [`crate::PolicyVerdict::RequireApproval`]. The
    /// scheduler hands the same `ApprovalRegistry` handle to every
    /// per-agent kernel so a grant issued through the HTTP API
    /// survives the short-lived kernel that was active when the
    /// proposal was first denied.
    pub(super) approvals: ApprovalRegistry,
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
        Self::new_with_approvals(
            store,
            provider,
            executor,
            config,
            agent_id,
            ApprovalRegistry::new(),
        )
    }

    /// Construct a kernel with a caller-supplied [`ApprovalRegistry`].
    ///
    /// Phase 6 entry point used by the scheduler so every per-agent
    /// kernel it builds shares the same registry. Tests that want the
    /// default "per-kernel empty registry" behavior should keep using
    /// [`Self::new`].
    ///
    /// # Errors
    /// Returns an error if the store cannot be read.
    pub fn new_with_approvals(
        store: Arc<dyn Store>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        executor: ExecutorRouter,
        config: KernelConfig,
        agent_id: AgentId,
        approvals: ApprovalRegistry,
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
            approvals,
        })
    }

    /// Get a reference to the underlying store.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// Read-only accessor for the kernel's `Policy`.
    ///
    /// Required by the Wave 7 invariant-policy-matrix integration test
    /// so it can pre-seed `AskOnce` session approvals against a live
    /// kernel before submitting a tool proposal. Pure observational
    /// surface — the policy's interior mutable state is still protected
    /// by its own `Mutex`.
    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Return a handle to the shared [`ApprovalRegistry`] consulted
    /// when a tool proposal triggers [`crate::PolicyVerdict::RequireApproval`].
    ///
    /// Cloning the returned registry yields a handle that still points
    /// at the same underlying storage — that's what lets the HTTP
    /// `/tool-approval` handler talk to the scheduler's registry
    /// without holding a specific kernel instance.
    #[must_use]
    pub fn approval_registry(&self) -> ApprovalRegistry {
        self.approvals.clone()
    }

    /// Register a single-use approval for `(agent_id, tool, args_hash)`.
    ///
    /// Phase 6 (security audit) — closes the "AlwaysAsk silently
    /// downgrades to Deny" finding. The next tool proposal whose
    /// canonical Blake3 args hash equals `args_hash` runs as if the
    /// tool were `AlwaysAllow`; the registry entry is consumed on
    /// match, so a second proposal with the same hash needs a fresh
    /// grant.
    pub fn grant_approval(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) {
        self.approvals.grant(agent_id, tool, args_hash);
    }

    /// Revoke a previously granted approval without consuming it via
    /// a tool call. Returns `true` when an entry was actually removed.
    pub fn revoke_approval(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) -> bool {
        self.approvals.revoke(agent_id, tool, args_hash)
    }

    // -----------------------------------------------------------------------
    // Sequence helpers
    // -----------------------------------------------------------------------

    pub(super) fn next_seq(&self) -> u64 {
        let mut seq = self
            .seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = *seq;
        *seq += 1;
        current
    }

    pub(super) fn reserve_seq_range(&self, count: usize) -> u64 {
        let mut seq = self
            .seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = *seq;
        *seq += count as u64;
        base
    }

    pub(super) fn agent_workspace(&self) -> PathBuf {
        if self.config.use_workspace_base_as_root {
            self.config.workspace_base.clone()
        } else {
            self.config.workspace_base.join(self.agent_id.to_hex())
        }
    }

    // -----------------------------------------------------------------------
    // Context helpers
    // -----------------------------------------------------------------------

    pub(super) fn load_window(
        &self,
        next_seq: u64,
    ) -> Result<Vec<RecordEntry>, crate::KernelError> {
        let from_seq = next_seq.saturating_sub(self.config.record_window_size as u64);
        self.store
            .scan_record(self.agent_id, from_seq, self.config.record_window_size)
            .map_err(|e| crate::KernelError::Store(format!("scan_record: {e}")))
    }

    pub(super) fn load_runtime_capabilities(
        &self,
    ) -> Result<Option<RuntimeCapabilityInstall>, crate::KernelError> {
        self.store
            .get_runtime_capabilities(self.agent_id)
            .map_err(|e| crate::KernelError::Store(format!("get_runtime_capabilities: {e}")))
    }
}
