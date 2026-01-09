//! Domain types for the Aura system.
//!
//! Includes Transaction, Action, Effect, `RecordEntry`, and related types.

use crate::ids::{ActionId, AgentId, TxId};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Transaction Types
// ============================================================================

/// The kind of transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
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
}

/// An immutable transaction input to the system.
///
/// Transactions are the only way state can change in Aura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// Unique transaction identifier
    pub tx_id: TxId,
    /// Target agent
    pub agent_id: AgentId,
    /// Timestamp in milliseconds since epoch
    pub ts_ms: u64,
    /// Type of transaction
    pub kind: TransactionKind,
    /// Versioned payload (opaque bytes)
    #[serde(with = "bytes_serde")]
    pub payload: Bytes,
}

impl Transaction {
    /// Create a new transaction.
    #[must_use]
    pub fn new(
        tx_id: TxId,
        agent_id: AgentId,
        ts_ms: u64,
        kind: TransactionKind,
        payload: impl Into<Bytes>,
    ) -> Self {
        Self {
            tx_id,
            agent_id,
            ts_ms,
            kind,
            payload: payload.into(),
        }
    }

    /// Create a user prompt transaction.
    #[must_use]
    pub fn user_prompt(agent_id: AgentId, payload: impl Into<Bytes>) -> Self {
        let payload = payload.into();
        let tx_id = TxId::from_content(&payload);
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        Self::new(tx_id, agent_id, ts_ms, TransactionKind::UserPrompt, payload)
    }

    /// Create an action result transaction.
    #[must_use]
    pub fn action_result(agent_id: AgentId, payload: impl Into<Bytes>) -> Self {
        let payload = payload.into();
        let tx_id = TxId::from_content(&payload);
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        Self::new(
            tx_id,
            agent_id,
            ts_ms,
            TransactionKind::ActionResult,
            payload,
        )
    }

    /// Create a session start transaction (context reset marker).
    #[must_use]
    pub fn session_start(agent_id: AgentId) -> Self {
        let payload = Bytes::from_static(b"session_start");
        let tx_id = TxId::from_content(&payload);
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        Self::new(
            tx_id,
            agent_id,
            ts_ms,
            TransactionKind::SessionStart,
            payload,
        )
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
    /// Tool name (e.g., "fs_ls", "fs_read", "cmd_run")
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

    /// Create an fs_ls tool call.
    #[must_use]
    pub fn fs_ls(path: impl Into<String>) -> Self {
        Self::new("fs_ls", serde_json::json!({ "path": path.into() }))
    }

    /// Create an fs_read tool call.
    #[must_use]
    pub fn fs_read(path: impl Into<String>, max_bytes: Option<usize>) -> Self {
        let mut args = serde_json::json!({ "path": path.into() });
        if let Some(max) = max_bytes {
            args["max_bytes"] = serde_json::json!(max);
        }
        Self::new("fs_read", args)
    }

    /// Create an fs_stat tool call.
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
}
