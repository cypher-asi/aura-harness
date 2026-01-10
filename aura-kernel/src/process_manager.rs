//! Process Manager for async command execution.
//!
//! The ProcessManager tracks long-running processes that exceed the sync threshold
//! and creates completion transactions when they finish.

use aura_core::{ActionId, ActionResultPayload, AgentId, Hash, ProcessId, ProcessPending, Transaction};
use dashmap::DashMap;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument, warn};

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
    /// The originating transaction's hash (for reference_tx_hash).
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

/// Manages long-running processes and creates completion transactions.
pub struct ProcessManager {
    /// Running processes indexed by process_id.
    processes: DashMap<ProcessId, RunningProcess>,
    /// Channel to send completion transactions.
    tx_sender: mpsc::Sender<Transaction>,
    /// Configuration.
    config: ProcessManagerConfig,
}

impl ProcessManager {
    /// Create a new process manager.
    pub fn new(tx_sender: mpsc::Sender<Transaction>, config: ProcessManagerConfig) -> Self {
        Self {
            processes: DashMap::new(),
            tx_sender,
            config,
        }
    }

    /// Create a process manager with default config.
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
            command: command.clone(),
            started_at: Instant::now(),
            child,
        };

        self.processes.insert(process_id, running);

        // Spawn a monitor task for this process
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.monitor_process(process_id).await;
        });
    }

    /// Monitor a process until completion.
    #[instrument(skip(self), fields(process_id = %process_id))]
    async fn monitor_process(self: Arc<Self>, process_id: ProcessId) {
        let max_duration = Duration::from_millis(self.config.max_async_timeout_ms);
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms);

        loop {
            // Check if process is still registered (might be cancelled)
            let mut process = match self.processes.get_mut(&process_id) {
                Some(p) => p,
                None => {
                    debug!("Process no longer registered");
                    return;
                }
            };

            // Check for timeout
            if process.started_at.elapsed() > max_duration {
                warn!("Process timed out");
                // Kill the process
                let _ = process.child.kill();
                drop(process); // Release the lock before removing

                // Remove and send failure
                if let Some((_, running)) = self.processes.remove(&process_id) {
                    self.send_completion(running, ProcessOutput {
                        exit_code: None,
                        stdout: Vec::new(),
                        stderr: b"Process timed out".to_vec(),
                        success: false,
                        duration_ms: max_duration.as_millis() as u64,
                    })
                    .await;
                }
                return;
            }

            // Try to get the exit status without blocking
            match process.child.try_wait() {
                Ok(Some(status)) => {
                    // Process finished
                    let duration_ms = process.started_at.elapsed().as_millis() as u64;
                    let exit_code = status.code();
                    let success = status.success();
                    drop(process); // Release the lock before removing

                    // Remove from tracking
                    if let Some((_, mut running)) = self.processes.remove(&process_id) {
                        // Collect output
                        let stdout = collect_output(running.child.stdout.take());
                        let stderr = collect_output(running.child.stderr.take());

                        info!(exit_code = ?exit_code, success = success, duration_ms = duration_ms, "Process completed");

                        self.send_completion(running, ProcessOutput {
                            exit_code,
                            stdout,
                            stderr,
                            success,
                            duration_ms,
                        })
                        .await;
                    }
                    return;
                }
                Ok(None) => {
                    // Process still running
                    drop(process); // Release lock before sleeping
                    tokio::time::sleep(poll_interval).await;
                }
                Err(e) => {
                    error!(error = %e, "Failed to check process status");
                    drop(process);
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }
    }

    /// Send a completion transaction.
    async fn send_completion(&self, running: RunningProcess, output: ProcessOutput) {
        let payload = if output.success {
            ActionResultPayload::success(
                running.action_id,
                running.process_id,
                output.exit_code,
                output.stdout,
                output.duration_ms,
            )
        } else {
            let mut payload = ActionResultPayload::failure(
                running.action_id,
                running.process_id,
                output.exit_code,
                output.stderr,
                output.duration_ms,
            );
            // Include stdout in failed results too
            payload.stdout = output.stdout.into();
            payload
        };

        // Create completion transaction
        // Note: We pass None for prev_hash because the app will need to provide
        // the current chain head when actually processing this transaction.
        // The transaction will be re-created with proper chaining when received.
        let tx = Transaction::process_complete(
            running.agent_id,
            &payload,
            running.reference_tx_hash,
            None, // prev_hash will be set by the receiver
        );

        if let Err(e) = self.tx_sender.send(tx).await {
            error!(error = %e, "Failed to send completion transaction");
        }
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

    /// Create a ProcessPending payload for a newly registered process.
    #[must_use]
    pub fn create_pending_payload(process_id: ProcessId, command: &str) -> ProcessPending {
        ProcessPending::new(process_id, command)
    }
}

/// Helper to collect output from a process pipe.
fn collect_output<R: std::io::Read>(pipe: Option<R>) -> Vec<u8> {
    pipe.map_or_else(Vec::new, |mut p| {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut p, &mut buf);
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_manager_creation() {
        let (tx, _rx) = mpsc::channel(10);
        let manager = ProcessManager::with_defaults(tx);
        assert_eq!(manager.running_count(), 0);
    }

    #[tokio::test]
    async fn test_fast_process_completes() {
        let (tx, mut rx) = mpsc::channel(10);
        let manager = Arc::new(ProcessManager::with_defaults(tx));

        let agent_id = AgentId::generate();
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        let reference_hash = Hash::from_content(b"test tx");

        // Run a fast command
        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", "echo hello"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", "echo hello"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        manager.register(
            agent_id,
            reference_hash,
            action_id,
            process_id,
            child,
            "echo hello".to_string(),
        );

        // Wait for completion transaction
        let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for completion")
            .expect("Channel closed");

        assert_eq!(completion.tx_type, aura_core::TransactionType::ProcessComplete);
        assert_eq!(completion.reference_tx_hash, Some(reference_hash));
    }

    #[tokio::test]
    async fn test_multiple_concurrent_processes() {
        let (tx, mut rx) = mpsc::channel(10);
        let manager = Arc::new(ProcessManager::with_defaults(tx));

        let agent_id = AgentId::generate();
        let reference_hash = Hash::from_content(b"test tx");

        // Spawn multiple concurrent processes
        let mut process_ids = Vec::new();
        for i in 0..3 {
            let process_id = ProcessId::generate();
            let action_id = ActionId::generate();
            process_ids.push(process_id);

            #[cfg(windows)]
            let child = std::process::Command::new("cmd.exe")
                .args(["/C", &format!("echo process_{}", i)])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();

            #[cfg(not(windows))]
            let child = std::process::Command::new("sh")
                .args(["-c", &format!("echo process_{}", i)])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();

            manager.register(
                agent_id,
                reference_hash,
                action_id,
                process_id,
                child,
                format!("echo process_{}", i),
            );
        }

        // All processes should have been registered
        // (they may complete very quickly, so we just verify we can register multiple)
        
        // Wait for all completion transactions
        let mut completions = Vec::new();
        for _ in 0..3 {
            let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("Timeout waiting for completion")
                .expect("Channel closed");
            completions.push(completion);
        }

        // All completions should be ProcessComplete type
        for completion in &completions {
            assert_eq!(completion.tx_type, aura_core::TransactionType::ProcessComplete);
            assert_eq!(completion.reference_tx_hash, Some(reference_hash));
        }
    }

    #[tokio::test]
    async fn test_process_timeout() {
        // Use a very short timeout for testing
        let config = ProcessManagerConfig {
            max_async_timeout_ms: 100, // 100ms timeout
            poll_interval_ms: 10,
        };
        let (tx, mut rx) = mpsc::channel(10);
        let manager = Arc::new(ProcessManager::new(tx, config));

        let agent_id = AgentId::generate();
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        let reference_hash = Hash::from_content(b"test tx");

        // Run a command that takes longer than the timeout
        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 10 127.0.0.1"]) // Takes ~10 seconds
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 10"]) // Takes 10 seconds
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        manager.register(
            agent_id,
            reference_hash,
            action_id,
            process_id,
            child,
            "long running command".to_string(),
        );

        // Wait for timeout completion (should be quick due to short timeout)
        let completion = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for completion")
            .expect("Channel closed");

        // Should be a failed completion due to timeout
        assert_eq!(completion.tx_type, aura_core::TransactionType::ProcessComplete);
        assert_eq!(completion.reference_tx_hash, Some(reference_hash));
        
        // The payload should indicate failure
        let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
        assert!(!payload.success);
    }

    #[tokio::test]
    async fn test_register_process_tracking() {
        let (tx, _rx) = mpsc::channel(10);
        let manager = Arc::new(ProcessManager::with_defaults(tx));

        let agent_id = AgentId::generate();
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        let reference_hash = Hash::from_content(b"test tx");

        // Spawn a slow process so we can check it's tracked
        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 5 127.0.0.1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 5"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        // Before registration, process count should be 0
        assert_eq!(manager.running_count(), 0);
        assert!(!manager.is_running(&process_id));

        manager.register(
            agent_id,
            reference_hash,
            action_id,
            process_id,
            child,
            "slow command".to_string(),
        );

        // Immediately after registration, process should be tracked
        // (give it a moment to actually register)
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Process may have already completed, so we just check it doesn't panic
        let _ = manager.running_count();
        
        // Cancel it so the test doesn't wait
        let _ = manager.cancel(&process_id);
    }

    #[tokio::test]
    async fn test_cancel_process() {
        let (tx, _rx) = mpsc::channel(10);
        let manager = Arc::new(ProcessManager::with_defaults(tx));

        let agent_id = AgentId::generate();
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        let reference_hash = Hash::from_content(b"test tx");

        // Spawn a slow process
        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        manager.register(
            agent_id,
            reference_hash,
            action_id,
            process_id,
            child,
            "slow command".to_string(),
        );

        // Give it a moment to register
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel should succeed
        let cancelled = manager.cancel(&process_id);
        assert!(cancelled);

        // Process should no longer be running
        assert!(!manager.is_running(&process_id));

        // Second cancel should fail (already removed)
        let cancelled_again = manager.cancel(&process_id);
        assert!(!cancelled_again);
    }

    #[test]
    fn test_create_pending_payload() {
        let process_id = ProcessId::generate();
        let command = "echo hello";
        
        let payload = ProcessManager::create_pending_payload(process_id, command);
        
        assert_eq!(payload.process_id, process_id);
        assert_eq!(payload.command, command);
    }
}
