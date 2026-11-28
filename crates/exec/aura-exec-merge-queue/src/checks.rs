//! The build and test checks an entry must pass before it lands.
//!
//! A check runs as `sh -c <command>` in the queue's worktree, in a process
//! group of its own on Unix. Whenever the check ends, or its future is
//! dropped (the processor shut down, or the entry's own time limit cut
//! it off), a [`GroupGuard`] kills that whole group, so nothing it started
//! (cargo, its rustc and test processes) keeps running in the worktree. Its output
//! goes to a log file rather than memory; only the end of it is read back
//! for a failure's reason.

use std::io::SeekFrom;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::{Child, Command};

/// Lines of a failed check's output kept for its reason.
const TAIL_LINES: usize = 30;
/// Characters of a failed check's output kept for its reason.
const TAIL_CHARS: usize = 4_000;
/// Bytes read back from the end of a check's log.
const TAIL_BYTES: u64 = 16 * 1024;

/// Why a check didn't pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    /// The command that failed
    pub command: String,
    /// What happened, with the end of its output
    pub reason: String,
}

/// Run `command` through `sh -c` in `dir`, within `timeout`, writing its
/// output to `log` (replaced).
///
/// # Errors
/// The command couldn't start, exited non-zero, or ran past `timeout`
/// (its whole process group is killed).
pub async fn run_check(
    dir: &Path,
    command: &str,
    timeout: Duration,
    log: &Path,
) -> Result<(), CheckFailure> {
    let failure = |reason: String| CheckFailure {
        command: command.to_string(),
        reason,
    };
    let mut child = spawn(dir, command, log)
        .await
        .map_err(|e| failure(format!("could not start: {e}")))?;
    // Dropped on every way out of here, a cancelled future included.
    let group = GroupGuard::of(&child);
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(failure(format!("could not wait for it: {e}"))),
        Err(_) => {
            drop(group);
            // Already dead is fine: the group kill got it; this reaps sh.
            let _ = child.kill().await;
            return Err(failure(format!("timed out after {}s", timeout.as_secs())));
        }
    };
    // Whatever the shell left running in the background goes with it.
    drop(group);
    if status.success() {
        return Ok(());
    }
    let code = status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"));
    Err(failure(format!(
        "ended with {code}:\n{}",
        tail(&read_tail(log).await)
    )))
}

async fn spawn(dir: &Path, command: &str, log: &Path) -> std::io::Result<Child> {
    let out = tokio::fs::File::create(log).await?.into_std().await;
    let err = out.try_clone()?;
    let mut child = Command::new("sh");
    child
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        // A stale snapshot fails the check instead of writing a new one
        // (`.snap.new`) and passing.
        .env("INSTA_UPDATE", "no")
        .env("CI", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .kill_on_drop(true);
    #[cfg(unix)]
    child.process_group(0);
    child.spawn()
}

/// Kills a check's process group when dropped. `kill_on_drop` alone only
/// kills the shell, not what it started.
pub(crate) struct GroupGuard {
    #[cfg(unix)]
    group: Option<rustix::process::Pid>,
}

impl GroupGuard {
    /// The guard of `child`'s group (it leads one: `process_group(0)`).
    /// Without unix process groups there is nothing to guard, and `child`
    /// goes unread.
    #[cfg_attr(not(unix), allow(unused_variables))]
    pub(crate) fn of(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw),
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // ESRCH: the group is gone already, which is the point.
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }
}

/// The last [`TAIL_BYTES`] of `log`, empty when it can't be read.
async fn read_tail(log: &Path) -> String {
    let read = async {
        let mut file = tokio::fs::File::open(log).await?;
        let len = file.metadata().await?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
            .await?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&bytes).into_owned())
    };
    read.await.unwrap_or_default()
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

    async fn check(
        command: &str,
        timeout: Duration,
    ) -> (Result<(), CheckFailure>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("check.log");
        (run_check(dir.path(), command, timeout, &log).await, dir)
    }

    #[tokio::test]
    async fn a_passing_check_is_ok() {
        assert_eq!(check("true", Duration::from_secs(10)).await.0, Ok(()));
    }

    #[tokio::test]
    async fn a_failing_check_says_how_it_ended_with_its_output() {
        let failure = check("echo boom; echo bang >&2; exit 3", Duration::from_secs(10))
            .await
            .0
            .unwrap_err();
        assert!(failure.reason.contains("exit 3"), "{}", failure.reason);
        assert!(failure.reason.contains("boom"), "{}", failure.reason);
        assert!(failure.reason.contains("bang"), "{}", failure.reason);
    }

    #[tokio::test]
    async fn a_check_running_too_long_times_out() {
        let failure = check("sleep 5", Duration::from_millis(100))
            .await
            .0
            .unwrap_err();
        assert!(failure.reason.contains("timed out"), "{}", failure.reason);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_what_the_check_started_too() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-running");
        // The shell starts a child that would write the marker later.
        let command = format!("(sleep 1; touch {}) & wait", marker.display());
        let log = dir.path().join("check.log");
        let failed = run_check(dir.path(), &command, Duration::from_millis(100), &log).await;
        assert!(failed.is_err());
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(!marker.exists(), "the check's child outlived its timeout");
    }

    #[tokio::test]
    async fn only_the_end_of_a_long_output_is_kept() {
        let failure = check("seq 1 200000; exit 1", Duration::from_secs(30))
            .await
            .0
            .unwrap_err();
        assert!(failure.reason.ends_with("200000"), "{}", failure.reason);
        assert!(failure.reason.len() < TAIL_CHARS + 100);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_check_whose_future_is_dropped_takes_its_processes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-running");
        let command = format!("(sleep 1; touch {}) & wait", marker.display());
        let log = dir.path().join("check.log");
        // Like a shutdown or the entry's time limit: the check's future is
        // dropped while it runs, its own timeout far away.
        let cut = tokio::time::timeout(
            Duration::from_millis(100),
            run_check(dir.path(), &command, Duration::from_secs(60), &log),
        )
        .await;
        assert!(cut.is_err(), "the check was cut off");
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            !marker.exists(),
            "the check's child outlived its dropped future"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn what_a_passing_check_left_in_the_background_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-running");
        let command = format!("(sleep 1; touch {}) >/dev/null 2>&1 &", marker.display());
        let log = dir.path().join("check.log");
        let passed = run_check(dir.path(), &command, Duration::from_secs(10), &log).await;
        assert_eq!(passed, Ok(()));
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(!marker.exists(), "a background child outlived its check");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let kept = tail(&text);
        assert!(kept.starts_with("line 70"));
        assert!(kept.ends_with("line 99"));
    }
}
