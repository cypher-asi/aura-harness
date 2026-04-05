use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use tracing::{debug, instrument};

/// Result of a threshold-based wait operation.
///
/// When a command is run with a sync threshold:
/// - `Completed`: The command finished within the threshold
/// - `Pending`: The command is still running, handle returned for async tracking
pub enum ThresholdResult {
    /// Command completed within the threshold.
    Completed(std::process::Output),
    /// Command is still running after the threshold.
    Pending(std::process::Child),
}

/// Spawn a shell command and return the child process handle.
///
/// This is the low-level spawn operation that doesn't wait for completion.
/// Use this when you need to manage the process lifecycle yourself.
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_spawn(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(std::process::Child, String), ToolError> {
    use std::process::{Command, Stdio};

    let working_dir = match cwd {
        Some(dir) => sandbox.resolve_existing(dir)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?working_dir, arg_count = args.len(), "Spawning command");

    let full_command = if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    };

    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd.exe");
        let trimmed = full_command.trim_matches('"');
        c.args(["/S", "/C", &format!("\"{}\"", trimmed)]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", &full_command]);
        c
    };

    #[cfg(windows)]
    {
        if let Some(fresh_path) = refresh_system_path() {
            cmd.env("PATH", fresh_path);
        }
        cmd.env("PYTHONUTF8", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");
    }

    cmd.current_dir(&working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        ToolError::CommandFailed(format!("Failed to spawn command '{program}': {e}"))
    })?;

    Ok((child, full_command))
}

/// Run a shell command with threshold-based execution.
///
/// This waits for the command to complete up to `sync_threshold_ms`.
/// - If the command completes within the threshold, returns `ThresholdResult::Completed`
/// - If the command is still running after the threshold, returns `ThresholdResult::Pending`
///   with the child handle for async tracking
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
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

/// Run a shell command synchronously with a timeout.
///
/// This is the original synchronous API that waits for completion or kills on timeout.
/// Use `cmd_run_with_threshold` for async-capable execution.
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_run(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    timeout_ms: u64,
) -> Result<ToolResult, ToolError> {
    use std::time::Duration;

    let (child, _full_command) = cmd_spawn(sandbox, program, args, cwd)?;

    let output = match wait_with_hard_timeout(child, Duration::from_millis(timeout_ms)) {
        Ok(out) => out,
        Err(e) => {
            return Err(ToolError::CommandFailed(format!("Command timed out: {e}")));
        }
    };

    output_to_tool_result(output)
}

/// Truncation limits for command output.
const STDOUT_TRUNCATE_LIMIT: usize = 8_000;
/// Truncation limit for stderr.
const STDERR_TRUNCATE_LIMIT: usize = 4_000;

/// Truncate a string to at most `limit` bytes on a char boundary.
fn truncate_output(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (truncated, {limit} char limit)", &s[..end])
}

/// Convert process output to a tool result.
///
/// Returns a *successful* `ToolResult` in all cases (never `Err`) so that
/// downstream command-failure tracking can rely on `ToolResult::ok == false`
/// (`is_error`) rather than on a Rust `Err` variant.
///
/// Stdout is capped at 8 000 chars, stderr at 4 000 chars.
#[allow(clippy::needless_pass_by_value)]
pub fn output_to_tool_result(output: std::process::Output) -> Result<ToolResult, ToolError> {
    let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let stdout = truncate_output(&raw_stdout, STDOUT_TRUNCATE_LIMIT);
    let stderr = truncate_output(&raw_stderr, STDERR_TRUNCATE_LIMIT);

    let exit_code = output.status.code().unwrap_or(-1);

    if output.status.success() {
        let mut result = ToolResult::success("run_command", stdout);
        if !stderr.is_empty() {
            result.stderr = stderr.into_bytes().into();
        }
        result = result.with_metadata("exit_code", "0".to_string());
        Ok(result)
    } else {
        let structured = format!("exit_code: {exit_code}\nstdout:\n{stdout}\nstderr:\n{stderr}");
        let mut result = ToolResult::failure("run_command", structured);
        result.exit_code = Some(exit_code);
        result = result.with_metadata("exit_code", exit_code.to_string());
        Ok(result)
    }
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
    use std::io::Read;
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let stdout = child.stdout.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
            let stderr = child.stderr.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
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

/// Wait for a child process with a hard timeout (kills on timeout).
///
/// This is the original timeout behavior - if the process doesn't complete
/// within the timeout, it is killed and an error is returned.
fn wait_with_hard_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = child.stdout.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
            let stderr = child.stderr.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }

        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Process timed out",
            ));
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Read the current Machine + User PATH from the Windows registry and merge it
/// with the process PATH so that both registry entries (which may have been
/// updated since the harness started) and session-only entries (e.g. Python
/// user-scripts installed via pip) are available to child processes.
#[cfg(windows)]
fn refresh_system_path() -> Option<String> {
    fn read_reg_path(key: &str) -> Option<String> {
        std::process::Command::new("reg")
            .args(["query", key, "/v", "Path"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.contains("REG_"))
                    .and_then(|l| {
                        l.split("REG_EXPAND_SZ")
                            .nth(1)
                            .or_else(|| l.split("REG_SZ").nth(1))
                    })
                    .map(|v| v.trim().to_string())
            })
            .map(|p| expand_env_vars(&p))
    }

    let machine_reg = read_reg_path(
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    let user_reg = read_reg_path(r"HKCU\Environment");

    let process_path = std::env::var("PATH").unwrap_or_default();

    let mut segments: Vec<&str> = Vec::new();

    if let Some(ref m) = machine_reg {
        segments.extend(m.split(';').filter(|s| !s.is_empty()));
    }
    if let Some(ref u) = user_reg {
        segments.extend(u.split(';').filter(|s| !s.is_empty()));
    }
    for entry in process_path.split(';').filter(|s| !s.is_empty()) {
        if !segments.iter().any(|existing| existing.eq_ignore_ascii_case(entry)) {
            segments.push(entry);
        }
    }

    if segments.is_empty() {
        return None;
    }

    Some(segments.join(";"))
}

