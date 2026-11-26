//! Kernel records, for the Record panel.

use serde::{Deserialize, Serialize};

/// Most records the Record panel holds, and the most history loaded into
/// it, newest kept. A memory guard rather than a display limit: a
/// [`RecordSummary`] can carry up to ~20KB of pretty-printed payload in
/// `message`, so this bounds the panel to a few hundred MB at worst.
pub const MAX_RECORDS: usize = 10_000;

/// Summary of a kernel record for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSummary {
    /// Sequence number
    pub seq: u64,
    /// Formatted timestamp (HH:MM:SS)
    pub timestamp: String,
    /// Full context hash (hex encoded)
    pub full_hash: String,
    /// Last 4 characters of the context hash
    pub hash_suffix: String,
    /// Transaction kind (e.g., `UserPrompt`, `ActionResult`)
    pub tx_kind: String,
    /// Sender/user who initiated the transaction
    pub sender: String,
    /// Message content (truncated for display)
    pub message: String,
    /// Number of actions in this record
    pub action_count: usize,
    /// Effect status summary (e.g., "2 ok", "1 ok, 1 failed")
    pub effect_status: String,
    /// Status indicator (ok, err, pending, or -)
    pub status: RecordStatus,
    /// Additional info (e.g., tool name)
    pub info: String,
    /// Record panel column: what the record is about (tool name, LLM stop
    /// reason, sender). Empty means the panel shows `info` instead.
    #[serde(default)]
    pub what: String,
    /// Record panel column: category of `what` (tool group, LLM error
    /// class or model family).
    #[serde(default)]
    pub group: String,
    /// Record panel text after the columns, without what they already show.
    #[serde(default)]
    pub detail: String,
    /// Error details if status is Error
    #[serde(default)]
    pub error_details: String,

    // Transaction fields
    /// Transaction ID (hex encoded)
    #[serde(default)]
    pub tx_id: String,
    /// Agent ID (hex encoded)
    #[serde(default)]
    pub agent_id: String,
    /// Raw timestamp in milliseconds since epoch
    #[serde(default)]
    pub ts_ms: u64,
    /// Policy provenance, set for policy decision entries.
    #[serde(default)]
    pub policy: Option<PolicyDetail>,
}

/// Why the kernel allowed, denied, or paused a tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDetail {
    /// Tool the decision applies to
    pub tool: String,
    /// `approved`, `denied`, or `pending_approval`
    pub verdict: String,
    /// The deciding rule, rendered as `source.rule`
    pub decided_by: String,
    /// Denial / pending reason, empty when approved
    pub reason: String,
    /// Evaluated gates in order, each rendered as `outcome source.rule`
    pub trace: Vec<String>,
    /// Live approver (`hook` or `user`, with remember scope), empty if none
    pub approval: String,
    /// Agent mode at decision time
    pub mode: String,
    /// Hash of the policy configuration that was evaluated
    pub policy_hash: String,
    /// Hash of the `ToolProposal` transaction this decision answers
    pub proposal_tx: String,
}

/// Status of a record entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    /// All effects completed successfully
    Ok,
    /// One or more effects failed
    Error,
    /// Effects are pending
    Pending,
    /// No effects (neutral status)
    None,
}
