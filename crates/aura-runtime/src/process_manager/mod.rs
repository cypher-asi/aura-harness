//! Process Manager for async command execution.
//!
//! The `ProcessManager` tracks long-running processes that exceed the sync threshold
//! and creates completion transactions when they finish.

mod monitor;
mod types;

#[cfg(test)]
mod tests;

pub use types::{ProcessManagerConfig, ProcessOutput, RunningProcess};

use aura_core::{ActionId, AgentId, Hash, ProcessId, ProcessPending, Transaction};
use dashmap::DashMap;
use std::process::Child;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, instrument};

/// Manages long-running processes and creates completion transactions.
pub struct ProcessManager {
    /// Running processes indexed by `process_id`.
    pub(crate) processes: DashMap<ProcessId, RunningProcess>,
    /// Channel to send completion transactions.
    pub(crate) tx_sender: mpsc::Sender<Transaction>,
    /// Configuration.
    pub(crate) config: ProcessManagerConfig,
}

impl ProcessManager {
    /// Create a new process manager.
    #[must_use]
    pub fn new(tx_sender: mpsc::Sender<Transaction>, config: ProcessManagerConfig) -> Self {
        Self {
            processes: DashMap::new(),
            tx_sender,
            config,
        }
    }

    /// Create a process manager with default config.
    #[must_use]
    pub fn with_defaults(tx_sender: mpsc::Sender<Transaction>) -> Self {
        Self::new(tx_sender, ProcessManagerConfig::default())
    }

    /// Register a process for async monitoring.
    ///
    /// This spawns a background task that waits for the process to complete
    /// and sends a completion transaction.
    #[instrument(skip(self, child), fields(process_id = %process_id, command = %command))]
    pub fn register(
        self: &Arc<Self>,
        agent_id: AgentId,
        reference_tx_hash: Hash,
        action_id: ActionId,
        process_id: ProcessId,
        child: Child,
        command: String,
    ) {
        info!("Registering async process");

        let running = RunningProcess {
            action_id,
            agent_id,
            process_id,
            reference_tx_hash,
            command,
            started_at: std::time::Instant::now(),
            child,
        };

        self.processes.insert(process_id, running);

        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.monitor_process(process_id).await;
        });
    }

    /// Get the number of currently running processes.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.processes.len()
    }

    /// Check if a process is still running.
    #[must_use]
    pub fn is_running(&self, process_id: &ProcessId) -> bool {
        self.processes.contains_key(process_id)
    }

    /// Cancel a running process.
    ///
    /// Returns true if the process was found and killed.
    pub fn cancel(&self, process_id: &ProcessId) -> bool {
        if let Some((_, mut running)) = self.processes.remove(process_id) {
            let _ = running.child.kill();
            info!(process_id = %process_id, "Process cancelled");
            true
        } else {
            false
        }
    }

    /// Create a `ProcessPending` payload for a newly registered process.
    #[must_use]
    pub fn create_pending_payload(process_id: ProcessId, command: &str) -> ProcessPending {
        ProcessPending::new(process_id, command)
    }
}
