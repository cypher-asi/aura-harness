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
//! - [`config`], [`approval`], [`system_record`] — `KernelConfig`, the
//!   live tool-approval types, and the `write_system_*` record helpers.
//! - [`window`]  — the context hashes of the newest entries, kept so an
//!   append need not read the record window back from the store.
//! - [`tests`]   — integration tests that exercise the full `Kernel` surface.

use crate::policy::Policy;
use crate::replay::{ReplayConsumer, ReplayReport};
use crate::ExecutorRouter;
use aura_core_modes::KernelMode;
use aura_core_types::{
    AgentId, ContextHash, Hash, PolicyDecision, RecordEntry, RuntimeCapabilityInstall,
};
use aura_exec_traits::RunScratch;
use aura_model::{ModelProvider, ModelRequest};
use aura_store_db::Store;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod approval;
#[cfg(test)]
mod cache_salt_tests;
mod config;
#[cfg(test)]
mod mode_switch_tests;
mod process;
mod reason;
mod seq;
mod stream;
mod system_record;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_audit_tests;
mod tools;
mod window;

pub use approval::{
    ApprovalRequiredInfo, PendingToolPrompt, ToolApprovalError, ToolApprovalPrompter,
    ToolApprovalRemember, ToolApprovalResponse,
};
pub use config::KernelConfig;
pub use stream::ReasonStreamHandle;
pub use system_record::{write_system_entry, write_system_record};
pub use tools::{PresetToolResult, ToolRun};

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
    /// Machine-readable result classification.
    pub kind: aura_core_types::ToolResultKind,
    /// Set when the kernel produced this output because the policy
    /// raised [`crate::PolicyVerdict::RequireApproval`].
    pub approval_required: Option<ApprovalRequiredInfo>,
    /// Optional per-file line diff for file-mutating tools (`fs_write`,
    /// `fs_edit`, `fs_delete`). Populated by the kernel boundary from
    /// the underlying [`aura_core_types::ToolResult::line_diff`] so the agent
    /// loop can attach accurate `lines_added` / `lines_removed` counts
    /// to its `FileChange` records without re-reading the filesystem.
    /// `None` for every other tool and for tool failures.
    pub line_diff: Option<aura_core_types::LineDiff>,
    /// Optional rendered image (base64 + media type) produced by
    /// computer-use / vision tools, carried up from the underlying
    /// [`aura_core_types::ToolResult::image`] so the agent loop can
    /// replay the screenshot to the model as an image block. `None` for
    /// every other tool and for tool failures.
    pub image: Option<aura_core_types::ToolResultImage>,
}

/// Receives every record entry as the kernel persists it, in sequence
/// order.
///
/// Called synchronously on the kernel's processing task; implementations
/// must not block (forward to a channel instead).
pub trait RecordObserver: Send + Sync + std::fmt::Debug {
    fn on_entry(&self, entry: &RecordEntry);
}

/// Where the policy decision for a tool call was recorded.
#[derive(Debug, Clone)]
pub struct PolicyDecisionRef {
    /// Sequence number of the `ToolProposal` entry.
    pub proposal_seq: u64,
    /// Hash of the `ToolProposal` transaction.
    pub proposal_tx_hash: Hash,
    /// Sequence number of the `PolicyDecision` entry.
    pub seq: u64,
    /// Hash of the `PolicyDecision` transaction.
    pub tx_hash: Hash,
    /// The recorded decision payload.
    pub decision: PolicyDecision,
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
    /// Tool call is awaiting an out-of-band operator approval.
    NeedsApproval {
        /// Human-readable reason, e.g.
        /// `"Tool 'run_command' is set to ask"`.
        reason: String,
        /// Structured live prompt metadata for tri-state `ask` prompts.
        prompt: Option<PendingToolPrompt>,
    },
}

/// Result of processing a transaction.
#[derive(Debug)]
pub struct ProcessResult {
    /// The record entry created. For tool proposals this is the last
    /// entry written for the call: the `ToolExecution` entry when the call
    /// ran, otherwise the `PolicyDecision` entry.
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
    /// Recorded `PolicyDecision` entry for tool proposals.
    pub policy_decision: Option<PolicyDecisionRef>,
}

