//! Build command execution and test output parsing.
//!
//! Provides [`run_build_command`] for executing shell commands with streaming
//! output, timeout handling, and output truncation. Also includes parsers for
//! cargo test, Jest, and generic test output formats.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use aura_exec_tools::RunScratch;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

mod test_output;

pub use test_output::parse_test_output;

/// Typed errors returned by [`run_build_command`] and its helpers.
///
/// Library-crate signature replacement for the previous
/// `anyhow::Result<_>` returns. `anyhow::Error: From<E>` for any
/// `E: std::error::Error + 'static` so binary call sites still get
/// the same `?` ergonomics.
#[derive(Debug, thiserror::Error)]
pub enum BuildRunnerError {
    /// Caller passed an empty command string.
    #[error("build_command is empty")]
    EmptyCommand,
    /// `Command::spawn` failed.
    #[error("failed to execute build command `{command}` in `{cwd}`: {source}")]
    SpawnFailed {
        /// The literal command string.
        command: String,
        /// Working directory it was invoked in.
        cwd: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// IO error while reading a child process pipe.
    #[error("failed reading build command {stream}: {source}")]
    PipeRead {
        /// `"stdout"` or `"stderr"`.
        stream: &'static str,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// IO error while waiting for the child process.
    #[error("IO error waiting for build command `{command}`: {source}")]
    Wait {
        /// The literal command string.
        command: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// A spawned `tokio::task::JoinHandle` failed.
    #[error("build command `{command}` {stream} collector failed: {source}")]
    CollectorJoin {
        /// The literal command string.
        command: String,
        /// `"stdout"` or `"stderr"`.
        stream: String,
        /// Underlying join error.
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Convenience alias for the join handle returned by the output
/// collector tasks.
type CollectorHandle = tokio::task::JoinHandle<Result<String, BuildRunnerError>>;

use aura_config::MAX_OUTPUT_BYTES;
const BUILD_TIMEOUT: Duration = Duration::from_secs(aura_config::BUILD_TIMEOUT_SECS);

/// How long output from a killed, timed-out build is waited for.
const TIMEOUT_COLLECT_GRACE: Duration = Duration::from_secs(5);

fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let start = &s[..floor_char_boundary(s, half)];
    let end = &s[ceil_char_boundary(s, s.len() - half)..];
    format!(
        "{start}\n\n... (truncated {0} bytes) ...\n\n{end}",
        s.len() - max
    )
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn needs_shell(cmd: &str) -> bool {
    cmd.contains("&&")
        || cmd.contains("||")
        || cmd.contains('|')
        || cmd.contains('>')
        || cmd.contains('<')
        || cmd.contains(';')
        || cmd.contains('$')
        || cmd.contains('`')
}

/// Run a build command in the project directory and capture the result.
///
/// Simple commands are split on whitespace and executed directly. Commands
/// containing shell operators (`&&`, `|`, etc.) are run through the system
/// shell (`cmd /C` on Windows, `sh -c` on Unix).
///
/// If `output_tx` is provided, stdout/stderr lines are streamed through
/// the channel as they arrive. With a `scratch`, the command gets the
/// run's temp, cache and build variables (see [`RunScratch::vars`]), like
/// the commands the run's tools start; it is not write-confined.
pub async fn run_build_command(
    project_dir: &Path,
    build_command: &str,
    output_tx: Option<UnboundedSender<String>>,
    scratch: Option<&RunScratch>,
) -> Result<BuildResult, BuildRunnerError> {
    run_build_command_with_timeout(
        project_dir,
        build_command,
        output_tx,
        scratch,
        BUILD_TIMEOUT,
    )
    .await
}

/// [`run_build_command`] with a caller-chosen timeout. The process is
/// killed when the timeout expires or the returned future is dropped.
pub async fn run_build_command_with_timeout(
    project_dir: &Path,
    build_command: &str,
    output_tx: Option<UnboundedSender<String>>,
    scratch: Option<&RunScratch>,
    timeout: Duration,
) -> Result<BuildResult, BuildRunnerError> {
    if build_command.split_whitespace().next().is_none() {
        return Err(BuildRunnerError::EmptyCommand);
    }

    info!(
        dir = %project_dir.display(),
        command = %build_command,
        scratch = ?scratch.map(RunScratch::dir),
        "running build verification"
    );

    let mut child = build_child_command(project_dir, build_command, scratch)
        .spawn()
        .map_err(|e| BuildRunnerError::SpawnFailed {
            command: build_command.to_string(),
            cwd: project_dir.display().to_string(),
            source: e,
        })?;

    let (stdout_handle, stderr_handle) = spawn_output_collectors(&mut child, output_tx);

    let result = await_build_result(
        &mut child,
        build_command,
        stdout_handle,
        stderr_handle,
        timeout,
    )
    .await?;

    log_build_result(&result, build_command);
    Ok(result)
}

/// The command that runs `build_command` in `project_dir`, with piped
/// output, killed when dropped, and with `scratch`'s variables if any.
fn build_child_command(
    project_dir: &Path,
    build_command: &str,
    scratch: Option<&RunScratch>,
) -> Command {
    let mut command = if needs_shell(build_command) {
        shell_command(build_command)
    } else {
        direct_command(project_dir, build_command)
    };
    command
        .current_dir(project_dir)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, dir) in scratch.iter().flat_map(|scratch| scratch.vars()) {
        command.env(key, dir);
    }
    command
}

/// `build_command` run by the system shell.
fn shell_command(build_command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        command.as_std_mut().raw_arg(format!("/C {build_command}"));
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new("sh");
        command.args(["-c", build_command]);
        command
    }
}

/// `build_command` split on whitespace and run without a shell.
fn direct_command(project_dir: &Path, build_command: &str) -> Command {
    let mut parts = build_command.split_whitespace();
    let program = resolve_program(parts.next().unwrap_or_default(), project_dir);
    let mut command = Command::new(program);
    command.args(parts);
    command
}

/// The program to start for the bare name `first`.
///
/// On Windows, Rust's `Command::new("npm")` ignores `PATHEXT` so
/// node-ecosystem shims (`npm.cmd`, `yarn.cmd`, `pnpm.cmd`, `vite.cmd`,
/// etc.) fail with `program not found` even when they are clearly on
/// PATH. Resolve the bare name through `which` first (which *does* honour
/// `PATHEXT`) so the build/test gate can run npm scripts on Windows
/// without forcing operators to wrap every command in `cmd /C`.
#[cfg(target_os = "windows")]
fn resolve_program(first: &str, project_dir: &Path) -> std::ffi::OsString {
    windows_resolve_program(first, project_dir)
        .map_or_else(|| first.into(), std::path::PathBuf::into_os_string)
}

/// The program to start for the bare name `first`: the name itself.
#[cfg(not(target_os = "windows"))]
fn resolve_program(first: &str, _project_dir: &Path) -> std::ffi::OsString {
    first.into()
}

/// PATHEXT-aware bare-name resolution used by [`direct_command`] on
/// Windows. See the analogous helper in
/// `aura_exec_tools::fs_tools::cmd` — kept duplicated here to avoid widening
/// `aura-exec-tools`' public surface for what's a small, very local fix.
///
/// Returns `None` for inputs that already look like a path or carry an
/// explicit executable extension; the caller then falls back to passing
/// the original string straight to `Command::new`, which preserves the
/// pre-fix behaviour (and lets `Command::new` produce the canonical OS
/// `not found` error if the file truly doesn't exist).
#[cfg(target_os = "windows")]
fn windows_resolve_program(program: &str, cwd: &Path) -> Option<std::path::PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return None;
    }
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".exe")
        || lower.ends_with(".cmd")
        || lower.ends_with(".bat")
        || lower.ends_with(".com")
        || lower.ends_with(".ps1")
    {
        return None;
    }
    let path_env = std::env::var("PATH").ok();
    match path_env {
        Some(p) => which::which_in(program, Some(p), cwd).ok(),
        None => which::which(program).ok(),
    }
}

