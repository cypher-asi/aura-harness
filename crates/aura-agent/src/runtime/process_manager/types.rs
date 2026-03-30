//! Types for the process manager.

use aura_core::{ActionId, AgentId, Hash, ProcessId};
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
    /// The child process handle.
    pub child: Child,
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
