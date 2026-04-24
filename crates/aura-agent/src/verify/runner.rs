//! Build command execution and test output parsing.
//!
//! Provides [`run_build_command`] for executing shell commands with streaming
//! output, timeout handling, and output truncation. Also includes parsers for
//! cargo test, Jest, and generic test output formats.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndividualTestResult {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const MAX_OUTPUT_BYTES: usize = 12_000;
const BUILD_TIMEOUT: Duration = Duration::from_secs(120);

fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let start = &s[..half];
    let end = &s[s.len() - half..];
    format!(
        "{start}\n\n... (truncated {0} bytes) ...\n\n{end}",
        s.len() - max
    )
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
/// the channel as they arrive.
pub async fn run_build_command(
    project_dir: &Path,
    build_command: &str,
    output_tx: Option<UnboundedSender<String>>,
) -> anyhow::Result<BuildResult> {
    if build_command.split_whitespace().next().is_none() {
        anyhow::bail!("build_command is empty");
    }

    info!(
        dir = %project_dir.display(),
        command = %build_command,
        "running build verification"
    );

    let mut child = spawn_build_child(project_dir, build_command)?;

    let (stdout_handle, stderr_handle) = spawn_output_collectors(&mut child, output_tx);

    let result =
        await_build_result(&mut child, build_command, stdout_handle, stderr_handle).await?;

    log_build_result(&result, build_command);
    Ok(result)
}

fn spawn_build_child(
    project_dir: &Path,
    build_command: &str,
) -> anyhow::Result<tokio::process::Child> {
    let child = if needs_shell(build_command) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.as_std_mut().raw_arg(format!("/C {build_command}"));
            c.current_dir(project_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("sh")
                .args(["-c", build_command])
                .current_dir(project_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        }
    } else {
        let parts: Vec<&str> = build_command.split_whitespace().collect();
        Command::new(parts[0])
            .args(&parts[1..])
            .current_dir(project_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    }
    .map_err(|e| anyhow::anyhow!("failed to execute build command `{build_command}`: {e}"))?;
    Ok(child)
}

fn spawn_output_collectors(
    child: &mut tokio::process::Child,
    output_tx: Option<UnboundedSender<String>>,
) -> (
    tokio::task::JoinHandle<String>,
    tokio::task::JoinHandle<String>,
) {
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_tx = output_tx.clone();
    let stdout_handle = tokio::spawn(async move { collect_lines(stdout_pipe, stdout_tx).await });
    let stderr_handle = tokio::spawn(async move { collect_lines(stderr_pipe, output_tx).await });

    (stdout_handle, stderr_handle)
}

async fn collect_lines<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    tx: Option<UnboundedSender<String>>,
) -> String {
    let mut collected = String::new();
    if let Some(pipe) = pipe {
        let mut reader = tokio::io::BufReader::new(pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(ref tx) = tx {
                let _ = tx.send(format!("{line}\n"));
            }
            collected.push_str(&line);
            collected.push('\n');
        }
    }
    collected
}

async fn await_build_result(
    child: &mut tokio::process::Child,
    build_command: &str,
    stdout_handle: tokio::task::JoinHandle<String>,
    stderr_handle: tokio::task::JoinHandle<String>,
) -> anyhow::Result<BuildResult> {
    match tokio::time::timeout(BUILD_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout_raw = stdout_handle.await.unwrap_or_default();
            let stderr_raw = stderr_handle.await.unwrap_or_default();
            Ok(BuildResult {
                success: status.success(),
                stdout: truncate_output(&stdout_raw, MAX_OUTPUT_BYTES),
                stderr: truncate_output(&stderr_raw, MAX_OUTPUT_BYTES),
                exit_code: status.code(),
                timed_out: false,
            })
        }
        Ok(Err(e)) => {
            anyhow::bail!("IO error waiting for build command `{build_command}`: {e}");
        }
        Err(_) => handle_build_timeout(child, build_command, stdout_handle, stderr_handle).await,
    }
}

async fn handle_build_timeout(
    child: &mut tokio::process::Child,
    build_command: &str,
    stdout_handle: tokio::task::JoinHandle<String>,
    stderr_handle: tokio::task::JoinHandle<String>,
) -> anyhow::Result<BuildResult> {
    warn!(
        command = %build_command,
        timeout_secs = BUILD_TIMEOUT.as_secs(),
        "build command timed out, killing process"
    );
    if let Err(e) = child.kill().await {
        warn!(command = %build_command, error = %e, "failed to kill timed-out build process");
    }
    let partial_stderr = stderr_handle.await.unwrap_or_default();
    let timeout_msg = format!(
        "Build command timed out after {}s. The command may start a long-running \
         process (e.g. a server). Use `cargo build` or `cargo check` instead of \
         `cargo run` for build verification.",
        BUILD_TIMEOUT.as_secs()
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
        stdout: stdout_handle.await.unwrap_or_default(),
        stderr,
        exit_code: None,
        timed_out: true,
    })
}

