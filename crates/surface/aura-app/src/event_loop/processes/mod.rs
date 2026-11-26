//! What the Processes tab lists, and killing it.
//!
//! The table tracks the work the event loop starts itself (agent turns
//! and `task` spawns) and, on every snapshot, merges in what runs on its
//! own: background agents in the [`AgentHost`], background commands in
//! the [`ProcessManager`], and any OS process an agent's command started
//! (see [`super::os_processes`]). Rows that disappear are kept, with how
//! they ended, until [`MAX_FINISHED`] newer ones push them out.
//!
//! A publisher task sends the list to the UI whenever it changes, so it
//! stays live while agents work, along with each agent's background jobs
//! for its status line.

mod arrange;
mod background;
mod snapshot;

pub use arrange::spawn_publisher;
pub(crate) use background::stop_background_commands;
pub use background::BackgroundCommands;
use snapshot::command_row;

use super::os_processes::{self, OsProcess};
use super::subagents::lock;
use aura_agent::{ProcessInfo, ProcessManager};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, ProcessId};
use aura_engine::{AgentHost, AgentState};
use aura_ui::events::{ProcessKind, ProcessStatus, ProcessSummary};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Finished rows kept for the list.
pub(crate) const MAX_FINISHED: usize = 50;

/// How often the publisher looks for changes.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(500);
/// Publishes between scans of `/proc`, which reads every process on the
/// machine; the harness's own turns, agents and commands are checked on
/// every publish.
const OS_SCAN_EVERY: u32 = 10;

/// Row id of `agent`'s running turn.
pub(crate) fn turn_id(agent: AgentId) -> String {
    format!("turn:{}", agent.to_hex())
}

/// Row id of the `task` spawn running as `child`.
pub(crate) fn task_id(child: AgentId) -> String {
    format!("task:{}", child.to_hex())
}

fn agent_row_id(agent: AgentId) -> String {
    format!("agent:{}", agent.to_hex())
}

fn command_row_id(process: ProcessId) -> String {
    format!("cmd:{}", process.to_hex())
}

/// Work the event loop started and can cancel.
struct Live {
    summary: ProcessSummary,
    cancel: Option<CancellationToken>,
}

/// A row while the tree is being built.
struct Row {
    summary: ProcessSummary,
    /// Hex id of the agent this row is (a turn of it, or the agent itself).
    subject: Option<String>,
    ppid: Option<u32>,
}

/// How a kill request was carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Killed {
    pub(crate) kind: ProcessKind,
    pub(crate) label: String,
    /// Hex id of the agent that owned it.
    pub(crate) owner: String,
}

#[derive(Default)]
struct Inner {
    live: Vec<Live>,
    finished: VecDeque<ProcessSummary>,
    /// Rows found by the last snapshot, by id, so vanished ones can be
    /// moved to `finished`.
    seen: HashMap<String, ProcessSummary>,
    /// OS processes from the last snapshot, by row id, for kills.
    os: HashMap<String, OsProcess>,
    /// Rows the user killed that haven't finished yet.
    killed: HashSet<String>,
    /// Background commands their own agent stopped (`job_kill`), whose
    /// end it needn't be told about.
    stopped_by_owner: HashSet<String>,
    /// Display names of the terminal's agents, by hex id.
    names: HashMap<String, String>,
    /// Root agents registered with the host.
    roots: Vec<AgentId>,
    /// `finished` changed since it was last saved.
    unsaved: bool,
}

impl Inner {
    fn retire(&mut self, mut summary: ProcessSummary, status: ProcessStatus) {
        summary.status = if self.killed.remove(&summary.id) {
            ProcessStatus::Killed
        } else {
            status
        };
        if summary.owner_name.is_empty() {
            summary.owner_name = self.name_of(&summary.owner_agent_id);
        }
        summary.ended_ms.get_or_insert_with(now_ms);
        summary.killable = false;
        summary.depth = 0;
        self.finished.push_front(summary);
        self.finished.truncate(MAX_FINISHED);
        self.unsaved = true;
    }

    fn name_of(&self, agent_hex: &str) -> String {
        self.names
            .get(agent_hex)
            .cloned()
            .unwrap_or_else(|| short_hex(agent_hex))
    }
}

fn short_hex(hex: &str) -> String {
    hex.chars().take(8).collect()
}

/// Everything the Processes tab lists.
pub struct ProcessTable {
    inner: Mutex<Inner>,
    host: Option<AgentHost>,
    commands: Option<Arc<ProcessManager>>,
    scan_os: bool,
    /// Where finished rows are kept between sessions.
    history: OnceLock<PathBuf>,
}