fn spawn_output_collectors(
    child: &mut tokio::process::Child,
    output_tx: Option<UnboundedSender<String>>,
) -> (CollectorHandle, CollectorHandle) {
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_tx = output_tx.clone();
    let stdout_handle =
        tokio::spawn(async move { collect_lines("stdout", stdout_pipe, stdout_tx).await });
    let stderr_handle =
        tokio::spawn(async move { collect_lines("stderr", stderr_pipe, output_tx).await });

    (stdout_handle, stderr_handle)
}

async fn collect_lines<R: tokio::io::AsyncRead + Unpin>(
    stream_name: &'static str,
    pipe: Option<R>,
    tx: Option<UnboundedSender<String>>,
) -> Result<String, BuildRunnerError> {
    let mut collected = String::new();
    if let Some(pipe) = pipe {
        let mut reader = tokio::io::BufReader::new(pipe).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| BuildRunnerError::PipeRead {
                stream: stream_name,
                source: e,
            })?
        {
            if let Some(ref tx) = tx {
                let _ = tx.send(format!("{line}\n"));
            }
            collected.push_str(&line);
            collected.push('\n');
        }
    }
    Ok(collected)
}

async fn await_build_result(
    child: &mut tokio::process::Child,
    build_command: &str,
    stdout_handle: CollectorHandle,
    stderr_handle: CollectorHandle,
    timeout: Duration,
) -> Result<BuildResult, BuildRunnerError> {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout_raw = collect_joined_output(stdout_handle, "stdout", build_command).await?;
            let stderr_raw = collect_joined_output(stderr_handle, "stderr", build_command).await?;
            Ok(BuildResult {
                success: status.success(),
                stdout: truncate_output(&stdout_raw, MAX_OUTPUT_BYTES),
                stderr: truncate_output(&stderr_raw, MAX_OUTPUT_BYTES),
                exit_code: status.code(),
                timed_out: false,
            })
        }
        Ok(Err(e)) => Err(BuildRunnerError::Wait {
            command: build_command.to_string(),
            source: e,
        }),
        Err(_) => {
            handle_build_timeout(child, build_command, stdout_handle, stderr_handle, timeout).await
        }
    }
}

