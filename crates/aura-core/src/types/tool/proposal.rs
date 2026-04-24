//! Tool proposals and the audit-log decision enum.

use serde::{Deserialize, Serialize};

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
    pub fn new(
        tool_use_id: impl Into<String>,
        tool: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
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
