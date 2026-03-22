//! Tool-related types: proposals, executions, definitions, calls, and results.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Definition for an external tool registered at runtime via `session_init`.
///
/// External tools are dispatched via HTTP POST to a callback URL.
/// This type is shared between the session protocol and the tool executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalToolDefinition {
    /// Tool name (must be unique across all tools).
    pub name: String,
    /// Human-readable description for the model.
    pub description: String,
    /// JSON Schema for input parameters.
    pub input_schema: serde_json::Value,
    /// HTTP endpoint that handles tool execution.
    pub callback_url: String,
}

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
    #[serde(default, with = "super::bytes_serde")]
    pub stdout: Bytes,
    /// Standard error
    #[serde(default, with = "super::bytes_serde")]
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
