//! Kernel configuration.

use super::{RecordObserver, ToolApprovalPrompter};
use crate::policy::PolicyConfig;
use aura_core_modes::KernelMode;
use aura_exec_traits::RunScratch;
use aura_plugin_hooks::PluginHookHost;
use aura_store_record::DEFAULT_SUMMARY_CHUNK_BYTES;
use aura_store_snapshot::{NoopSnapshotStore, SnapshotStore};
use std::path::PathBuf;
use std::sync::Arc;

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
    /// Phase 6b replay anchor.
    ///
    /// - `None` (default) — live mode: the kernel calls the live
    ///   reasoner and executor as usual.
    /// - `Some(from_seq)` — replay mode: at construction the kernel
    ///   runs a [`crate::ReplayConsumer`] over its store's record log
    ///   for `agent_id`, starting at `from_seq` (inclusive), to
    ///   replay every historical [`aura_core_types::RecordEntry`] in
    ///   forward `seq` order. The replay validates each entry's
    ///   `context_hash` against a fresh recomputation through
    ///   [`crate::hash_tx_with_window`] and, for AuditedLite
    ///   entries whose effect payloads were summarised, validates
    ///   the `full_hash` against [`KernelConfig::snapshot_store`].
    ///   On success the [`crate::ReplayReport`] is stashed on the
    ///   kernel and exposed via [`Kernel::replay_report`]. On
    ///   failure the constructor returns
    ///   [`crate::KernelError::Replay`] and no further processing
    ///   is permitted from that instance.
    ///
    /// Wiring this field replaces the Phase 6a `replay_mode: bool`
    /// placeholder that had no consumer.
    pub replay_from: Option<u64>,
    /// Content-addressed store for AuditedLite payloads: the kernel
    /// writes each summarised effect's full bytes here, and replay
    /// reads them back. Defaults to
    /// [`aura_store_snapshot::NoopSnapshotStore`] (keeps nothing), so
    /// replay of an AuditedLite turn surfaces
    /// [`crate::ReplayError::SnapshotMissing`] unless a real backend
    /// such as [`aura_store_snapshot::FsSnapshotStore`] is wired here.
    pub snapshot_store: Arc<dyn SnapshotStore>,
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
    /// Phase 6a kernel audit tier (`Audited` vs `AuditedLite`).
    ///
    /// Imported from [`aura_core_modes::KernelMode`]; the kernel does
    /// not own this enum. `Audited` (default) records full inline
    /// payloads; `AuditedLite` summarises payloads above
    /// [`KernelConfig::audited_lite_threshold_bytes`] into a
    /// `RecordPayload::Summary { head, tail, full_hash, full_len }`.
    /// Sequence numbers and `RecordKind` values are identical across
    /// the two modes — only the payload representation differs.
    pub kernel_mode: KernelMode,
    /// Payload-size threshold (bytes) above which `AuditedLite`
    /// switches from full inline to head/tail summary. Defaults to
    /// [`aura_store_record::DEFAULT_SUMMARY_CHUNK_BYTES`] (1 KiB).
    /// Ignored when `kernel_mode == KernelMode::Audited`.
    pub audited_lite_threshold_bytes: usize,
    /// Phase 10 carve-out 5b: optional [`PluginHookHost`] consulted
    /// by [`Kernel::resolve_prompt_verdict`] before the interactive
    /// `ToolApprovalPrompter` is invoked. A registered
    /// `PermissionRequest` handler that returns
    /// [`aura_plugin_hooks::HookOutcome::Approve`] short-circuits
    /// the prompt with `PolicyVerdict::Allow`, and `Deny` with
    /// `PolicyVerdict::Deny { reason }`. Any other outcome
    /// (`Continue` / `TimedOut`) falls through to the interactive
    /// prompt.
    ///
    /// `None` (default) preserves Phase 8 behaviour exactly — the
    /// kernel never fires `PermissionRequest` hooks and the
    /// interactive prompt is always reached when configured.
    pub plugin_hooks: Option<Arc<PluginHookHost>>,
    /// Notified with every record entry this kernel persists (tool
    /// proposals, policy decisions, executions, reasoning, prompts,
    /// replies, system entries) immediately after it is appended, so UIs
    /// can show the policy decision before the tool runs and the record
    /// feed stays gapless.
    pub record_observer: Option<Arc<dyn RecordObserver>>,
    /// Prompt-cache scope of every model request this kernel sends (see
    /// [`aura_model::ModelRequest::scope_cache`]). Requests from kernels
    /// with different salts never share provider prompt-cache entries;
    /// `None` (default) leaves requests unscoped.
    pub cache_salt: Option<String>,
    /// The run's private folder for its commands' temp files, caches and
    /// builds (see [`aura_exec_traits::RunScratch`]). Kernels with
    /// different scratch folders never share those through the command
    /// environment; `None` (default) leaves commands on the shared ones.
    pub run_scratch: Option<RunScratch>,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            record_window_size: 50,
            policy: PolicyConfig::default(),
            workspace_base: PathBuf::from("./workspaces"),
            use_workspace_base_as_root: false,
            replay_from: None,
            snapshot_store: Arc::new(NoopSnapshotStore),
            proposal_timeout_ms: 120_000,
            tool_timeout_ms: 120_000,
            tool_approval_prompter: None,
            originating_user_id: None,
            kernel_mode: KernelMode::Audited,
            audited_lite_threshold_bytes: DEFAULT_SUMMARY_CHUNK_BYTES,
            plugin_hooks: None,
            record_observer: None,
            cache_salt: None,
            run_scratch: None,
        }
    }
}
