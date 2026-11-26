//! Running a spawned command to completion: threshold waits, hard
//! timeouts that kill the whole process group, and capped pipe reads.

use super::live::LiveOutput;
use super::output::{output_to_tool_result, output_to_tool_result_with_program};
use super::spawn::{cmd_spawn, cmd_spawn_shell_script, spawn_program};
use super::stop::Registration;
use super::{ProcessTags, ThresholdResult};
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use aura_core_types::ToolResult;
use tracing::instrument;

#[cfg(unix)]
fn terminate_child_process_group(child: &mut std::process::Child) {
    let target = format!("-{}", child.id());
    let _ = std::process::Command::new("kill")
        .args(["-KILL", "--", &target])
        .status();
    let _ = child.kill();

    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_) => return,
        }
    }

    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_child_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

/// Run a command with threshold-based execution.
///
/// This waits for the command to complete up to `sync_threshold_ms`.
/// - If the command completes within the threshold, returns `ThresholdResult::Completed`
/// - If the command is still running after the threshold, returns `ThresholdResult::Pending`
///   with the child handle for async tracking
///
/// The command is spawned directly (no shell wrapping); see [`cmd_spawn`].
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_run_with_threshold(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    sync_threshold_ms: u64,
) -> Result<(ThresholdResult, String), ToolError> {
    use std::time::Duration;

    let (child, full_command) = cmd_spawn(sandbox, program, args, cwd)?;

    let result = wait_with_threshold(child, Duration::from_millis(sync_threshold_ms));
    Ok((result, full_command))
}

/// Run a command synchronously with a timeout.
///
/// Spawns directly (no shell). Use `cmd_run_with_threshold` for
/// async-capable execution.
#[cfg(test)]
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_run(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    timeout_ms: u64,
) -> Result<ToolResult, ToolError> {
    run_program(
        sandbox,
        program,
        args,
        cwd,
        timeout_ms,
        &ProcessTags::default(),
    )
}

/// [`cmd_run`] with `tags` in the environment.
pub(super) fn run_program(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    timeout_ms: u64,
    tags: &ProcessTags,
) -> Result<ToolResult, ToolError> {
    use std::time::Duration;

    let (child, _full_command) = spawn_program(sandbox, program, args, cwd, tags)?;
    let stop = Registration::new(tags, Some(child.id()));

    let timeout = Duration::from_millis(timeout_ms);
    let output = match wait_with_hard_timeout(child, timeout, &stop, &tags.output) {
        Ok(out) => out,
        Err(e) => {
            return Err(ToolError::CommandFailed(format!("Command timed out: {e}")));
        }
    };

    output_to_tool_result_with_program(output, Some(program), args).map(|r| stop.annotate(r))
}

/// Run a raw shell script synchronously with a timeout.
///
/// See [`cmd_spawn_shell_script`] — the caller is responsible for
/// enforcing `ToolConfig::command.allowed_shell_scripts`
/// before invoking this.
pub(super) fn cmd_run_shell_script(
    sandbox: &Sandbox,
    shell_script: &str,
    cwd: Option<&str>,
    timeout_ms: u64,
    tags: &ProcessTags,
) -> Result<ToolResult, ToolError> {
    use std::time::Duration;

    let (child, _display) = cmd_spawn_shell_script(sandbox, shell_script, cwd, tags)?;
    let stop = Registration::new(tags, Some(child.id()));

    let timeout = Duration::from_millis(timeout_ms);
    let output = match wait_with_hard_timeout(child, timeout, &stop, &tags.output) {
        Ok(out) => out,
        Err(e) => {
            return Err(ToolError::CommandFailed(format!("Command timed out: {e}")));
        }
    };

    output_to_tool_result(output).map(|r| stop.annotate(r))
}

/// Wait for a child process with a threshold.
///
/// If the process completes within the threshold, returns `ThresholdResult::Completed`.
/// If the process is still running after the threshold, returns `ThresholdResult::Pending`
/// with the child handle intact (NOT killed).
fn wait_with_threshold(
    mut child: std::process::Child,
    threshold: std::time::Duration,
) -> ThresholdResult {
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let none = LiveOutput::default();
            let stdout = (child.stdout.take()).map_or_else(Vec::new, |s| read_capped(s, &none));
            let stderr = (child.stderr.take()).map_or_else(Vec::new, |s| read_capped(s, &none));
            return ThresholdResult::Completed(std::process::Output {
                status,
                stdout,
                stderr,
            });
        } else if start.elapsed() > threshold {
            return ThresholdResult::Pending(child);
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Read `pipe` to its end on its own thread, showing `live` each chunk.
fn drain_pipe<R: std::io::Read + Send + 'static>(
    pipe: R,
    live: LiveOutput,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || read_capped(pipe, &live))
}

/// Bytes of a pipe's output kept from its start, and as many from its
/// end; the middle of a runaway program's output is dropped as it is
/// read, so many agents' builds can't fill memory.
pub(super) const PIPE_KEEP_BYTES: usize = 512 * 1024;

/// Read `pipe` to its end, keeping its first and last [`PIPE_KEEP_BYTES`]
/// and a line saying how much was cut between them. `live` is shown every
/// chunk as it is read.
pub(super) fn read_capped<R: std::io::Read>(mut pipe: R, live: &LiveOutput) -> Vec<u8> {
    let mut head = Vec::new();
    let mut tail = std::collections::VecDeque::<u8>::new();
    let mut cut = 0usize;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut bytes = &chunk[..n];
        live.push(bytes);
        if head.len() < PIPE_KEEP_BYTES {
            let room = (PIPE_KEEP_BYTES - head.len()).min(bytes.len());
            head.extend_from_slice(&bytes[..room]);
            bytes = &bytes[room..];
        }
        tail.extend(bytes);
        if tail.len() > PIPE_KEEP_BYTES {
            let over = tail.len() - PIPE_KEEP_BYTES;
            tail.drain(..over);
            cut += over;
        }
    }
    if cut > 0 {
        head.extend_from_slice(format!("\n... ({cut} bytes of output dropped) ...\n").as_bytes());
    }
    head.extend(tail);
    head
}

/// Wait for a child process with a hard timeout (kills on timeout).
///
/// This is the original timeout behavior - if the process doesn't complete
/// within the timeout, it is killed and an error is returned. A child the
/// user stopped through `stop` is killed once its grace runs out, and its
/// output so far returned. `live` is shown the output as it arrives.
fn wait_with_hard_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
    stop: &Registration,
    live: &LiveOutput,
) -> std::io::Result<std::process::Output> {
    use std::thread;
    use std::time::Instant;

    // Read both pipes while waiting: a child that fills a pipe buffer
    // (about 64 KB) blocks on write and would never exit.
    let stdout = child
        .stdout
        .take()
        .map(|pipe| drain_pipe(pipe, live.clone()));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| drain_pipe(pipe, live.clone()));
    let collect = |reader: Option<thread::JoinHandle<Vec<u8>>>| {
        reader.map_or_else(Vec::new, |handle| handle.join().unwrap_or_default())
    };

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let output = std::process::Output {
                status,
                stdout: collect(stdout),
                stderr: collect(stderr),
            };
            live.finish();
            return Ok(output);
        }
        live.tick();

        if stop.overdue() {
            terminate_child_process_group(&mut child);
            continue;
        }
        if start.elapsed() > timeout {
            terminate_child_process_group(&mut child);
            live.finish();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Process timed out",
            ));
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}
