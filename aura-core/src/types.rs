//! Domain types for the Aura system.
//!
//! Includes Transaction, Action, Effect, `RecordEntry`, and related types.

use crate::ids::{ActionId, AgentId, Hash, ProcessId, TxId};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Transaction Types
// ============================================================================

/// The type of transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionType {
    /// User-initiated prompt/message
    UserPrompt,
    /// Message from another agent
    AgentMsg,
    /// Scheduled or event-based trigger
    Trigger,
    /// Result from a previously executed action
    ActionResult,
    /// System-generated transaction
    System,
    /// Session/context reset marker
    SessionStart,
    /// Tool proposal from the reasoner (LLM suggestion, before policy check)
    ToolProposal,
    /// Tool execution result (after kernel policy decision)
    ToolExecution,
    /// Async process completion (callback from background process)
    ProcessComplete,
}

/// Legacy alias for backwards compatibility.
#[deprecated(since = "0.2.0", note = "Use TransactionType instead")]
pub type TransactionKind = TransactionType;

/// An immutable transaction input to the system.
///
/// Transactions are the only way state can change in Aura.
/// The `hash` is derived from content + previous tx hash, creating an immutable chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// Unique hash derived from content + previous tx hash (blockchain-style chain)
    /// Uses default (zeroed) hash for backwards compatibility with old records.
    #[serde(default, with = "hex_hash")]
    pub hash: Hash,
    /// Target agent
    pub agent_id: AgentId,
    /// Timestamp in milliseconds since epoch
    pub ts_ms: u64,
    /// Type of transaction
    pub tx_type: TransactionType,
    /// Versioned payload (opaque bytes)
    #[serde(with = "bytes_serde")]
    pub payload: Bytes,
    /// Optional reference to a related transaction (for callbacks from async processes)
    #[serde(default, skip_serializing_if = "Option::is_none", with = "option_hex_hash")]
    pub reference_tx_hash: Option<Hash>,
}

impl Transaction {
    /// Get current timestamp in milliseconds.
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Create a new transaction with explicit hash (for replay/import).
    #[must_use]
    pub fn new(
        hash: Hash,
        agent_id: AgentId,
        ts_ms: u64,
        tx_type: TransactionType,
        payload: impl Into<Bytes>,
    ) -> Self {
        Self {
            hash,
            agent_id,
            ts_ms,
            tx_type,
            payload: payload.into(),
            reference_tx_hash: None,
        }
    }

    /// Create a new transaction chained to a previous transaction.
    #[must_use]
    pub fn new_chained(
        agent_id: AgentId,
        tx_type: TransactionType,
        payload: impl Into<Bytes>,
        prev_hash: Option<&Hash>,
    ) -> Self {
        let payload = payload.into();
        let hash = Hash::from_content_chained(&payload, prev_hash);
        let ts_ms = Self::now_ms();

        Self {
            hash,
            agent_id,
            ts_ms,
            tx_type,
            payload,
            reference_tx_hash: None,
        }
    }

    /// Create a user prompt transaction.
    #[must_use]
    pub fn user_prompt(agent_id: AgentId, payload: impl Into<Bytes>) -> Self {
        Self::new_chained(agent_id, TransactionType::UserPrompt, payload, None)
    }

    /// Create a user prompt transaction chained to a previous transaction.
    #[must_use]
    pub fn user_prompt_chained(agent_id: AgentId, payload: impl Into<Bytes>, prev_hash: &Hash) -> Self {
        Self::new_chained(agent_id, TransactionType::UserPrompt, payload, Some(prev_hash))
    }

    /// Create an action result transaction.
    #[must_use]
    pub fn action_result(agent_id: AgentId, payload: impl Into<Bytes>) -> Self {
        Self::new_chained(agent_id, TransactionType::ActionResult, payload, None)
    }

    /// Create an action result with a reference to the originating transaction.
    ///
    /// Used for async process completions that need to reference their origin.
    #[must_use]
    pub fn action_result_with_reference(
        agent_id: AgentId,
        payload: impl Into<Bytes>,
        reference_tx_hash: Hash,
        prev_hash: Option<&Hash>,
    ) -> Self {
        let payload = payload.into();
        let hash = Hash::from_content_chained(&payload, prev_hash);
        let ts_ms = Self::now_ms();

        Self {
            hash,
            agent_id,
            ts_ms,
            tx_type: TransactionType::ActionResult,
            payload,
            reference_tx_hash: Some(reference_tx_hash),
        }
    }

