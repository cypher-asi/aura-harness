//! Approval management for tool requests.
//!
//! Handles the approval flow for tools that require user confirmation.

use aura_core::ToolCall;
use aura_kernel::PermissionLevel;
use std::collections::VecDeque;

// ============================================================================
// Approval Request
// ============================================================================

/// A pending approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Request ID
    pub id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments (reserved for approval UI)
    pub args: serde_json::Value,
    /// Permission level (reserved for approval UI)
    pub permission: PermissionLevel,
    /// Description for the user
    pub description: String,
}

impl ApprovalRequest {
    /// Create a new approval request.
    #[must_use]
    pub fn new(id: impl Into<String>, tool_call: &ToolCall, permission: PermissionLevel) -> Self {
        let description = format_tool_description(&tool_call.tool, &tool_call.args);
        Self {
            id: id.into(),
            tool: tool_call.tool.clone(),
            args: tool_call.args.clone(),
            permission,
            description,
        }
    }
}

/// Format a human-readable description of a tool call.
fn format_tool_description(tool: &str, args: &serde_json::Value) -> String {
    match tool {
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Write to file: {path}")
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Edit file: {path}")
        }
        "run_command" => {
            let program = args.get("program").and_then(|v| v.as_str()).unwrap_or("?");
            let cmd_args = args
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            format!("Run command: {program} {cmd_args}")
        }
        _ => format!("Execute tool: {tool}"),
    }
}

// ============================================================================
// Approval Queue
// ============================================================================

/// Approval decision (reserved for approval UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Approve the request
    Approve,
    /// Deny the request
    Deny,
    /// Approve for the entire session
    ApproveSession,
}

/// Queue of pending approval requests.
#[derive(Debug, Default)]
pub struct ApprovalQueue {
    pending: VecDeque<ApprovalRequest>,
}

impl ApprovalQueue {
    /// Create a new empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a request to the queue.
    pub fn push(&mut self, request: ApprovalRequest) {
        self.pending.push_back(request);
    }

    /// Get the next pending request without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<&ApprovalRequest> {
        self.pending.front()
    }

    /// Remove and return the next pending request.
    pub fn pop(&mut self) -> Option<ApprovalRequest> {
        self.pending.pop_front()
    }

    /// Check if there are pending requests.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Get the number of pending requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Check if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Clear all pending requests.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_request() {
        let tool_call = ToolCall::new(
            "write_file",
            serde_json::json!({ "path": "test.txt", "content": "hello" }),
        );
        let request = ApprovalRequest::new("req-1", &tool_call, PermissionLevel::AskOnce);

        assert_eq!(request.tool, "write_file");
        assert!(request.description.contains("test.txt"));
    }

    #[test]
    fn test_approval_queue() {
        let mut queue = ApprovalQueue::new();
        assert!(queue.is_empty());

        let tool_call = ToolCall::new("write_file", serde_json::json!({}));
        queue.push(ApprovalRequest::new(
            "1",
            &tool_call,
            PermissionLevel::AskOnce,
        ));

        assert!(!queue.is_empty());
        assert_eq!(queue.len(), 1);
        assert!(queue.has_pending());

        let request = queue.pop().unwrap();
        assert_eq!(request.id, "1");
        assert!(queue.is_empty());
    }

    #[test]
    fn test_approval_queue_fifo_order() {
        let mut queue = ApprovalQueue::new();
        let tc1 = ToolCall::new("write_file", serde_json::json!({"path": "a.txt"}));
        let tc2 = ToolCall::new("edit_file", serde_json::json!({"path": "b.txt"}));
        let tc3 = ToolCall::new("run_command", serde_json::json!({"program": "ls"}));

        queue.push(ApprovalRequest::new("1", &tc1, PermissionLevel::AskOnce));
        queue.push(ApprovalRequest::new("2", &tc2, PermissionLevel::AlwaysAsk));
        queue.push(ApprovalRequest::new("3", &tc3, PermissionLevel::AskOnce));

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop().unwrap().id, "1");
        assert_eq!(queue.pop().unwrap().id, "2");
        assert_eq!(queue.pop().unwrap().id, "3");
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_approval_queue_peek_does_not_remove() {
        let mut queue = ApprovalQueue::new();
        let tc = ToolCall::new("write_file", serde_json::json!({}));
        queue.push(ApprovalRequest::new("1", &tc, PermissionLevel::AskOnce));

        assert_eq!(queue.peek().unwrap().id, "1");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek().unwrap().id, "1");
    }

    #[test]
    fn test_approval_queue_clear() {
        let mut queue = ApprovalQueue::new();
        let tc = ToolCall::new("write_file", serde_json::json!({}));
        queue.push(ApprovalRequest::new("1", &tc, PermissionLevel::AskOnce));
        queue.push(ApprovalRequest::new("2", &tc, PermissionLevel::AskOnce));
        assert_eq!(queue.len(), 2);

        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(!queue.has_pending());
    }

    #[test]
    fn test_approval_request_fs_edit_description() {
        let tc = ToolCall::new(
            "edit_file",
            serde_json::json!({"path": "src/main.rs", "old_text": "a", "new_text": "b"}),
        );
        let req = ApprovalRequest::new("1", &tc, PermissionLevel::AlwaysAsk);
        assert!(req.description.contains("src/main.rs"));
        assert!(req.description.contains("Edit"));
    }

    #[test]
    fn test_approval_request_cmd_run_description() {
        let tc = ToolCall::new(
            "run_command",
            serde_json::json!({"program": "cargo", "args": ["test", "--workspace"]}),
        );
        let req = ApprovalRequest::new("1", &tc, PermissionLevel::AskOnce);
        assert!(req.description.contains("cargo"));
        assert!(req.description.contains("test"));
    }

    #[test]
    fn test_approval_request_unknown_tool_description() {
        let tc = ToolCall::new("custom_tool", serde_json::json!({"foo": "bar"}));
        let req = ApprovalRequest::new("1", &tc, PermissionLevel::AskOnce);
        assert!(req.description.contains("custom_tool"));
    }

    #[test]
    fn test_approval_decision_equality() {
        assert_eq!(ApprovalDecision::Approve, ApprovalDecision::Approve);
        assert_ne!(ApprovalDecision::Approve, ApprovalDecision::Deny);
        assert_ne!(ApprovalDecision::ApproveSession, ApprovalDecision::Approve);
    }
}
