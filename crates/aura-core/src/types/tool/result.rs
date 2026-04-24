//! Tool execution result envelope returned to the kernel.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
    pub stdout: Bytes,
    /// Standard error
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
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