    /// Create a session start transaction (context reset marker).
    #[must_use]
    pub fn session_start(agent_id: AgentId) -> Self {
        Self::new_chained(agent_id, TransactionType::SessionStart, Bytes::from_static(b"session_start"), None)
    }

    /// Create a tool proposal transaction.
    ///
    /// Records a tool call suggested by the reasoner (LLM) before policy evaluation.
    /// The payload contains the proposed tool call details.
    #[must_use]
    pub fn tool_proposal(agent_id: AgentId, proposal: &ToolProposal) -> Self {
        let payload = serde_json::to_vec(proposal).unwrap_or_default();
        Self::new_chained(agent_id, TransactionType::ToolProposal, payload, None)
    }

    /// Create a tool execution transaction.
    ///
    /// Records the kernel's decision and execution result for a tool proposal.
    /// This captures what actually happened after policy evaluation.
    #[must_use]
    pub fn tool_execution(agent_id: AgentId, execution: &ToolExecution) -> Self {
        let payload = serde_json::to_vec(execution).unwrap_or_default();
        Self::new_chained(agent_id, TransactionType::ToolExecution, payload, None)
    }

    /// Create a process completion transaction.
    ///
    /// Records the result of an async process that completed after the initial
    /// transaction was recorded. Links back to the originating transaction.
    #[must_use]
    pub fn process_complete(
        agent_id: AgentId,
        payload: &ActionResultPayload,
        reference_tx_hash: Hash,
        prev_hash: Option<&Hash>,
    ) -> Self {
        let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
        let hash = Hash::from_content_chained(&payload_bytes, prev_hash);
        let ts_ms = Self::now_ms();

        Self {
            hash,
            agent_id,
            ts_ms,
            tx_type: TransactionType::ProcessComplete,
            payload: payload_bytes.into(),
            reference_tx_hash: Some(reference_tx_hash),
        }
    }

    /// Get the transaction hash (legacy compatibility with tx_id).
    #[must_use]
    pub fn tx_id(&self) -> TxId {
        TxId::new(*self.hash.as_bytes())
    }
}

// ============================================================================
// Tool Proposal and Execution Payloads
// ============================================================================

/// A tool proposal from the reasoner (LLM).
///
/// This records what the LLM suggested before any policy check.
/// The kernel will decide whether to approve or deny this proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProposal {
    /// Tool use ID from the model
    pub tool_use_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Source of the proposal (e.g., model name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ToolProposal {
    /// Create a new tool proposal.
    #[must_use]
    pub fn new(tool_use_id: impl Into<String>, tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            tool: tool.into(),
            args,
            source: None,
        }
    }

    /// Set the source of the proposal.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// The kernel's decision on a tool proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    /// Approved and executed
    Approved,
    /// Denied by policy
    Denied,
    /// Requires user approval (pending)
    PendingApproval,
}

/// Tool execution result from the kernel.
///
/// This records what actually happened after policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecution {
    /// Reference to the original proposal's `tool_use_id`
    pub tool_use_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments (copied from proposal for auditability)
    pub args: serde_json::Value,
    /// Kernel's decision
    pub decision: ToolDecision,
    /// Reason for the decision (especially for denials)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Execution result (if approved and executed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Whether the execution failed (only relevant if approved)
    #[serde(default)]
    pub is_error: bool,
}

// ============================================================================
// Async Process Types
// ============================================================================

/// Payload for a pending process effect.
///
/// This is stored in the Effect payload when a command exceeds the sync threshold
/// and is moved to async execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPending {
    /// Unique process identifier for tracking
    pub process_id: ProcessId,
    /// The command being executed
    pub command: String,
    /// When the process started (milliseconds since epoch)
    pub started_at_ms: u64,
}

impl ProcessPending {
    /// Create a new pending process payload.
    #[must_use]
    pub fn new(process_id: ProcessId, command: impl Into<String>) -> Self {
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        Self {
            process_id,
            command: command.into(),
            started_at_ms,
        }
    }
}