async fn collect_joined_output(
    handle: CollectorHandle,
    stream_name: &str,
    build_command: &str,
) -> Result<String, BuildRunnerError> {
    handle.await.map_err(|e| BuildRunnerError::CollectorJoin {
        command: build_command.to_string(),
        stream: stream_name.to_string(),
        source: e,
    })?
}

async fn handle_build_timeout(
    child: &mut tokio::process::Child,
    build_command: &str,
    stdout_handle: CollectorHandle,
    stderr_handle: CollectorHandle,
    timeout: Duration,
) -> Result<BuildResult, BuildRunnerError> {
    warn!(
        command = %build_command,
        timeout_secs = timeout.as_secs(),
        "build command timed out, killing process"
    );
    if let Err(e) = child.kill().await {
        warn!(command = %build_command, error = %e, "failed to kill timed-out build process");
    }
    let partial_stderr = collect_timeout_output(stderr_handle, "stderr", build_command).await;
    let timeout_msg = format!(
        "Build command `{build_command}` timed out after {}s. The command may start a long-running \
         process (e.g. a server). Use `cargo build` or `cargo check` instead of \
         `cargo run` for build verification.",
        timeout.as_secs()
    );
    let stderr = if partial_stderr.is_empty() {
        timeout_msg
    } else {
        format!(
            "{}\n\n{}",
            truncate_output(&partial_stderr, MAX_OUTPUT_BYTES),
            timeout_msg
        )
    };
    Ok(BuildResult {
        success: false,
        stdout: truncate_output(
            &collect_timeout_output(stdout_handle, "stdout", build_command).await,
            MAX_OUTPUT_BYTES,
        ),
        stderr,
        exit_code: None,
        timed_out: true,
    })
}

async fn collect_timeout_output(
    handle: CollectorHandle,
    stream_name: &str,
    build_command: &str,
) -> String {
    // Killing a shell leaves its children (e.g. the `cargo` it started)
    // holding the pipes open, so the collectors may never see EOF.
    match tokio::time::timeout(
        TIMEOUT_COLLECT_GRACE,
        collect_joined_output(handle, stream_name, build_command),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => format!("[failed to collect build command {stream_name}: {e}]\n"),
        Err(_) => String::new(),
    }
}

fn log_build_result(result: &BuildResult, build_command: &str) {
    if result.success {
        info!(command = %build_command, "build verification passed");
    } else {
        // Logged at `info!` rather than `warn!` because a failed build
        // here is *not* a regression on its own — the build-baseline
        // machinery in `agent_loop::tool_pipeline::run_auto_build`
        // and `BuildBaseline::annotate` is what decides whether this
        // failure represents *new* errors versus matching the
        // pre-existing baseline. That layer surfaces real regressions
        // at `warn` level; the runner itself should stay quiet so
        // workspaces with a known-dirty baseline don't spam a `WARN`
        // after every write-tool invocation (observed in harness runs
        // as identical `stderr_len=10212` lines on every tool call).
        info!(
            command = %build_command,
            exit_code = ?result.exit_code,
            stderr_len = result.stderr.len(),
            "build verification returned non-zero \
             (baseline comparison done by agent_loop::run_auto_build)"
        );
    }
}

#[cfg(test)]
mod tests;
