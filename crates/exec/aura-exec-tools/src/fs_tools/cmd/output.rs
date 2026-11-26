//! Turning a finished process's output into a `ToolResult`: truncation
//! and the `cargo` error summary attached as metadata.

use crate::error::ToolError;
use aura_core_types::ToolResult;

/// Truncation limits for command output.
pub(super) const STDOUT_TRUNCATE_LIMIT: usize = 8_000;
/// Truncation limit for stderr.
const STDERR_TRUNCATE_LIMIT: usize = 4_000;

/// Truncate a string to about `limit` bytes on char boundaries, keeping
/// its start and, with the larger share, its end: errors, failing tests
/// and summaries are printed last.
pub(super) fn truncate_output(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut head = limit * 3 / 8;
    while head > 0 && !s.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = s.len() - (limit - head);
    while tail < s.len() && !s.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n... (truncated: {} bytes cut from the middle) ...\n{}",
        &s[..head],
        tail - head,
        &s[tail..]
    )
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
    output_to_tool_result_with_program(output, None, &[])
}

/// Variant that also receives the spawned program / args so it can run
/// language-specific output classifiers (today: `cargo check|build|
/// test|clippy` stderr → structured error metadata so the agent loop
/// sees "compiler errors detected" even when stdout was empty).
///
/// Returns `Result` for symmetry with [`output_to_tool_result`] and so
/// the call site in `cmd_run` can use the `?` operator alongside the
/// other fallible spawn / wait helpers without introducing an awkward
/// `Ok(...)` wrap.
#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
pub(crate) fn output_to_tool_result_with_program(
    output: std::process::Output,
    program: Option<&str>,
    cmd_args: &[String],
) -> Result<ToolResult, ToolError> {
    let raw_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let stdout = truncate_output(&raw_stdout, STDOUT_TRUNCATE_LIMIT);
    let stderr = truncate_output(&raw_stderr, STDERR_TRUNCATE_LIMIT);

    let exit_code = output.status.code().unwrap_or(-1);
    let cargo_diagnostics = classify_cargo_invocation(program, cmd_args)
        .map(|_subcommand| extract_cargo_errors(&raw_stderr))
        .unwrap_or_default();

    if output.status.success() {
        let mut result = ToolResult::success("run_command", stdout);
        if !stderr.is_empty() {
            result.stderr = stderr.into_bytes().into();
        }
        result = result.with_metadata("exit_code", "0".to_string());
        result = attach_cargo_error_metadata(result, &cargo_diagnostics);
        Ok(result)
    } else {
        let structured = format!("exit_code: {exit_code}\nstdout:\n{stdout}\nstderr:\n{stderr}");
        let mut result = ToolResult::failure("run_command", structured);
        result.exit_code = Some(exit_code);
        result = result.with_metadata("exit_code", exit_code.to_string());
        result = attach_cargo_error_metadata(result, &cargo_diagnostics);
        Ok(result)
    }
}

/// Structured pull-out of one `error[Eddd]: …` block from `cargo`
/// stderr. Populated by [`extract_cargo_errors`] from the
/// `--message-format=short` / human stderr format. We intentionally
/// keep the parser line-oriented and cheap — the goal is just to
/// surface the FIRST few diagnostics to the agent loop, not to replace
/// `cargo`'s JSON output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CargoError {
    pub code: String,
    pub message: String,
    pub location: Option<String>,
}

/// Heuristic: return `Some(subcommand)` when the spawned program looks
/// like a Rust build verb whose stderr is worth parsing. `cargo check`,
/// `cargo build`, `cargo test`, `cargo clippy` and `cargo run` all
/// emit the same `error[Exxxx]:` format.
pub(super) fn classify_cargo_invocation(
    program: Option<&str>,
    args: &[String],
) -> Option<&'static str> {
    let program_name = program?.rsplit(['/', '\\']).next()?;
    let stem = program_name
        .strip_suffix(".exe")
        .or_else(|| program_name.strip_suffix(".EXE"))
        .unwrap_or(program_name);
    if !stem.eq_ignore_ascii_case("cargo") {
        return None;
    }
    let subcommand = args.iter().find(|a| !a.starts_with('+'))?;
    match subcommand.as_str() {
        "check" => Some("check"),
        "build" => Some("build"),
        "test" => Some("test"),
        "clippy" => Some("clippy"),
        "run" => Some("run"),
        _ => None,
    }
}

/// Scan `cargo` stderr for `error[Exxxx]: <message>` blocks and the
/// `--> file:line:col` line that follows them. Returns at most
/// [`MAX_CAPTURED_ERRORS`] entries to keep the metadata blob small;
/// the agent only needs the first few to know it has work to do.
pub(super) fn extract_cargo_errors(stderr: &str) -> Vec<CargoError> {
    const MAX_CAPTURED_ERRORS: usize = 5;
    let mut out: Vec<CargoError> = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();
    let mut idx = 0;
    while idx < lines.len() && out.len() < MAX_CAPTURED_ERRORS {
        let line = lines[idx].trim_start();
        if let Some(remainder) = line.strip_prefix("error[") {
            if let Some(close) = remainder.find(']') {
                let code = remainder[..close].to_string();
                let after_code = remainder[close + 1..].trim_start();
                let message = after_code
                    .strip_prefix(':')
                    .map_or_else(|| after_code.to_string(), |s| s.trim().to_string());
                // Look ahead at the next non-blank line for the `-->`
                // file location cargo prints under each diagnostic.
                let mut location = None;
                for follow in lines.iter().skip(idx + 1).take(3) {
                    let f = follow.trim_start();
                    if let Some(loc) = f.strip_prefix("--> ") {
                        location = Some(loc.trim().to_string());
                        break;
                    }
                }
                out.push(CargoError {
                    code,
                    message,
                    location,
                });
            }
        }
        idx += 1;
    }
    out
}

/// Attach a compact `cargo_errors` metadata blob plus a human-readable
/// `compiler_errors` count when [`extract_cargo_errors`] surfaced any
/// diagnostics. `cargo_errors` is JSON so downstream consumers can
/// machine-parse it without re-running the regex.
fn attach_cargo_error_metadata(mut result: ToolResult, errors: &[CargoError]) -> ToolResult {
    if errors.is_empty() {
        return result;
    }
    let json: Vec<serde_json::Value> = errors
        .iter()
        .map(|err| {
            serde_json::json!({
                "code": err.code,
                "message": err.message,
                "location": err.location,
            })
        })
        .collect();
    if let Ok(serialised) = serde_json::to_string(&json) {
        result = result.with_metadata("cargo_errors", serialised);
    }
    result = result.with_metadata("compiler_errors", errors.len().to_string());
    result
}