/// Payload for ActionResult transactions from completed async processes.
///
/// This is used when an async process completes and needs to be recorded
/// as a continuation of the original transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResultPayload {
    /// The action_id this result continues
    pub action_id: ActionId,
    /// Process identifier for correlation
    pub process_id: ProcessId,
    /// Exit code from the process
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Standard output from the process
    #[serde(default, with = "bytes_serde")]
    pub stdout: Bytes,
    /// Standard error from the process
    #[serde(default, with = "bytes_serde")]
    pub stderr: Bytes,
    /// Whether the process succeeded
    pub success: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl ActionResultPayload {
    /// Create a successful result payload.
    #[must_use]
    pub fn success(
        action_id: ActionId,
        process_id: ProcessId,
        exit_code: Option<i32>,
        stdout: impl Into<Bytes>,
        duration_ms: u64,
    ) -> Self {
        Self {
            action_id,
            process_id,
            exit_code,
            stdout: stdout.into(),
            stderr: Bytes::new(),
            success: true,
            duration_ms,
        }
    }

    /// Create a failed result payload.
    #[must_use]
    pub fn failure(
        action_id: ActionId,
        process_id: ProcessId,
        exit_code: Option<i32>,
        stderr: impl Into<Bytes>,
        duration_ms: u64,
    ) -> Self {
        Self {
            action_id,
            process_id,
            exit_code,
            stdout: Bytes::new(),
            stderr: stderr.into(),
            success: false,
            duration_ms,
        }
    }
}

// ============================================================================
// Action Types
// ============================================================================

/// The kind of action (whitepaper-aligned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Reasoning/thinking action
    Reason,
    /// Store information for future use
    Memorize,
    /// Make a decision
    Decide,
    /// Delegate to external system (tools, other agents)
    Delegate,
}

/// An authorized action to be executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// Unique action identifier
    pub action_id: ActionId,
    /// Type of action
    pub kind: ActionKind,
    /// Versioned payload (opaque bytes)
    #[serde(with = "bytes_serde")]
    pub payload: Bytes,
}

impl Action {
    /// Create a new action.
    #[must_use]
    pub fn new(action_id: ActionId, kind: ActionKind, payload: impl Into<Bytes>) -> Self {
        Self {
            action_id,
            kind,
            payload: payload.into(),
        }
    }

    /// Create a delegate action for a tool call.
    #[must_use]
    pub fn delegate_tool(tool_call: &ToolCall) -> Self {
        let payload = serde_json::to_vec(tool_call).unwrap_or_default();
        Self::new(ActionId::generate(), ActionKind::Delegate, payload)
    }
}

// ============================================================================
// Effect Types
// ============================================================================

/// The kind of effect produced by an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// A proposal was generated
    Proposal,
    /// An artifact was created/stored
    Artifact,
    /// A belief was updated
    Belief,
    /// An agreement was reached (tool result, etc.)
    Agreement,
}

/// The status of an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    /// Successfully committed
    Committed,
    /// Pending external completion
    Pending,
    /// Failed
    Failed,
}

/// The result of executing an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// Reference to the action that produced this effect
    pub action_id: ActionId,
    /// Type of effect
    pub kind: EffectKind,
    /// Status of the effect
    pub status: EffectStatus,
    /// Result payload (opaque bytes)
    #[serde(with = "bytes_serde")]
    pub payload: Bytes,
}

impl Effect {
    /// Create a new effect.
    #[must_use]
    pub fn new(
        action_id: ActionId,
        kind: EffectKind,
        status: EffectStatus,
        payload: impl Into<Bytes>,
    ) -> Self {
        Self {
            action_id,
            kind,
            status,
            payload: payload.into(),
        }
    }

    /// Create a committed agreement effect (e.g., tool result).
    #[must_use]
    pub fn committed_agreement(action_id: ActionId, payload: impl Into<Bytes>) -> Self {
        Self::new(
            action_id,
            EffectKind::Agreement,
            EffectStatus::Committed,
            payload,
        )
    }

    /// Create a failed effect.
    #[must_use]
    pub fn failed(action_id: ActionId, kind: EffectKind, error: impl Into<Bytes>) -> Self {
        Self::new(action_id, kind, EffectStatus::Failed, error)
    }

    /// Create a pending effect.
    #[must_use]
    pub fn pending(action_id: ActionId, kind: EffectKind) -> Self {
        Self::new(action_id, kind, EffectStatus::Pending, Bytes::new())
    }
}

// ============================================================================
// Proposal Types (from Reasoner)
// ============================================================================

/// A proposal from the reasoner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// Proposed action kind
    pub action_kind: ActionKind,
    /// Payload for the proposed action
    #[serde(with = "bytes_serde")]
    pub payload: Bytes,
    /// Optional reasoning for the proposal
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

