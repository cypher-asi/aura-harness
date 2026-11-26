//! A tool call waiting for the user to approve it.

/// Pending approval request.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Approval request ID
    pub id: String,
    /// Tool name
    pub tool: String,
    /// Description of the action
    pub description: String,
    /// What "always allow" would allow; empty when it isn't offered
    pub always: String,
}
