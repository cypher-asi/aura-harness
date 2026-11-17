//! Types for the process manager.

use aura_core_types::{ActionId, AgentId, Hash, ProcessId};
use std::path::PathBuf;
use std::process::Child;
use std::time::Instant;

/// Configuration for the process manager.
#[derive(Debug, Clone)]
pub struct ProcessManagerConfig {
    /// Maximum timeout for async processes (milliseconds).
    pub max_async_timeout_ms: u64,
    /// Polling interval for process completion (milliseconds).
    pub poll_interval_ms: u64,
}

impl Default for ProcessManagerConfig {
    fn default() -> Self {
        Self {
            max_async_timeout_ms: 600_000, // 10 minutes
            poll_interval_ms: 100,         // 100ms polling
        }
    }
}

/// Where a process runs and where its output goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessMeta {
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// File the process writes its stdout and stderr to, when it isn't piped.
    pub log_path: Option<PathBuf>,
    /// Model `tool_use` id of the call that started it.
    pub tool_use_id: Option<String>,
}

/// Information about a running process.
pub struct RunningProcess {
    /// The action ID this process belongs to.
    pub action_id: ActionId,
    /// The agent ID this process belongs to.
    pub agent_id: AgentId,
    /// Unique process identifier.
    pub process_id: ProcessId,
    /// The originating transaction's hash (for `reference_tx_hash`).
    pub reference_tx_hash: Hash,
    /// The command being executed.
    pub command: String,
    /// When the process started.
    pub started_at: Instant,
    /// When the process started, in milliseconds since the epoch.
    pub started_ms: u64,
    pub meta: ProcessMeta,
    /// The child process handle.
    pub child: Child,
}

/// Read-only view of a running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub process_id: ProcessId,
    pub agent_id: AgentId,
    pub action_id: ActionId,
    pub reference_tx_hash: Hash,
    pub command: String,
    pub pid: u32,
    /// Milliseconds since the epoch.
    pub started_ms: u64,
    pub meta: ProcessMeta,
}

/// Output from a completed process.
#[derive(Debug)]
pub struct ProcessOutput {
    /// Exit code (if available).
    pub exit_code: Option<i32>,
    /// Standard output.
    pub stdout: Vec<u8>,
    /// Standard error.
    pub stderr: Vec<u8>,
    /// Whether the process succeeded.
    pub success: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}
