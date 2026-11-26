use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

use crate::common::ToolApprovalRemember;

/// Payload for `tool_approval_prompt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolApprovalPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub agent_id: String,
    pub remember_options: Vec<ToolApprovalRemember>,
}

/// A policy rule: the layer (`source`) plus the rule key within it,
/// e.g. `user_default` + `per_tool[run_command]=on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct PolicyRuleWire {
    pub source: String,
    pub rule: String,
}

/// One evaluated policy gate, in evaluation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct PolicyCheckWire {
    pub source: String,
    pub rule: String,
    /// `allow`, `deny`, or `ask`.
    pub outcome: String,
}

/// Payload for `tool_policy_decision`: the kernel's recorded policy
/// decision for a tool call. Use `seq` / `tx_hash` (and
/// `proposal_tx_hash`) to fetch the record entries via the
/// `/agents/:agent_id/tool_calls/:tool_use_id/audit` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ToolPolicyDecision {
    pub tool_use_id: String,
    pub tool_name: String,
    /// `approved`, `denied`, or `pending_approval`.
    pub verdict: String,
    #[serde(default)]
    pub reason: Option<String>,
    /// The rule that produced the verdict.
    pub decided_by: PolicyRuleWire,
    /// Every gate evaluated, in order.
    pub trace: Vec<PolicyCheckWire>,
    /// `hook` or `user` when a live approval contributed.
    #[serde(default)]
    pub approved_by: Option<String>,
    /// Remember scope chosen by the approver (`once`, `session`, `forever`).
    #[serde(default)]
    pub remember: Option<String>,
    pub mode: String,
    pub policy_hash: String,
    pub agent_id: String,
    pub seq: u64,
    pub tx_hash: String,
    pub proposal_tx_hash: String,
}