impl Proposal {
    /// Create a new proposal.
    #[must_use]
    pub fn new(action_kind: ActionKind, payload: impl Into<Bytes>) -> Self {
        Self {
            action_kind,
            payload: payload.into(),
            rationale: None,
        }
    }

    /// Add a rationale to the proposal.
    #[must_use]
    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = Some(rationale.into());
        self
    }
}

/// Trace information from the reasoner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    /// Model used for reasoning
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Latency in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// A set of proposals from the reasoner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalSet {
    /// List of proposals
    pub proposals: Vec<Proposal>,
    /// Optional trace information
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Trace>,
}

impl ProposalSet {
    /// Create a new empty proposal set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a proposal set with proposals.
    #[must_use]
    pub const fn with_proposals(proposals: Vec<Proposal>) -> Self {
        Self {
            proposals,
            trace: None,
        }
    }

    /// Add trace information.
    #[must_use]
    pub fn with_trace(mut self, trace: Trace) -> Self {
        self.trace = Some(trace);
        self
    }
}

// ============================================================================
// Decision Types
// ============================================================================

/// Information about a rejected proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedProposal {
    /// Index of the rejected proposal
    pub proposal_index: u32,
    /// Reason for rejection
    pub reason: String,
}

/// The decision made by the kernel after evaluating proposals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// IDs of accepted actions
    pub accepted_action_ids: Vec<ActionId>,
    /// Information about rejected proposals
    pub rejected: Vec<RejectedProposal>,
}

impl Decision {
    /// Create a new empty decision.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept an action.
    pub fn accept(&mut self, action_id: ActionId) {
        self.accepted_action_ids.push(action_id);
    }

    /// Reject a proposal.
    pub fn reject(&mut self, proposal_index: u32, reason: impl Into<String>) {
        self.rejected.push(RejectedProposal {
            proposal_index,
            reason: reason.into(),
        });
    }
}

// ============================================================================
// Record Entry
// ============================================================================

/// Current kernel version for record entries.
pub const KERNEL_VERSION: u32 = 1;

/// A single entry in the agent's record (append-only log).
///
/// One `RecordEntry` is created for each processed `Transaction`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordEntry {
    /// Sequence number (strictly ordered per agent)
    pub seq: u64,
    /// The transaction that was processed
    pub tx: Transaction,
    /// Kernel version that processed this entry
    pub kernel_version: u32,
    /// Hash of deterministic inputs used to decide
    #[serde(with = "hex_bytes_32")]
    pub context_hash: [u8; 32],
    /// Proposals from the reasoner (recorded verbatim)
    pub proposals: ProposalSet,
    /// Decision made by the kernel
    pub decision: Decision,
    /// Authorized actions
    pub actions: Vec<Action>,
    /// Effects from executing actions
    pub effects: Vec<Effect>,
}

impl RecordEntry {
    /// Create a new record entry builder.
    #[must_use]
    pub fn builder(seq: u64, tx: Transaction) -> RecordEntryBuilder {
        RecordEntryBuilder::new(seq, tx)
    }
}

/// Builder for `RecordEntry`.
pub struct RecordEntryBuilder {
    seq: u64,
    tx: Transaction,
    context_hash: [u8; 32],
    proposals: ProposalSet,
    decision: Decision,
    actions: Vec<Action>,
    effects: Vec<Effect>,
}

impl RecordEntryBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new(seq: u64, tx: Transaction) -> Self {
        Self {
            seq,
            tx,
            context_hash: [0u8; 32],
            proposals: ProposalSet::new(),
            decision: Decision::new(),
            actions: Vec::new(),
            effects: Vec::new(),
        }
    }

    /// Set the context hash.
    #[must_use]
    pub const fn context_hash(mut self, hash: [u8; 32]) -> Self {
        self.context_hash = hash;
        self
    }

    /// Set the proposals.
    #[must_use]
    pub fn proposals(mut self, proposals: ProposalSet) -> Self {
        self.proposals = proposals;
        self
    }

    /// Set the decision.
    #[must_use]
    pub fn decision(mut self, decision: Decision) -> Self {
        self.decision = decision;
        self
    }

    /// Set the actions.
    #[must_use]
    pub fn actions(mut self, actions: Vec<Action>) -> Self {
        self.actions = actions;
        self
    }

    /// Set the effects.
    #[must_use]
    pub fn effects(mut self, effects: Vec<Effect>) -> Self {
        self.effects = effects;
        self
    }

    /// Build the record entry.
    #[must_use]
    pub fn build(self) -> RecordEntry {
        RecordEntry {
            seq: self.seq,
            tx: self.tx,
            kernel_version: KERNEL_VERSION,
            context_hash: self.context_hash,
            proposals: self.proposals,
            decision: self.decision,
            actions: self.actions,
            effects: self.effects,
        }
    }
}