/// Result of a reasoning call.
#[derive(Debug)]
pub struct ReasonResult {
    /// The record entry created
    pub entry: RecordEntry,
    /// The model response
    pub response: aura_model::ModelResponse,
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
    /// The sequence number after the last one claimed, under the lock each
    /// record path holds from claiming its number through its append
    /// ([`seq::claim`])
    pub(super) seq: Arc<Mutex<u64>>,
    /// Context hashes of the agent's newest entries, read in place of
    /// the store's while they reach its head ([`window`])
    pub(super) recent: Arc<Mutex<window::RecentHashes>>,
    /// Phase 6b replay outcome — `Some(report)` after a successful
    /// replay sweep at construction time when
    /// [`KernelConfig::replay_from`] is `Some`; `None` for live-mode
    /// kernels. Exposed read-only via [`Kernel::replay_report`].
    pub(super) replay_report: Option<ReplayReport>,
}

impl Kernel {
    /// Create a new kernel bound to a specific agent.
    ///
    /// Reads the current head sequence from the store so the internal counter
    /// starts at `head_seq + 1`. When [`KernelConfig::replay_from`] is
    /// `Some(from_seq)` the constructor additionally runs a
    /// [`ReplayConsumer`] over the existing record log for
    /// `agent_id`, starting at `from_seq` (inclusive), and stashes
    /// the resulting [`ReplayReport`]. The kernel will not begin
    /// accepting new transactions until that replay completes — see
    /// the [`KernelConfig::replay_from`] docs.
    ///
    /// # Errors
    ///
    /// - [`crate::KernelError::Store`] if the store cannot be read.
    /// - [`crate::KernelError::Replay`] if replay is configured and
    ///   the replay consumer surfaces a [`ReplayError`] (context
    ///   divergence, missing snapshot, store failure, etc.).
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

        // Phase 6b: run replay BEFORE the kernel becomes available for
        // any process_*() call. The constructor surfaces a typed
        // ReplayError so callers can distinguish replay failures from
        // generic kernel/store errors without string-matching.
        let replay_report = if let Some(from_seq) = config.replay_from {
            let consumer = ReplayConsumer::new(
                store.clone(),
                config.snapshot_store.clone(),
                agent_id,
                from_seq,
                config.record_window_size,
            );
            Some(consumer.run()?)
        } else {
            None
        };