fn log_build_result(result: &BuildResult, build_command: &str) {
    if result.success {
        info!(command = %build_command, "build verification passed");
    } else {
        // Logged at `info!` rather than `warn!` because a failed build
        // here is *not* a regression on its own — the build-baseline
        // machinery in `agent_loop::tool_processing::run_auto_build`
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

/// Parse test runner output into individual test results and a summary line.
///
/// Supports cargo test and Jest/npm test formats. Falls back to a single
/// aggregate result derived from the exit code when the format is unrecognised.
pub fn parse_test_output(
    stdout: &str,
    stderr: &str,
    success: bool,
) -> (Vec<IndividualTestResult>, String) {
    let combined = format!("{stdout}\n{stderr}");

    let cargo_results = parse_cargo_test(&combined);
    if !cargo_results.is_empty() {
        let passed = cargo_results
            .iter()
            .filter(|r| r.status == "passed")
            .count();
        let failed = cargo_results
            .iter()
            .filter(|r| r.status == "failed")
            .count();
        let ignored = cargo_results
            .iter()
            .filter(|r| r.status == "skipped")
            .count();
        let summary = format!("{passed} passed, {failed} failed, {ignored} ignored");
        return (cargo_results, summary);
    }

    let jest_results = parse_jest_output(&combined);
    if !jest_results.is_empty() {
        let passed = jest_results.iter().filter(|r| r.status == "passed").count();
        let failed = jest_results.iter().filter(|r| r.status == "failed").count();
        let summary = format!("{passed} passed, {failed} failed");
        return (jest_results, summary);
    }

    let status = if success { "passed" } else { "failed" };
    let summary = if success {
        "all tests passed".to_string()
    } else {
        "tests failed".to_string()
    };
    let result = IndividualTestResult {
        name: "(aggregate)".to_string(),
        status: status.to_string(),
        message: if success {
            None
        } else {
            Some(truncate_output(&combined, 2000))
        },
    };
    (vec![result], summary)
}

fn parse_cargo_test(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("test ") {
            continue;
        }
        let rest = &trimmed[5..];
        if let Some(idx) = rest.find(" ... ") {
            let name = rest[..idx].trim().to_string();
            let outcome = rest[idx + 5..].trim();
            let status = match outcome {
                "ok" => "passed",
                "FAILED" => "failed",
                s if s.starts_with("ignored") => "skipped",
                _ => continue,
            };
            let message = if status == "failed" {
                Some(outcome.to_string())
            } else {
                None
            };
            results.push(IndividualTestResult {
                name,
                status: status.to_string(),
                message,
            });
        }
    }
    results
}

fn parse_jest_output(output: &str) -> Vec<IndividualTestResult> {
    let mut results = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("PASS ") {
            results.push(IndividualTestResult {
                name: rest.trim().to_string(),
                status: "passed".to_string(),
                message: None,
            });
        } else if let Some(rest) = trimmed.strip_prefix("FAIL ") {
            results.push(IndividualTestResult {
                name: rest.trim().to_string(),
                status: "failed".to_string(),
                message: None,
            });
        } else if trimmed.starts_with("\u{2713} ") || trimmed.starts_with("✓ ") {
            results.push(IndividualTestResult {
                name: trimmed[2..].trim().to_string(),
                status: "passed".to_string(),
                message: None,
            });
        } else if trimmed.starts_with("\u{2717} ")
            || trimmed.starts_with("✕ ")
            || trimmed.starts_with("✗ ")
        {
            results.push(IndividualTestResult {
                name: trimmed[3..].trim().to_string(),
                status: "failed".to_string(),
                message: None,
            });
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cargo_test_output() {
        let stdout = "\
running 3 tests
test utils::tests::test_parse ... ok
test utils::tests::test_format ... FAILED
test utils::tests::test_skip ... ignored
";
        let (results, summary) = parse_test_output(stdout, "", true);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, "passed");
        assert_eq!(results[1].status, "failed");
        assert_eq!(results[2].status, "skipped");
        assert!(summary.contains("1 passed"));
        assert!(summary.contains("1 failed"));
        assert!(summary.contains("1 ignored"));
    }

    #[test]
    fn parse_jest_pass_fail() {
        let stdout = "\
PASS src/utils.test.ts
FAIL src/api.test.ts
PASS src/hooks.test.ts
";
        let (results, summary) = parse_test_output(stdout, "", true);
        assert_eq!(results.len(), 3);
        assert_eq!(results.iter().filter(|r| r.status == "passed").count(), 2);
        assert_eq!(results.iter().filter(|r| r.status == "failed").count(), 1);
        assert!(summary.contains("2 passed"));
    }

    #[test]
    fn parse_fallback_success() {
        let (results, summary) = parse_test_output("all ok", "", true);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "passed");
        assert!(summary.contains("all tests passed"));
    }

    #[test]
    fn parse_fallback_failure() {
        let (results, summary) = parse_test_output("boom", "something went wrong", false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "failed");
        assert!(results[0].message.is_some());
        assert!(summary.contains("tests failed"));
    }

    #[test]
    fn truncate_short_output_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_long_output() {
        let long = "a".repeat(200);
        let result = truncate_output(&long, 50);
        assert!(result.len() < 200);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn needs_shell_with_pipe() {
        assert!(needs_shell("cargo test | head"));
    }

    #[test]
    fn needs_shell_with_and() {
        assert!(needs_shell("cd foo && npm build"));
    }

    #[test]
    fn needs_shell_simple_command() {
        assert!(!needs_shell("cargo build --release"));
    }
}