// ============================================================================
// Identity
// ============================================================================

/// Agent identity information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Agent identifier
    pub agent_id: AgentId,
    /// ZNS identifier (e.g., "0://Agent09")
    pub zns_id: String,
    /// Mutable display name
    pub name: String,
    /// Fingerprint of the identity
    #[serde(with = "hex_bytes_32")]
    pub identity_hash: [u8; 32],
}

impl Identity {
    /// Create a new identity.
    #[must_use]
    pub fn new(zns_id: impl Into<String>, name: impl Into<String>) -> Self {
        let zns_id = zns_id.into();
        let name = name.into();

        // Compute identity hash from zns_id
        let identity_hash = *blake3::hash(zns_id.as_bytes()).as_bytes();

        // Derive agent_id from identity_hash
        let agent_id = AgentId::new(identity_hash);

        Self {
            agent_id,
            zns_id,
            name,
            identity_hash,
        }
    }
}

// ============================================================================
// Tool Types
// ============================================================================

/// A tool call request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name (e.g., `fs_ls`, `fs_read`, `cmd_run`)
    pub tool: String,
    /// Tool arguments (versioned JSON)
    pub args: serde_json::Value,
}

impl ToolCall {
    /// Create a new tool call.
    #[must_use]
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            args,
        }
    }

    /// Create an `fs_ls` tool call.
    #[must_use]
    pub fn fs_ls(path: impl Into<String>) -> Self {
        Self::new("fs_ls", serde_json::json!({ "path": path.into() }))
    }

    /// Create an `fs_read` tool call.
    #[must_use]
    pub fn fs_read(path: impl Into<String>, max_bytes: Option<usize>) -> Self {
        let mut args = serde_json::json!({ "path": path.into() });
        if let Some(max) = max_bytes {
            args["max_bytes"] = serde_json::json!(max);
        }
        Self::new("fs_read", args)
    }

    /// Create an `fs_stat` tool call.
    #[must_use]
    pub fn fs_stat(path: impl Into<String>) -> Self {
        Self::new("fs_stat", serde_json::json!({ "path": path.into() }))
    }
}

/// Result from a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Tool name
    pub tool: String,
    /// Whether the tool succeeded
    pub ok: bool,
    /// Exit code (for commands)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Standard output
    #[serde(default, with = "bytes_serde")]
    pub stdout: Bytes,
    /// Standard error
    #[serde(default, with = "bytes_serde")]
    pub stderr: Bytes,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl ToolResult {
    /// Create a successful tool result.
    #[must_use]
    pub fn success(tool: impl Into<String>, stdout: impl Into<Bytes>) -> Self {
        Self {
            tool: tool.into(),
            ok: true,
            exit_code: None,
            stdout: stdout.into(),
            stderr: Bytes::new(),
            metadata: HashMap::new(),
        }
    }

    /// Create a failed tool result.
    #[must_use]
    pub fn failure(tool: impl Into<String>, stderr: impl Into<Bytes>) -> Self {
        Self {
            tool: tool.into(),
            ok: false,
            exit_code: None,
            stdout: Bytes::new(),
            stderr: stderr.into(),
            metadata: HashMap::new(),
        }
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// ============================================================================
// Serialization Helpers
// ============================================================================

/// Helper module for hex serialization of 32-byte arrays.
mod hex_bytes_32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

/// Helper module for Bytes serialization as base64.
mod bytes_serde {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        use base64::Engine;
        let s = String::deserialize(deserializer)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)?;
        Ok(Bytes::from(decoded))
    }
}

/// Helper module for hex serialization of Hash type.
mod hex_hash {
    use crate::ids::Hash;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &Hash, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hash.to_hex())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Hash, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Hash::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Helper module for optional hex serialization of Hash type.
