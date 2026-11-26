//! Running a prepared command and reading its result.

use std::io::Write;
use std::process::{Command, Stdio};

use super::IsolationError;

/// Run `command` and return its stdout, or its stderr as the error.
pub(super) fn output(mut command: Command, what: &str) -> Result<Vec<u8>, IsolationError> {
    let out = command.stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(IsolationError::Git(if stderr.is_empty() {
        format!("git {what} exited with {}", out.status)
    } else {
        format!("git {what}: {stderr}")
    }))
}

/// Run `command` with `input` on stdin.
pub(super) fn with_input(
    mut command: Command,
    input: &[u8],
    what: &str,
) -> Result<(), IsolationError> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let out = child.wait_with_output()?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(IsolationError::Git(format!("git {what}: {stderr}")))
}
