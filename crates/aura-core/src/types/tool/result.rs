//! Tool execution result envelope returned to the kernel.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-tool line-count summary for file-mutating tools.
///
/// Populated by tools that have direct access to pre- and post-mutation
/// file content at execution time (`fs_write`, `fs_edit`, `fs_delete`).
/// Surfaces upward through the kernel boundary via [`ToolResult::line_diff`]
/// and `ToolOutput::line_diff`, eventually landing on the per-task
/// `files_changed` summary aura-os-server persists for the dashboard's
/// "Lines" stat.
///
/// Tools that can't compute a diff (or for which it doesn't apply) leave
/// the field at `None`. Downstream consumers must treat `None` as
/// "unknown", not "zero" — the absence-vs-presence distinction is what
/// lets the dashboard tell "no edit happened" apart from "tool didn't
/// report counts".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineDiff {
    pub lines_added: u32,
    pub lines_removed: u32,
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
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
    pub stdout: Bytes,
    /// Standard error
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
    pub stderr: Bytes,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Optional per-file line diff produced by file-mutating tools
    /// (`fs_write`, `fs_edit`, `fs_delete`). `None` means "the tool
    /// didn't report counts" — consumers must not interpret it as a
    /// zero-line change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_diff: Option<LineDiff>,
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
            line_diff: None,
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
            line_diff: None,
        }
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Attach a typed per-file line diff. Used by `fs_write`, `fs_edit`,
    /// and `fs_delete` to surface the line counts they compute at
    /// execution time. The kernel boundary copies the value through to
    /// `ToolOutput::line_diff` so the agent loop can build accurate
    /// `FileChange` entries without re-reading the filesystem.
    #[must_use]
    pub fn with_line_diff(mut self, lines_added: u32, lines_removed: u32) -> Self {
        self.line_diff = Some(LineDiff {
            lines_added,
            lines_removed,
        });
        self
    }
}