impl ProcessTable {
    #[must_use]
    pub fn new(host: Option<AgentHost>, commands: Option<Arc<ProcessManager>>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            host,
            commands,
            scan_os: true,
            history: OnceLock::new(),
        })
    }

    /// Keep finished rows in `path` between sessions, starting with the
    /// ones it already holds.
    pub fn keep_history(&self, path: PathBuf) {
        let rows: Vec<ProcessSummary> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(path = %path.display(), error = %e, "Unreadable process history");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        if self.history.set(path).is_err() {
            return;
        }
        let mut inner = lock(&self.inner);
        inner.finished.extend(rows);
        inner.finished.truncate(MAX_FINISHED);
    }

    /// Write the finished rows to the history file, if they changed.
    pub fn save(&self) {
        let Some(path) = self.history.get() else {
            return;
        };
        let rows: Vec<ProcessSummary> = {
            let mut inner = lock(&self.inner);
            if !std::mem::take(&mut inner.unsaved) {
                return;
            }
            inner.finished.iter().cloned().collect()
        };
        let written = serde_json::to_vec(&rows)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(e) = written {
            warn!(path = %path.display(), error = %e, "Could not save process history");
            lock(&self.inner).unsaved = true;
        }
    }

    /// A table that doesn't look at OS processes, for tests.
    #[cfg(test)]
    pub(crate) fn without_os_scan(
        host: Option<AgentHost>,
        commands: Option<Arc<ProcessManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            host,
            commands,
            scan_os: false,
            history: OnceLock::new(),
        })
    }

    #[must_use]
    pub fn host(&self) -> Option<&AgentHost> {
        self.host.as_ref()
    }

    /// Show `name` for `agent` in owner columns.
    pub fn name_agent(&self, agent: AgentId, name: &str) {
        lock(&self.inner)
            .names
            .insert(agent.to_hex(), name.to_string());
    }

    /// [`Self::name_agent`] for each of `agents`, under one lock, leaving
    /// names that haven't changed as they are.
    pub fn name_agents<N: AsRef<str>>(&self, agents: impl IntoIterator<Item = (AgentId, N)>) {
        let mut inner = lock(&self.inner);
        for (agent, name) in agents {
            let name = name.as_ref();
            let hex = agent.to_hex();
            if inner.names.get(&hex).map(String::as_str) != Some(name) {
                inner.names.insert(hex, name.to_string());
            }
        }
    }

    /// List the background agents under `root`.
    pub fn add_root(&self, root: AgentId) {
        let mut inner = lock(&self.inner);
        if !inner.roots.contains(&root) {
            inner.roots.push(root);
        }
    }

    /// Track work the event loop runs; `cancel` stops it.
    pub fn start(&self, summary: ProcessSummary, cancel: Option<CancellationToken>) {
        let mut inner = lock(&self.inner);
        inner.live.retain(|live| live.summary.id != summary.id);
        inner.live.push(Live { summary, cancel });
    }

    /// End tracked work `id` with `status`.
    pub fn finish(&self, id: &str, status: ProcessStatus) {
        let mut inner = lock(&self.inner);
        let Some(index) = inner.live.iter().position(|live| live.summary.id == id) else {
            return;
        };
        let live = inner.live.remove(index);
        inner.retire(live.summary, status);
    }

    /// List background command `info` right away, so it has a row (and
    /// its end a notice) even if it exits before the next snapshot.
    pub(crate) fn command_started(&self, info: &ProcessInfo) {
        let mut inner = lock(&self.inner);
        let row = command_row(&inner, info);
        inner.seen.insert(row.summary.id.clone(), row.summary);
    }

    /// Note that the agent that started background command `process` is
    /// stopping it itself.
    pub(crate) fn stopped_by_owner(&self, process: ProcessId) {
        lock(&self.inner)
            .stopped_by_owner
            .insert(command_row_id(process));
    }

    /// Whether the agent that started `process` stopped it itself,
    /// forgetting that it did.
    pub(crate) fn take_stopped_by_owner(&self, process: ProcessId) -> bool {
        lock(&self.inner)
            .stopped_by_owner
            .remove(&command_row_id(process))
    }

    /// Whether `agent` has work running in the background that will tell
    /// it when it ends: commands, `task` spawns or agents it spawned.
    #[must_use]
    pub fn has_background_work(&self, agent: AgentId) -> bool {
        let hex = agent.to_hex();
        let tasks = lock(&self.inner).live.iter().any(|live| {
            live.summary.kind != ProcessKind::Turn && live.summary.owner_agent_id == hex
        });
        let commands = self
            .commands
            .as_ref()
            .is_some_and(|m| m.snapshot().iter().any(|info| info.agent_id == agent));
        let agents = self.host.as_ref().is_some_and(|h| {
            h.tree(agent)
                .iter()
                .any(|info| info.state == AgentState::Running)
        });
        tasks || commands || agents
    }

    /// Record how background command `process` ended, returning its row.
    pub fn command_finished(
        &self,
        process: ProcessId,
        exit_code: Option<i32>,
        success: bool,
    ) -> Option<ProcessSummary> {
        let id = command_row_id(process);
        let status = if success {
            ProcessStatus::Exited
        } else {
            ProcessStatus::Failed
        };
        let mut inner = lock(&self.inner);
        if let Some(mut summary) = inner.seen.remove(&id) {
            summary.exit_code = exit_code;
            inner.retire(summary, status);
        }
        let done = inner.finished.iter_mut().find(|row| row.id == id)?;
        if done.status != ProcessStatus::Killed {
            done.status = status;
        }
        done.exit_code = exit_code;
        let row = done.clone();
        inner.unsaved = true;
        Some(row)
    }
}

/// Log a kill that could not be carried out.
pub(crate) fn warn_kill_failed(id: &str, problem: &str) {
    warn!(process = id, %problem, "Kill failed");
}

#[cfg(test)]
mod tests;
