//! Process Manager for async command execution.
//!
//! The `ProcessManager` tracks long-running processes that exceed the sync threshold
//! and creates completion transactions when they finish.

mod monitor;
mod types;

#[cfg(test)]
mod tests;

pub use types::{
    NewProcess, ProcessInfo, ProcessManagerConfig, ProcessMeta, ProcessOutput, RunningProcess,
};

use aura_core_types::time::now_ms;
use aura_core_types::{ActionId, AgentId, Hash, ProcessId, ProcessPending, Transaction};
use dashmap::DashMap;
use std::process::Child;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

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
    pub fn register(
        self: &Arc<Self>,
        agent_id: AgentId,
        reference_tx_hash: Hash,
        action_id: ActionId,
        process_id: ProcessId,
        child: Child,
        command: String,
    ) {
        self.register_with(NewProcess {
            agent_id,
            reference_tx_hash,
            action_id,
            process_id,
            child,
            command,
            meta: ProcessMeta::default(),
        });
    }

    /// [`Self::register`] with where the process runs and logs.
    #[instrument(
        skip(self, process),
        fields(
            agent_id = ?process.agent_id,
            reference_tx_hash = ?process.reference_tx_hash,
            action_id = ?process.action_id,
            process_id = %process.process_id,
            command = %process.command,
        )
    )]
    pub fn register_with(self: &Arc<Self>, process: NewProcess) {
        info!("Registering async process");

        let NewProcess {
            agent_id,
            reference_tx_hash,
            action_id,
            process_id,
            child,
            command,
            meta,
        } = process;
        let running = RunningProcess {
            action_id,
            agent_id,
            process_id,
            reference_tx_hash,
            command,
            started_at: std::time::Instant::now(),
            started_ms: now_ms(),
            meta,
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

    /// The running processes, oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProcessInfo> {
        let mut out: Vec<ProcessInfo> = self
            .processes
            .iter()
            .map(|entry| {
                let running = entry.value();
                ProcessInfo {
                    process_id: running.process_id,
                    agent_id: running.agent_id,
                    action_id: running.action_id,
                    reference_tx_hash: running.reference_tx_hash,
                    command: running.command.clone(),
                    pid: running.child.id(),
                    started_ms: running.started_ms,
                    meta: running.meta.clone(),
                }
            })
            .collect();
        out.sort_by_key(|info| info.started_ms);
        out
    }

    /// Cancel a running process and everything it started. The owning
    /// agent gets a failed completion saying it was killed.
    ///
    /// Returns true if the process was found and killed.
    pub fn cancel(&self, process_id: &ProcessId) -> bool {
        let Some((_, mut running)) = self.processes.remove(process_id) else {
            return false;
        };
        kill_process_group(&mut running.child);
        let _ = running.child.wait();
        info!(process_id = %process_id, "Process cancelled");
        #[allow(clippy::cast_possible_truncation)]
        let output = ProcessOutput {
            exit_code: None,
            stdout: Vec::new(),
            stderr: b"Process killed".to_vec(),
            success: false,
            duration_ms: running.started_at.elapsed().as_millis() as u64,
        };
        match Self::completion_tx(&running, output) {
            Some(tx) => {
                if let Err(e) = self.tx_sender.try_send(tx) {
                    warn!(error = %e, "Failed to send kill completion");
                }
            }
            None => warn!("Failed to build kill completion"),
        }
        true
    }

    /// Create a `ProcessPending` payload for a newly registered process.
    #[must_use]
    pub fn create_pending_payload(process_id: ProcessId, command: &str) -> ProcessPending {
        ProcessPending::new(process_id, command)
    }
}

/// Kill `child` and, on Unix, the process group it leads, so shells take
/// their children down with them.
fn kill_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
}
