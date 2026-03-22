use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolResult;
use aura_reasoner::ToolDefinition;
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

    debug!(?working_dir, ?args, "Spawning command");

    // Build the full command string
    let full_command = if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    };

    // Use shell to run the command for better compatibility
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd.exe");
        c.args(["/C", &full_command]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", &full_command]);
        c
    };

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

    // Wait with threshold
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

    // Wait with hard timeout (kills on timeout)
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
    // Walk backward to find a char boundary
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
        let mut result = ToolResult::success("cmd_run", stdout);
        if !stderr.is_empty() {
            result.stderr = stderr.into_bytes().into();
        }
        result = result.with_metadata("exit_code", "0".to_string());
        Ok(result)
    } else {
        let structured = format!("exit_code: {exit_code}\nstdout:\n{stdout}\nstderr:\n{stderr}");
        let mut result = ToolResult::failure("cmd_run", structured);
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
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process finished, collect output
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
            }
            Ok(None) => {
                // Process still running
                if start.elapsed() > threshold {
                    // Threshold exceeded - return the child for async tracking
                    // Do NOT kill the process
                    return ThresholdResult::Pending(child);
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                // Error checking status - return child for caller to handle
                if start.elapsed() > threshold {
                    return ThresholdResult::Pending(child);
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
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
            // Process finished, collect output
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
            // Kill the process
            let _ = child.kill();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Process timed out",
            ));
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Validate a command string against the allowlist.
///
/// When the allowlist is non-empty, the first whitespace-delimited token
/// of the command string must appear in the list.
fn check_command_allowlist(command: &str, allowlist: &[String]) -> Result<(), ToolError> {
    if allowlist.is_empty() {
        return Ok(());
    }
    let program = command.split_whitespace().next().unwrap_or(command);
    if !allowlist.iter().any(|a| a == program) {
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
        "cmd_run"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cmd_run".into(),
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

        // "command" mode: single shell string, shell-wrapped directly
        if let Some(command) = args["command"].as_str() {
            check_command_allowlist(command, &ctx.config.command_allowlist)?;
            let command = command.to_string();
            let sandbox = ctx.sandbox.clone();
            return super::spawn_blocking_tool(move || {
                cmd_run(&sandbox, &command, &[], cwd.as_deref(), timeout_ms)
            })
            .await;
        }

        // "program" + "args" mode (legacy)
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
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_sandbox() -> (Sandbox, TempDir) {
        let dir = TempDir::new().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        (sandbox, dir)
    }

    // ========================================================================
    // cmd_run Tests
    // ========================================================================

    #[test]
    fn test_cmd_run_echo() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = cmd_run(&sandbox, "echo", &["hello".to_string()], None, 5000).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("hello"));
    }

    #[test]
    fn test_cmd_run_in_cwd() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/marker.txt"), "found").unwrap();

        #[cfg(windows)]
        let result = cmd_run(&sandbox, "dir", &[], Some("subdir"), 5000).unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "ls", &[], Some("subdir"), 5000).unwrap();

        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("marker"));
    }

    #[test]
    fn test_cmd_run_nonexistent_command() {
        let (sandbox, _dir) = create_test_sandbox();

        // Non-zero exit from a command that doesn't exist is now Ok with is_error
        let result = cmd_run(
            &sandbox,
            "nonexistent_command_that_does_not_exist_xyz",
            &[],
            None,
            5000,
        );
        // On some platforms this is a spawn error (Err), on others the shell
        // returns non-zero (Ok with !ok). Accept either.
        match result {
            Err(ToolError::CommandFailed(_)) => {}
            Ok(r) => assert!(!r.ok),
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_cmd_run_exit_code() {
        let (sandbox, _dir) = create_test_sandbox();

        #[cfg(windows)]
        let result = cmd_run(
            &sandbox,
            "cmd",
            &["/c".to_string(), "exit".to_string(), "1".to_string()],
            None,
            5000,
        )
        .unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "false", &[], None, 5000).unwrap();

        // Non-zero exit now returns Ok with is_error=true
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(1));
    }

    #[test]
    fn test_cmd_run_failure_returns_structured_result() {
        let (sandbox, _dir) = create_test_sandbox();

        #[cfg(windows)]
        let result = cmd_run(
            &sandbox,
            "cmd",
            &["/c".to_string(), "exit".to_string(), "42".to_string()],
            None,
            5000,
        )
        .unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "sh", &["-c".into(), "exit 42".into()], None, 5000).unwrap();

        assert!(!result.ok);
        let stderr_text = String::from_utf8_lossy(&result.stderr);
        assert!(stderr_text.contains("exit_code:"));
    }

    #[test]
    fn test_cmd_run_stdout_truncation() {
        let (sandbox, _dir) = create_test_sandbox();

        // Generate output larger than 8000 chars
        #[cfg(windows)]
        let result = cmd_run(
            &sandbox,
            "powershell",
            &["-Command".to_string(), "'x' * 10000".to_string()],
            None,
            10000,
        )
        .unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(
            &sandbox,
            "python3",
            &["-c".into(), "print('x' * 10000)".into()],
            None,
            10000,
        )
        .unwrap();

        assert!(result.ok);
        let stdout_text = String::from_utf8_lossy(&result.stdout);
        // Output should be truncated to ~8000 chars
        assert!(stdout_text.len() <= STDOUT_TRUNCATE_LIMIT + 100);
    }

    #[test]
    fn test_cmd_run_with_args() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("test.txt"), "content").unwrap();

        #[cfg(windows)]
        let result = cmd_run(&sandbox, "type", &["test.txt".to_string()], None, 5000).unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "cat", &["test.txt".to_string()], None, 5000).unwrap();

        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("content"));
    }

    // ========================================================================
    // wait_with_threshold Tests
    // ========================================================================

    #[test]
    fn test_fast_command_returns_output() {
        let (sandbox, _dir) = create_test_sandbox();

        let (result, _command) =
            cmd_run_with_threshold(&sandbox, "echo", &["fast_output".to_string()], None, 5000)
                .unwrap();

        match result {
            ThresholdResult::Completed(output) => {
                assert!(output.status.success());
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("fast_output"));
            }
            ThresholdResult::Pending(_) => {
                panic!("Expected Completed, got Pending for fast command");
            }
        }
    }

    #[test]
    fn test_slow_command_returns_child() {
        let (sandbox, _dir) = create_test_sandbox();

        #[cfg(windows)]
        let (result, command) = cmd_run_with_threshold(
            &sandbox,
            "ping",
            &["-n".to_string(), "10".to_string(), "127.0.0.1".to_string()],
            None,
            100,
        )
        .unwrap();

        #[cfg(not(windows))]
        let (result, command) =
            cmd_run_with_threshold(&sandbox, "sleep", &["10".to_string()], None, 100).unwrap();

        match result {
            ThresholdResult::Pending(mut child) => {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "Child should still be running"
                );
                let _ = child.kill();
                let _ = child.wait();
                assert!(!command.is_empty());
            }
            ThresholdResult::Completed(_) => {
                panic!("Expected Pending, got Completed for slow command");
            }
        }
    }

    #[test]
    fn test_threshold_boundary_fast_completes() {
        let (sandbox, _dir) = create_test_sandbox();

        let (result, _command) =
            cmd_run_with_threshold(&sandbox, "echo", &["boundary".to_string()], None, 1000)
                .unwrap();

        match result {
            ThresholdResult::Completed(output) => {
                assert!(output.status.success());
            }
            ThresholdResult::Pending(_) => {
                panic!("Expected Completed for fast echo command");
            }
        }
    }

    #[test]
    fn test_cmd_spawn_returns_command_string() {
        let (sandbox, _dir) = create_test_sandbox();

        let (mut child, command) =
            cmd_spawn(&sandbox, "echo", &["test_arg".to_string()], None).unwrap();

        assert!(command.contains("echo"));
        assert!(command.contains("test_arg"));

        let _ = child.wait();
    }

    #[test]
    fn test_output_to_tool_result_success() {
        #[cfg(windows)]
        let status = {
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "exit 0"])
                .output()
                .unwrap();
            output.status
        };

        #[cfg(not(windows))]
        let status = {
            let output = std::process::Command::new("true").output().unwrap();
            output.status
        };

        let output = std::process::Output {
            status,
            stdout: b"success output".to_vec(),
            stderr: Vec::new(),
        };

        let result = output_to_tool_result(output).unwrap();
        assert!(result.ok);
        assert_eq!(String::from_utf8_lossy(&result.stdout), "success output");
    }

    #[test]
    fn test_output_to_tool_result_failure() {
        #[cfg(windows)]
        let status = {
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "exit 1"])
                .output()
                .unwrap();
            output.status
        };

        #[cfg(not(windows))]
        let status = {
            let output = std::process::Command::new("false").output().unwrap();
            output.status
        };

        let output = std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: b"error message".to_vec(),
        };

        // Now returns Ok with is_error=true instead of Err
        let result = output_to_tool_result(output).unwrap();
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(1));
        let stderr_text = String::from_utf8_lossy(&result.stderr);
        assert!(stderr_text.contains("error message"));
    }

    // ========================================================================
    // truncate_output boundary tests
    // ========================================================================

    #[test]
    fn test_truncate_output_under_limit() {
        let s = "short";
        let result = truncate_output(s, 100);
        assert_eq!(result, "short");
    }

    #[test]
    fn test_truncate_output_exact_limit() {
        let s = "x".repeat(STDOUT_TRUNCATE_LIMIT);
        let result = truncate_output(&s, STDOUT_TRUNCATE_LIMIT);
        assert_eq!(result.len(), STDOUT_TRUNCATE_LIMIT);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn test_truncate_output_over_limit() {
        let s = "x".repeat(STDOUT_TRUNCATE_LIMIT + 500);
        let result = truncate_output(&s, STDOUT_TRUNCATE_LIMIT);
        assert!(result.contains("truncated"));
        assert!(result.len() <= STDOUT_TRUNCATE_LIMIT + 100);
    }

    #[test]
    fn test_truncate_output_multibyte_boundary() {
        // 3-byte UTF-8 char: €
        let s = "€".repeat(4000);
        let result = truncate_output(&s, 10);
        assert!(result.is_char_boundary(result.find('\n').unwrap_or(result.len())));
    }

    #[test]
    fn test_truncate_output_empty() {
        let result = truncate_output("", 100);
        assert_eq!(result, "");
    }

    // ========================================================================
    // check_command_allowlist tests
    // ========================================================================

    #[test]
    fn test_command_allowlist_empty_allows_all() {
        assert!(check_command_allowlist("anything", &[]).is_ok());
    }

    #[test]
    fn test_command_allowlist_blocks_unlisted() {
        let allowlist = vec!["echo".to_string(), "ls".to_string()];
        let result = check_command_allowlist("rm -rf /", &allowlist);
        assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
    }

    #[test]
    fn test_command_allowlist_allows_listed() {
        let allowlist = vec!["echo".to_string(), "ls".to_string()];
        assert!(check_command_allowlist("echo hello", &allowlist).is_ok());
        assert!(check_command_allowlist("ls -la", &allowlist).is_ok());
    }

    #[test]
    fn test_command_allowlist_extracts_first_token() {
        let allowlist = vec!["cargo".to_string()];
        assert!(check_command_allowlist("cargo build --release", &allowlist).is_ok());
    }

    #[test]
    fn test_output_to_tool_result_exit_code_metadata() {
        #[cfg(windows)]
        let status = {
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "exit 0"])
                .output()
                .unwrap();
            output.status
        };

        #[cfg(not(windows))]
        let status = {
            let output = std::process::Command::new("true").output().unwrap();
            output.status
        };

        let output = std::process::Output {
            status,
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
        };

        let result = output_to_tool_result(output).unwrap();
        assert_eq!(result.metadata.get("exit_code").unwrap(), "0");
    }
}
