//! Foreground commands the user can stop while they run.
//!
//! A command a tool call runs in the foreground is registered here, under
//! the agent that made the call, until it exits. [`stop_commands`] asks
//! its process group to end; the call then returns what the command
//! printed so far with [`STOPPED_NOTE`], and the agent's turn goes on.

use super::ProcessTags;
use aura_core_types::ToolResult;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How long a stopped command has to exit on `SIGTERM` before its wait
/// loop kills it.
pub(super) const STOP_GRACE: Duration = Duration::from_secs(2);

/// What the model is told about a command the user stopped.
const STOPPED_NOTE: &str = "[STOPPED] The user stopped this command before it finished. \
     The output above is what it printed until then.";

struct Running {
    agent_id: String,
    /// Leader of the command's process group.
    pid: u32,
    stopped_at: Option<Instant>,
}

fn running() -> MutexGuard<'static, HashMap<u64, Running>> {
    static RUNNING: OnceLock<Mutex<HashMap<u64, Running>>> = OnceLock::new();
    RUNNING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// A running command's entry, removed when dropped.
pub(super) struct Registration(Option<u64>);

impl Registration {
    /// Register the command whose process group `pid` leads, run by the
    /// call `tags` describes. A command with no agent can't be stopped.
    pub(super) fn new(tags: &ProcessTags, pid: Option<u32>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (Some(agent_id), Some(pid)) = (&tags.agent_id, pid) else {
            return Self(None);
        };
        let key = NEXT.fetch_add(1, Ordering::Relaxed);
        running().insert(
            key,
            Running {
                agent_id: agent_id.clone(),
                pid,
                stopped_at: None,
            },
        );
        Self(Some(key))
    }

    /// When the user stopped the command, if they did.
    pub(super) fn stopped_at(&self) -> Option<Instant> {
        let key = self.0?;
        running().get(&key)?.stopped_at
    }

    /// Whether the stopped command has had its grace and must be killed.
    pub(super) fn overdue(&self) -> bool {
        self.stopped_at()
            .is_some_and(|at| at.elapsed() > STOP_GRACE)
    }

    /// `result`, with the note that the user stopped the command when
    /// they did. A command that succeeded finished before the stop
    /// reached it.
    pub(super) fn annotate(&self, mut result: ToolResult) -> ToolResult {
        if result.ok || self.stopped_at().is_none() {
            return result;
        }
        let mut stderr = result.stderr.to_vec();
        if !stderr.is_empty() && !stderr.ends_with(b"\n") {
            stderr.push(b'\n');
        }
        stderr.extend_from_slice(STOPPED_NOTE.as_bytes());
        result.stderr = stderr.into();
        result.with_metadata("stopped_by_user", "true")
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(key) = self.0 {
            running().remove(&key);
        }
    }
}

/// Stop the foreground commands of the agent with hex id `agent_id`:
/// `SIGTERM` to each one's process group, or `SIGKILL` to one already
/// stopped. Returns how many there were.
pub fn stop_commands(agent_id: &str) -> usize {
    let targets: Vec<(u32, bool)> = running()
        .values_mut()
        .filter(|command| command.agent_id == agent_id)
        .map(|command| {
            let again = command.stopped_at.is_some();
            command.stopped_at.get_or_insert_with(Instant::now);
            (command.pid, again)
        })
        .collect();
    for (pid, force) in &targets {
        signal_group(*pid, *force);
    }
    targets.len()
}

#[cfg(unix)]
fn signal_group(pid: u32, force: bool) {
    let signal = if force { "-KILL" } else { "-TERM" };
    let _ = std::process::Command::new("kill")
        .args([signal, "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn signal_group(pid: u32, _force: bool) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::sandbox::Sandbox;
    use aura_core_types::AgentId;
    use std::thread;

    fn tags() -> ProcessTags {
        ProcessTags {
            agent_id: Some(AgentId::generate().to_hex()),
            tool_use_id: Some("toolu_stop".into()),
            scratch: None,
        }
    }

    fn registered(agent: &str) -> bool {
        running().values().any(|command| command.agent_id == agent)
    }

    /// Stop `agent`'s command once it has run for a moment.
    fn stop_soon(agent: String) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !registered(&agent) {
                assert!(Instant::now() < deadline, "command never registered");
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(300));
            assert_eq!(stop_commands(&agent), 1);
        })
    }

    fn stopped_note(result: &ToolResult) -> bool {
        result.metadata.get("stopped_by_user").map(String::as_str) == Some("true")
            && String::from_utf8_lossy(&result.stderr).contains("[STOPPED]")
    }

    #[test]
    fn stopping_returns_the_output_so_far() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let tags = tags();
        let agent = tags.agent_id.clone().unwrap();
        let stopper = stop_soon(agent.clone());
        let start = Instant::now();

        let result = super::super::cmd_run_shell_script(
            &sandbox,
            "echo started; sleep 30",
            None,
            60_000,
            &tags,
        )
        .unwrap();

        stopper.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(!result.ok);
        assert!(stopped_note(&result), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("started"));
        assert!(!registered(&agent));
    }

    #[test]
    fn a_command_ignoring_term_is_killed_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let tags = tags();
        let stopper = stop_soon(tags.agent_id.clone().unwrap());
        let start = Instant::now();

        let result = super::super::cmd_run_shell_script(
            &sandbox,
            "trap '' TERM; echo started; sleep 30",
            None,
            60_000,
            &tags,
        )
        .unwrap();

        stopper.join().unwrap();
        assert!(start.elapsed() >= STOP_GRACE);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(stopped_note(&result), "{result:?}");
    }

    #[test]
    fn stopping_a_terminal_command_keeps_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let tags = tags();
        let stopper = stop_soon(tags.agent_id.clone().unwrap());

        let result = super::super::pty::cmd_run_in_pty(
            &sandbox,
            &super::super::pty::PtyCommand::ShellScript("echo started; sleep 30"),
            None,
            80,
            60_000,
            &tags,
        )
        .expect("pseudo-terminal available")
        .unwrap();

        stopper.join().unwrap();
        assert!(!result.ok);
        assert!(stopped_note(&result), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains("started"));
    }

    #[test]
    fn commands_without_an_agent_are_not_registered() {
        assert!(Registration::new(&ProcessTags::default(), Some(1))
            .0
            .is_none());
    }
}
