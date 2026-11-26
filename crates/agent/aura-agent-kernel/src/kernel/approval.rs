//! Live tool-approval types: the prompt, the response and the bridge trait.

use async_trait::async_trait;
use aura_core_types::{AgentId, ToolState};

/// Details about a tool invocation that was denied because it needs an
/// out-of-band operator approval. Set on [`ToolOutput::approval_required`](super::ToolOutput::approval_required)
/// when the policy returns [`crate::PolicyVerdict::RequireApproval`].
#[derive(Debug, Clone)]
pub struct ApprovalRequiredInfo {
    /// Tool name, e.g. `"run_command"`.
    pub tool: String,
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