mod option_hex_hash {
    use crate::ids::Hash;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &Option<Hash>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match hash {
            Some(h) => serializer.serialize_some(&h.to_hex()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Hash>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => Hash::from_hex(&s).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_roundtrip() {
        let tx = Transaction::user_prompt(AgentId::generate(), b"Hello, agent!".to_vec());
        let json = serde_json::to_string(&tx).unwrap();
        let parsed: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(tx, parsed);
    }

    #[test]
    fn transaction_with_reference() {
        let agent_id = AgentId::generate();
        let orig_tx = Transaction::user_prompt(agent_id, b"start process".to_vec());
        let result_payload = ActionResultPayload::success(
            ActionId::generate(),
            ProcessId::generate(),
            Some(0),
            b"output".to_vec(),
            1000,
        );
        let callback_tx = Transaction::process_complete(
            agent_id,
            &result_payload,
            orig_tx.hash,
            Some(&orig_tx.hash),
        );

        assert_eq!(callback_tx.reference_tx_hash, Some(orig_tx.hash));
        assert_eq!(callback_tx.tx_type, TransactionType::ProcessComplete);

        let json = serde_json::to_string(&callback_tx).unwrap();
        let parsed: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(callback_tx, parsed);
    }

    #[test]
    fn transaction_chaining() {
        let agent_id = AgentId::generate();

        // Genesis transaction (no prev)
        let tx1 = Transaction::user_prompt(agent_id, b"first".to_vec());

        // Chained transaction
        let tx2 = Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tx1.hash);

        // Same content with different prev produces different hash
        let tx3 = Transaction::user_prompt(agent_id, b"second".to_vec());
        assert_ne!(tx2.hash, tx3.hash);

        // Deterministic - same inputs produce same hash
        let tx4 = Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tx1.hash);
        assert_eq!(tx2.hash, tx4.hash);
    }

    #[test]
    fn action_roundtrip() {
        let action = Action::new(
            ActionId::generate(),
            ActionKind::Delegate,
            b"tool payload".to_vec(),
        );
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(action, parsed);
    }

    #[test]
    fn effect_roundtrip() {
        let effect = Effect::committed_agreement(ActionId::generate(), b"result".to_vec());
        let json = serde_json::to_string(&effect).unwrap();
        let parsed: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(effect, parsed);
    }

    #[test]
    fn record_entry_roundtrip() {
        let tx = Transaction::user_prompt(AgentId::generate(), b"test".to_vec());
        let entry = RecordEntry::builder(1, tx)
            .context_hash([1u8; 32])
            .proposals(ProposalSet::new())
            .decision(Decision::new())
            .actions(vec![])
            .effects(vec![])
            .build();

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RecordEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
    }

    #[test]
    fn identity_creation() {
        let identity = Identity::new("0://TestAgent", "Test Agent");
        assert!(!identity.zns_id.is_empty());
        assert_eq!(identity.name, "Test Agent");
    }

    #[test]
    fn tool_call_roundtrip() {
        let tool_call = ToolCall::fs_read("src/main.rs", Some(1024));
        let json = serde_json::to_string(&tool_call).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(tool_call, parsed);
    }

    #[test]
    fn tool_result_roundtrip() {
        let result =
            ToolResult::success("fs_read", b"file contents".to_vec()).with_metadata("size", "13");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, parsed);
    }

    #[test]
    fn process_pending_roundtrip() {
        let pending = ProcessPending::new(ProcessId::generate(), "cargo build --release");
        let json = serde_json::to_string(&pending).unwrap();
        let parsed: ProcessPending = serde_json::from_str(&json).unwrap();
        assert_eq!(pending, parsed);
    }

    #[test]
    fn action_result_payload_success_roundtrip() {
        let payload = ActionResultPayload::success(
            ActionId::generate(),
            ProcessId::generate(),
            Some(0),
            b"build succeeded".to_vec(),
            5000,
        );
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: ActionResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, parsed);
        assert!(payload.success);
    }

    #[test]
    fn action_result_payload_failure_roundtrip() {
        let payload = ActionResultPayload::failure(
            ActionId::generate(),
            ProcessId::generate(),
            Some(1),
            b"build failed".to_vec(),
            3000,
        );
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: ActionResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, parsed);
        assert!(!payload.success);
    }

    #[test]
    fn transaction_type_serialization() {
        // Verify all transaction types serialize correctly
        let types = vec![
            TransactionType::UserPrompt,
            TransactionType::AgentMsg,
            TransactionType::Trigger,
            TransactionType::ActionResult,
            TransactionType::System,
            TransactionType::SessionStart,
            TransactionType::ToolProposal,
            TransactionType::ToolExecution,
            TransactionType::ProcessComplete,
        ];

        for tx_type in types {
            let json = serde_json::to_string(&tx_type).unwrap();
            let parsed: TransactionType = serde_json::from_str(&json).unwrap();
            assert_eq!(tx_type, parsed);
        }
    }
}
