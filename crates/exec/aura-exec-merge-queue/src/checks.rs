//! The build and test checks an entry must pass before it lands.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Lines of a failed check's output kept for its reason.
const TAIL_LINES: usize = 30;
/// Characters of a failed check's output kept for its reason.
const TAIL_CHARS: usize = 4_000;

/// Why a check didn't pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    /// The command that failed
    pub command: String,
    /// What happened, with the end of its output
    pub reason: String,
}

/// Run `command` through `sh -c` in `dir`, within `timeout`.
///
/// # Errors
/// The command couldn't start, exited non-zero, or ran past `timeout`
/// (it is killed).
pub async fn run_check(dir: &Path, command: &str, timeout: Duration) -> Result<(), CheckFailure> {
    let failure = |reason: String| CheckFailure {
        command: command.to_string(),
        reason,
    };
    let mut child = Command::new("sh");
    child
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, child.output()).await {
        Err(_) => return Err(failure(format!("timed out after {}s", timeout.as_secs()))),
        Ok(Err(e)) => return Err(failure(format!("could not start: {e}"))),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let code = output
        .status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"));
    Err(failure(format!("ended with {code}:\n{}", tail(&text))))
}

/// The last [`TAIL_LINES`] lines of `text`, at most [`TAIL_CHARS`]
/// characters of them.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    let kept = lines.get(start..).unwrap_or_default().join("\n");
    let skip = kept.chars().count().saturating_sub(TAIL_CHARS);
    kept.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_passing_check_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            run_check(dir.path(), "true", Duration::from_secs(10)).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn a_failing_check_says_how_it_ended_with_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let failure = run_check(dir.path(), "echo boom; exit 3", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(failure.reason.contains("exit 3"), "{}", failure.reason);
        assert!(failure.reason.contains("boom"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_check_running_too_long_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let failure = run_check(dir.path(), "sleep 5", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(failure.reason.contains("timed out"), "{}", failure.reason);
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let kept = tail(&text);
        assert!(kept.starts_with("line 70"));
        assert!(kept.ends_with("line 99"));
    }
}