        let recent = window::RecentHashes::new(config.record_window_size);
        Ok(Self {
            store,
            provider,
            executor,
            policy,
            config,
            agent_id,
            seq: Arc::new(Mutex::new(head_seq + 1)),
            recent: Arc::new(Mutex::new(recent)),
            replay_report,
        })
    }

    /// Returns the replay report produced at construction time when
    /// [`KernelConfig::replay_from`] was set.
    ///
    /// `None` for live-mode kernels (no replay configured). The
    /// returned reference is read-only; the kernel does not re-run
    /// replay after the constructor.
    #[must_use]
    pub fn replay_report(&self) -> Option<&ReplayReport> {
        self.replay_report.as_ref()
    }

    /// Get a reference to the underlying store.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// Read-only accessor for the kernel's `Policy`.
    ///
    /// Exposed for policy-focused tests and diagnostics. Pure observational
    /// surface — the policy's interior mutable state is still protected by
    /// its own synchronization.
    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Whether a live tool-approval prompter is attached, so `ask` tools
    /// can be put to someone instead of being denied.
    #[must_use]
    pub fn has_approval_prompter(&self) -> bool {
        self.config.tool_approval_prompter.is_some()
    }

    /// Number of plugin hooks registered for
    /// [`aura_plugin_hooks::HookEvent::PermissionRequest`]; `0` when no
    /// hook host is attached.
    #[must_use]
    pub fn permission_hook_count(&self) -> usize {
        self.config.plugin_hooks.as_ref().map_or(0, |host| {
            host.registered_count(aura_plugin_hooks::HookEvent::PermissionRequest)
        })
    }

    // -----------------------------------------------------------------------
    // Sequence helpers
    // -----------------------------------------------------------------------

    /// Claims the agent's next sequence number ([`seq::claim`]).
    fn claim_seq(&self) -> Result<seq::Claim<'_>, crate::KernelError> {
        seq::claim(&*self.store, self.agent_id, &self.seq)
    }

    /// Keep the context hash of `entry`, just appended at `claim`'s
    /// number, for the entries after it, then let the claim go and pass
    /// the entry to the record observer.
    fn notify_entry(&self, claim: seq::Claim<'_>, entry: &RecordEntry) {
        window::remember(&self.recent, entry);
        drop(claim);
        if let Some(observer) = self.config.record_observer.as_ref() {
            observer.on_entry(entry);
        }
    }

    /// Directory the agent's tools run in.
    #[must_use]
    pub fn agent_workspace(&self) -> PathBuf {
        if self.config.use_workspace_base_as_root {
            self.config.workspace_base.clone()
        } else {
            self.config.workspace_base.join(self.agent_id.to_hex())
        }
    }

    // -----------------------------------------------------------------------
    // Context helpers
    // -----------------------------------------------------------------------

    /// The context hashes the entry at `next_seq` mixes in: those of the
    /// last `record_window_size` entries before it.
    pub(super) fn window_hashes(
        &self,
        next_seq: u64,
    ) -> Result<Vec<ContextHash>, crate::KernelError> {
        window::window_hashes(
            &*self.store,
            self.agent_id,
            &self.recent,
            next_seq,
            self.config.record_window_size,
        )
    }

    pub(super) fn load_runtime_capabilities(
        &self,
    ) -> Result<Option<RuntimeCapabilityInstall>, crate::KernelError> {
        self.store
            .get_runtime_capabilities(self.agent_id)
            .map_err(|e| crate::KernelError::Store(format!("get_runtime_capabilities: {e}")))
    }

    /// The same kernel, scoping the prompt cache of every model request it
    /// sends to `salt` (see [`KernelConfig::cache_salt`]).
    #[must_use]
    pub fn with_cache_salt(mut self, salt: impl Into<String>) -> Self {
        self.config.cache_salt = Some(salt.into());
        self
    }

    /// The prompt-cache scope of this kernel's model requests, if any.
    #[must_use]
    pub fn cache_salt(&self) -> Option<&str> {
        self.config.cache_salt.as_deref()
    }

    /// The same kernel, running its commands with `scratch` (see
    /// [`KernelConfig::run_scratch`]).
    #[must_use]
    pub fn with_run_scratch(mut self, scratch: RunScratch) -> Self {
        self.config.run_scratch = Some(scratch);
        self
    }

    /// The scratch folder of this kernel's commands, if any.
    #[must_use]
    pub fn run_scratch(&self) -> Option<&RunScratch> {
        self.config.run_scratch.as_ref()
    }

    /// Scope `request`'s prompt cache to the kernel's salt, when it has one.
    pub(super) fn scope_request(&self, request: &mut ModelRequest) {
        if let Some(salt) = self.config.cache_salt.as_deref() {
            request.scope_cache(salt);
        }
    }

    /// Returns the kernel's configured [`KernelMode`].
    #[must_use]
    pub fn kernel_mode(&self) -> KernelMode {
        self.config.kernel_mode
    }

    /// Returns `Some(threshold)` when [`KernelConfig::kernel_mode`] is
    /// [`KernelMode::AuditedLite`] (effect payloads above `threshold`
    /// bytes are summarised), or `None` for [`KernelMode::Audited`]
    /// (no summarisation; payloads stored verbatim).
    #[must_use]
    pub fn lite_payload_threshold(&self) -> Option<usize> {
        match self.config.kernel_mode {
            KernelMode::Audited => None,
            KernelMode::AuditedLite => Some(self.config.audited_lite_threshold_bytes),
        }
    }

    /// Encode `bytes` into a [`aura_store_record::RecordPayload`]
    /// honouring [`KernelConfig::kernel_mode`].
    ///
    /// - `KernelMode::Audited` always produces
    ///   [`aura_store_record::RecordPayload::Inline`] (full bytes).
    /// - `KernelMode::AuditedLite` produces
    ///   [`aura_store_record::RecordPayload::Summary`] when
    ///   `bytes.len() > audited_lite_threshold_bytes`, else
    ///   [`aura_store_record::RecordPayload::Inline`] (below
    ///   threshold). Sequence numbers + RecordKind are unaffected.
    #[must_use]
    pub fn encode_payload(&self, bytes: &[u8]) -> aura_store_record::RecordPayload {
        match self.config.kernel_mode {
            KernelMode::Audited => aura_store_record::RecordPayload::inline(bytes.to_vec()),
            KernelMode::AuditedLite => aura_store_record::summarize_payload(
                bytes,
                self.config.audited_lite_threshold_bytes,
            ),
        }
    }
}