/// Expand `%VAR%` patterns in a string using the current process environment.
/// Registry PATH values are stored as REG_EXPAND_SZ with variables like
/// `%SystemRoot%`, `%USERPROFILE%`, etc. that must be resolved before use.
#[cfg(windows)]
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(start) = result.find('%') {
        if let Some(end) = result[start + 1..].find('%') {
            let var_name = &result[start + 1..start + 1 + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + 1 + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

/// Validate a command string against the allowlist.
///
/// When the allowlist is non-empty, the command must match at least one entry.
/// Single-token entries match the first token of the command (program name).
/// Multi-token entries (containing whitespace) match as a prefix of the full
/// command, enabling rules like `"start obsidian://"` that restrict both the
/// program and its arguments. Shell metacharacters that could chain additional
/// commands are rejected.
fn check_command_allowlist(command: &str, allowlist: &[String]) -> Result<(), ToolError> {
    if allowlist.is_empty() {
        return Ok(());
    }

    // Block shell metacharacters that allow command chaining
    let dangerous = [";", "&&", "||", "|", "$(", "`", "\n"];
    for meta in &dangerous {
        if command.contains(meta) {
            return Err(ToolError::CommandNotAllowed(format!(
                "shell metacharacter '{meta}' not allowed"
            )));
        }
    }

    let program = command.split_whitespace().next().unwrap_or(command);
    let allowed = allowlist.iter().any(|a| {
        if a.contains(' ') {
            command.starts_with(a.as_str())
        } else {
            a == program
        }
    });
    if !allowed {
        return Err(ToolError::CommandNotAllowed(program.into()));
    }
    Ok(())
}

/// `cmd_run` tool: run a shell command.
///
/// Accepts two invocation styles:
/// - `command` (string): a single shell string, shell-wrapped directly
/// - `program` + `args` (legacy): program name with argument array
///
/// Also accepts `working_dir` as alias for `cwd`, and `timeout_secs` as
/// alternative to `timeout_ms`.
pub struct CmdRunTool;

#[async_trait]
impl Tool for CmdRunTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_command".into(),
            description:
                "Run a shell command. Accepts either 'command' (shell string) or 'program'+'args'."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command string (e.g. 'cargo build --release'). Mutually exclusive with program/args."
                    },
                    "program": {
                        "type": "string",
                        "description": "The program/command to run (legacy, prefer 'command')"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command arguments (used with 'program')"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory (default: workspace root)"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory (alias for 'cwd')"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (default: 30000)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (alternative to timeout_ms)"
                    }
                }
            }),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let cwd = args["cwd"]
            .as_str()
            .or_else(|| args["working_dir"].as_str())
            .map(String::from);

        let timeout_ms = if let Some(secs) = args["timeout_secs"].as_u64() {
            secs * 1000
        } else {
            args["timeout_ms"]
                .as_u64()
                .unwrap_or(ctx.config.sync_threshold_ms)
        };

        if let Some(command) = args["command"].as_str() {
            check_command_allowlist(command, &ctx.config.command_allowlist)?;
            let command = command.to_string();
            let sandbox = ctx.sandbox.clone();
            return super::spawn_blocking_tool(move || {
                cmd_run(&sandbox, &command, &[], cwd.as_deref(), timeout_ms)
            })
            .await;
        }

        let program = args["program"]
            .as_str()
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing 'command' or 'program' argument".into())
            })?
            .to_string();

        check_command_allowlist(&program, &ctx.config.command_allowlist)?;

        let cmd_args: Vec<String> = args["args"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || {
            cmd_run(&sandbox, &program, &cmd_args, cwd.as_deref(), timeout_ms)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
