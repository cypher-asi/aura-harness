//! Process monitoring and completion logic.

use super::types::{ProcessOutput, RunningProcess};
use super::ProcessManager;
use aura_core::{ActionResultPayload, ProcessId, Transaction};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

impl ProcessManager {
    /// Monitor a process until completion or timeout.
    #[instrument(skip(self), fields(process_id = %process_id))]
    pub(super) async fn monitor_process(self: Arc<Self>, process_id: ProcessId) {
        let max_duration = Duration::from_millis(self.config.max_async_timeout_ms);
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms);

        loop {
            let Some(mut process) = self.processes.get_mut(&process_id) else {
                debug!("Process no longer registered");
                return;
            };

            if process.started_at.elapsed() > max_duration {
                warn!("Process timed out");
                let _ = process.child.kill();
                drop(process);
                self.handle_timeout(process_id, max_duration).await;
                return;
            }

            match process.child.try_wait() {
                Ok(Some(status)) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let duration_ms = process.started_at.elapsed().as_millis() as u64;
                    let exit_code = status.code();
                    let success = status.success();
                    drop(process);
                    self.handle_completed(process_id, exit_code, success, duration_ms)
                        .await;
                    return;
                }
                Ok(None) => {
                    drop(process);
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

    /// Remove a timed-out process and send a failure completion.
    async fn handle_timeout(&self, process_id: ProcessId, max_duration: Duration) {
        if let Some((_, running)) = self.processes.remove(&process_id) {
            let output = ProcessOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr: b"Process timed out".to_vec(),
                success: false,
                #[allow(clippy::cast_possible_truncation)]
                duration_ms: max_duration.as_millis() as u64,
            };
            self.send_completion(running, output).await;
        }
    }

    /// Collect output from a finished process and send the completion transaction.
    async fn handle_completed(
        &self,
        process_id: ProcessId,
        exit_code: Option<i32>,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some((_, mut running)) = self.processes.remove(&process_id) {
            let (stdout, stderr) = tokio::join!(
                collect_output(running.child.stdout.take()),
                collect_output(running.child.stderr.take()),
            );

            info!(
                exit_code = ?exit_code,
                success = success,
                duration_ms = duration_ms,
                "Process completed"
            );

            let output = ProcessOutput {
                exit_code,
                stdout,
                stderr,
                success,
                duration_ms,
            };
            self.send_completion(running, output).await;
        }
    }

    /// Build and send a completion transaction for a finished process.
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
            payload.stdout = output.stdout.into();
            payload
        };

        let tx = match Transaction::process_complete(
            running.agent_id,
            &payload,
            running.reference_tx_hash,
            None,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                error!(error = %e, "Failed to create completion transaction");
                return;
            }
        };

        if let Err(e) = self.tx_sender.send(tx).await {
            error!(error = %e, "Failed to send completion transaction");
        }
    }
}

const MAX_PROCESS_OUTPUT: usize = 10 * 1024 * 1024;

/// Collect output from a process pipe without blocking the async runtime.
///
/// Output is capped at [`MAX_PROCESS_OUTPUT`] bytes (10 MiB). When the cap is
/// reached the trailing bytes are replaced with a truncation marker.
async fn collect_output<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> Vec<u8> {
    match pipe {
        None => Vec::new(),
        Some(pipe) => tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let n = pipe
                .take(MAX_PROCESS_OUTPUT as u64)
                .read_to_end(&mut buf)
                .unwrap_or(0);
            if n >= MAX_PROCESS_OUTPUT {
                buf.extend_from_slice(b"\n... [output truncated at 10 MiB]");
            }
            buf
        })
        .await
        .unwrap_or_default(),
    }
}
