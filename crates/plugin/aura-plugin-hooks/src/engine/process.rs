//! Hook process spawning: command resolution, scrubbed env wiring,
//! bounded wait with output capture, and exit-code mapping.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{HookEngine, RegisteredHook, HOOK_STDOUT_CAP_BYTES};
use crate::context::HookFiringContext;
use crate::error::HookError;
use crate::event::HookEvent;
use crate::outcome::HookOutcome;
use crate::sandbox::{self, InjectedEnv};

impl HookEngine {
    /// Spawn a single Phase 8 hook process and translate its exit
    /// status into a [`HookOutcome`].
    ///
    /// Outcome mapping (mirrors the Codex / Claude conventions):
    /// - exit code 0 → `Continue`.
    /// - exit code 2 → `Block { reason }` (reason from stderr).
    /// - exit code 3 → `Approve` (PermissionRequest).
    /// - exit code 4 → `Deny { reason }` (PermissionRequest).
    /// - exit code 5 → `Replace { new_value }` (stdout is the
    ///   replacement payload).
    /// - any other non-zero exit, spawn failure, or timeout →
    ///   `TimedOut`-equivalent: warn-log + `Continue`.
    pub(super) fn spawn_phase8(
        &self,
        hook: &RegisteredHook,
        injected: &InjectedEnv,
    ) -> HookOutcome {
        let prog = resolve_command(&hook.command, &hook.plugin_root);
        let env = sandbox::scrubbed_env(injected);

        let mut cmd = Command::new(prog);
        cmd.args(&hook.args);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started_at = Instant::now();
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(
                    plugin_id = %hook.plugin_id,
                    event = ?hook.event,
                    error = %err,
                    "hook spawn failed; treating as Continue"
                );
                return HookOutcome::Continue;
            }
        };

        let timeout = self.timeout;
        let wait_result = wait_with_timeout(child, timeout);
        let elapsed_ms = started_at.elapsed().as_millis();

        match wait_result {
            WaitResult::Completed {
                code,
                stdout,
                stderr,
            } => {
                truncate_warn(&hook.plugin_id, hook.event, &stdout, &stderr);
                let stdout_str = String::from_utf8_lossy(&stdout);
                let stderr_str = String::from_utf8_lossy(&stderr);
                map_exit_code(hook, code, &stdout_str, &stderr_str, elapsed_ms)
            }
            WaitResult::TimedOut => {
                tracing::warn!(
                    plugin_id = %hook.plugin_id,
                    event = ?hook.event,
                    timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    "hook handler timed out; treating as Continue"
                );
                HookOutcome::TimedOut
            }
        }
    }

    /// Phase 4c spawn path used by [`Self::fire`]. Retained for
    /// backward compatibility with the existing integration test.
    pub(super) fn spawn_phase4c(
        &self,
        hook: &RegisteredHook,
        ctx: &HookFiringContext,
    ) -> Result<(), HookError> {
        let prog = resolve_command(&hook.command, &hook.plugin_root);

        let mut env = sandbox::scrubbed_inherit();
        env.extend(ctx.env_vars());
        for (k, v) in &hook.env {
            env.insert(k.clone(), v.clone());
        }

        let mut cmd = Command::new(prog);
        cmd.args(&hook.args);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let output = cmd.output().map_err(|e| HookError::Spawn {
            plugin_id: hook.plugin_id.clone(),
            event: hook.event,
            source: e,
        })?;
        if !output.status.success() {
            return Err(HookError::ExitFailure {
                plugin_id: hook.plugin_id.clone(),
                event: hook.event,
                code: output.status.code(),
            });
        }
        Ok(())
    }
}

enum WaitResult {
    Completed {
        code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    TimedOut,
}

/// Wait for a child process up to `timeout`, capturing stdout/stderr
/// truncated at [`HOOK_STDOUT_CAP_BYTES`]. On timeout the child is
/// killed.
fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> WaitResult {
    use std::io::Read;

    // Lift the pipe handles BEFORE polling so we can read after
    // wait_with_output's child is consumed elsewhere.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                if let Some(mut s) = stdout_pipe.take() {
                    let mut taker = (&mut s).take(HOOK_STDOUT_CAP_BYTES as u64 + 1);
                    let _ = taker.read_to_end(&mut stdout_buf);
                }
                if let Some(mut s) = stderr_pipe.take() {
                    let mut taker = (&mut s).take(HOOK_STDOUT_CAP_BYTES as u64 + 1);
                    let _ = taker.read_to_end(&mut stderr_buf);
                }
                return WaitResult::Completed {
                    code,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return WaitResult::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return WaitResult::TimedOut;
            }
        }
    }
}

fn truncate_warn(plugin_id: &str, event: HookEvent, stdout: &[u8], stderr: &[u8]) {
    if stdout.len() > HOOK_STDOUT_CAP_BYTES {
        tracing::warn!(
            plugin_id,
            event = ?event,
            captured = stdout.len(),
            cap_bytes = HOOK_STDOUT_CAP_BYTES,
            "hook stdout truncated to cap"
        );
    }
    if stderr.len() > HOOK_STDOUT_CAP_BYTES {
        tracing::warn!(
            plugin_id,
            event = ?event,
            captured = stderr.len(),
            cap_bytes = HOOK_STDOUT_CAP_BYTES,
            "hook stderr truncated to cap"
        );
    }
}

fn map_exit_code(
    hook: &RegisteredHook,
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
    elapsed_ms: u128,
) -> HookOutcome {
    let plugin_id = &hook.plugin_id;
    let event = hook.event;
    let elapsed_ms_u64 = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
    match code {
        Some(0) => {
            tracing::debug!(
                plugin_id = %plugin_id,
                event = ?event,
                elapsed_ms = elapsed_ms_u64,
                "hook returned Continue"
            );
            HookOutcome::Continue
        }
        Some(2) => HookOutcome::Block {
            reason: stderr.trim().to_string(),
        },
        Some(3) => HookOutcome::Approve,
        Some(4) => HookOutcome::Deny {
            reason: stderr.trim().to_string(),
        },
        Some(5) => HookOutcome::Replace {
            new_value: stdout.trim_end_matches('\n').to_string(),
        },
        other => {
            tracing::warn!(
                plugin_id = %plugin_id,
                event = ?event,
                code = ?other,
                "hook exited with unrecognised code; treating as Continue"
            );
            HookOutcome::Continue
        }
    }
}

/// Resolve a hook command string against the plugin root.
pub(super) fn resolve_command(command: &str, plugin_root: &Path) -> PathBuf {
    let has_separator = command.contains('/') || command.contains('\\');
    let as_path = Path::new(command);
    if as_path.is_absolute() {
        return as_path.to_path_buf();
    }
    if has_separator {
        return plugin_root.join(as_path);
    }
    PathBuf::from(command)
}
