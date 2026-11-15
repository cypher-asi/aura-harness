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
//! stays live while the event loop is busy with a turn.

use super::os_processes::{self, OsProcess};
use super::subagents::lock;
use aura_agent::{ProcessInfo, ProcessManager, ProcessMeta};
use aura_core_types::{ActionId, AgentId, Hash, ProcessId};
use aura_engine::agents::LifecycleAction;
use aura_engine::{AgentHost, AgentInfo, AgentState};
use aura_store_db::ReadStore;
use aura_terminal::events::{ProcessKind, ProcessStatus, ProcessSummary};
use aura_terminal::UiCommand;
use aura_tools::{BackgroundProcess, BackgroundProcessHook};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Finished rows kept for the list.
pub(crate) const MAX_FINISHED: usize = 50;

/// How often the publisher looks for changes.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(500);

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

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
    /// Display names of the terminal's agents, by hex id.
    names: HashMap<String, String>,
    /// Root agents registered with the host.
    roots: Vec<AgentId>,
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
}

impl ProcessTable {
    #[must_use]
    pub fn new(host: Option<AgentHost>, commands: Option<Arc<ProcessManager>>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            host,
            commands,
            scan_os: true,
        })
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
        Some(done.clone())
    }

    /// Kill row `id`.
    ///
    /// # Errors
    /// When the row is unknown, already finished, or can't be killed.
    pub(crate) fn kill(&self, id: &str) -> Result<Killed, String> {
        let mut inner = lock(&self.inner);
        let summary = inner
            .live
            .iter()
            .map(|live| &live.summary)
            .chain(inner.seen.values())
            .find(|summary| summary.id == id)
            .cloned()
            .ok_or_else(|| "That process has already finished".to_string())?;
        if !summary.killable {
            return Err(format!("{} can't be killed", summary.label));
        }
        let done = Killed {
            kind: summary.kind,
            label: summary.label.clone(),
            owner: summary.owner_agent_id.clone(),
        };

        let cancel = inner
            .live
            .iter()
            .find(|live| live.summary.id == id)
            .and_then(|live| live.cancel.clone());
        if let Some(cancel) = cancel {
            cancel.cancel();
            inner.killed.insert(id.to_string());
            return Ok(done);
        }
        if let Some(process) = inner.os.get(id).cloned() {
            drop(inner);
            os_processes::kill(&process)?;
            lock(&self.inner).killed.insert(id.to_string());
            return Ok(done);
        }
        if let Some(hex) = id.strip_prefix("cmd:") {
            let process = ProcessId::from_hex(hex).map_err(|e| e.to_string())?;
            let manager = self.commands.as_ref().ok_or("no process manager")?;
            inner.killed.insert(id.to_string());
            drop(inner);
            if !manager.cancel(&process) {
                return Err(format!("{} has already exited", summary.label));
            }
            return Ok(done);
        }
        if let Some(hex) = id.strip_prefix("agent:") {
            let agent = AgentId::from_hex(hex).map_err(|e| e.to_string())?;
            let host = self.host.as_ref().ok_or("no agent host")?;
            inner.killed.insert(id.to_string());
            drop(inner);
            host.lifecycle(agent, LifecycleAction::Stop)?;
            return Ok(done);
        }
        Err(format!("{} can't be killed", summary.label))
    }

    /// The rows to show: live work as a tree, then finished rows, newest
    /// first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProcessSummary> {
        let roots = lock(&self.inner).roots.clone();
        let agents: Vec<(AgentId, Vec<AgentInfo>)> =
            self.host.as_ref().map_or_else(Vec::new, |h| {
                roots.iter().map(|root| (*root, h.tree(*root))).collect()
            });
        let commands = self
            .commands
            .as_ref()
            .map_or_else(Vec::new, |m| m.snapshot());
        let os = if self.scan_os {
            os_processes::tagged_processes()
        } else {
            Vec::new()
        };

        let mut inner = lock(&self.inner);
        let mut found: Vec<Row> = Vec::new();
        for (root, tree) in &agents {
            for info in tree.iter().filter(|info| info.state != AgentState::Root) {
                found.push(agent_row(&inner, *root, tree, info));
            }
        }
        let command_pids: HashSet<u32> = commands.iter().map(|c| c.pid).collect();
        for info in &commands {
            found.push(command_row(&inner, info));
        }
        let os: Vec<OsProcess> = os
            .into_iter()
            .filter(|p| !command_pids.contains(&p.pid))
            .filter(|p| {
                p.harness_pid == Some(std::process::id()) || inner.names.contains_key(&p.agent_id)
            })
            .collect();
        for process in &os {
            found.push(os_row(&inner, process));
        }

        let now: HashSet<String> = found.iter().map(|row| row.summary.id.clone()).collect();
        let vanished: Vec<ProcessSummary> = inner
            .seen
            .iter()
            .filter(|(id, _)| !now.contains(*id))
            .map(|(_, summary)| summary.clone())
            .collect();
        for summary in vanished {
            inner.seen.remove(&summary.id);
            inner.retire(summary, ProcessStatus::Exited);
        }
        inner.seen = found
            .iter()
            .map(|row| (row.summary.id.clone(), row.summary.clone()))
            .collect();
        inner.os = os.into_iter().map(|p| (p.key(), p)).collect();
        let Inner {
            killed, live, seen, ..
        } = &mut *inner;
        killed.retain(|id| live.iter().any(|l| l.summary.id == *id) || seen.contains_key(id));

        let mut rows: Vec<Row> = inner
            .live
            .iter()
            .map(|live| {
                let mut summary = live.summary.clone();
                summary.owner_name = inner.name_of(&summary.owner_agent_id);
                Row {
                    subject: live_subject(&summary),
                    summary,
                    ppid: None,
                }
            })
            .collect();
        rows.extend(found);
        let mut out = arrange(rows);
        out.extend(inner.finished.iter().cloned());
        out
    }
}

/// Agent a tracked row is: the agent a turn runs, or a `task` child.
fn live_subject(summary: &ProcessSummary) -> Option<String> {
    match summary.kind {
        ProcessKind::Turn => Some(summary.owner_agent_id.clone()),
        ProcessKind::Task => summary.id.strip_prefix("task:").map(str::to_string),
        _ => None,
    }
}

fn agent_row(inner: &Inner, root: AgentId, tree: &[AgentInfo], info: &AgentInfo) -> Row {
    let mut chain = Vec::new();
    let mut parent = info.parent;
    while let Some(id) = parent {
        chain.push(id.to_hex());
        parent = tree
            .iter()
            .find(|a| a.agent_id == id)
            .and_then(|a| a.parent);
    }
    let owner = chain.first().cloned().unwrap_or_else(|| root.to_hex());
    let status = match info.state {
        AgentState::Running => ProcessStatus::Running,
        AgentState::Paused => ProcessStatus::Paused,
        AgentState::Idle | AgentState::Root => ProcessStatus::Idle,
    };
    Row {
        summary: ProcessSummary {
            id: agent_row_id(info.agent_id),
            kind: ProcessKind::Agent,
            label: format!("{} ({})", info.name, info.role),
            detail: info.last_reply.clone().unwrap_or_default(),
            status,
            owner_name: inner.name_of(&owner),
            owner_agent_id: owner,
            parent_chain: chain.into_iter().skip(1).collect(),
            started_ms: inner
                .seen
                .get(&agent_row_id(info.agent_id))
                .map_or_else(now_ms, |seen| seen.started_ms),
            model: Some(info.model.clone()),
            turns: Some(info.turns),
            queued: Some(info.queued),
            jump_agent_id: root.to_hex(),
            killable: true,
            ..ProcessSummary::default()
        },
        subject: Some(info.agent_id.to_hex()),
        ppid: None,
    }
}

fn command_row(inner: &Inner, info: &ProcessInfo) -> Row {
    let owner = info.agent_id.to_hex();
    Row {
        summary: ProcessSummary {
            id: command_row_id(info.process_id),
            kind: ProcessKind::Command,
            label: info.command.clone(),
            detail: info.command.clone(),
            status: ProcessStatus::Running,
            owner_name: inner.name_of(&owner),
            jump_agent_id: owner.clone(),
            owner_agent_id: owner,
            started_ms: info.started_ms,
            tool_use_id: info.meta.tool_use_id.clone(),
            spawn_tx: Some(hex::encode(info.reference_tx_hash.as_bytes())),
            pid: Some(info.pid),
            cwd: info.meta.cwd.as_ref().map(|p| p.display().to_string()),
            log_path: info.meta.log_path.clone(),
            killable: true,
            ..ProcessSummary::default()
        },
        subject: None,
        ppid: None,
    }
}

fn os_row(inner: &Inner, process: &OsProcess) -> Row {
    let shell = process
        .tool_use_id
        .as_deref()
        .is_some_and(super::run_command::is_user_tool_use);
    Row {
        summary: ProcessSummary {
            id: process.key(),
            kind: if shell {
                ProcessKind::Shell
            } else {
                ProcessKind::Command
            },
            label: process.command.clone(),
            detail: process.command.clone(),
            status: ProcessStatus::Running,
            owner_name: inner.name_of(&process.agent_id),
            owner_agent_id: process.agent_id.clone(),
            jump_agent_id: process.agent_id.clone(),
            started_ms: process.started_ms,
            tool_use_id: process.tool_use_id.clone(),
            pid: Some(process.pid),
            cwd: process.cwd.as_ref().map(|p| p.display().to_string()),
            killable: true,
            ..ProcessSummary::default()
        },
        subject: None,
        ppid: Some(process.ppid),
    }
}

/// Order `rows` as a tree, oldest first among siblings, and set depths.
///
/// A row's parent is the row of the process that forked it, else the row
/// of the agent that owns it (a background agent or task before that
/// agent's turn). Turns are always at the top.
fn arrange(rows: Vec<Row>) -> Vec<ProcessSummary> {
    let parent_of = |row: &Row| -> Option<usize> {
        if let Some(ppid) = row.ppid {
            if let Some(i) = rows.iter().position(|r| r.summary.pid == Some(ppid)) {
                return Some(i);
            }
        }
        if row.summary.kind == ProcessKind::Turn {
            return None;
        }
        let owner = Some(&row.summary.owner_agent_id);
        rows.iter()
            .position(|r| r.subject.as_ref() == owner && r.summary.kind != ProcessKind::Turn)
            .or_else(|| rows.iter().position(|r| r.subject.as_ref() == owner))
    };
    let parents: Vec<Option<usize>> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| parent_of(row).filter(|p| *p != i))
        .collect();

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut tops = Vec::new();
    for (i, parent) in parents.iter().enumerate() {
        match parent {
            Some(p) => children[*p].push(i),
            None => tops.push(i),
        }
    }
    let by_start = |list: &mut Vec<usize>| list.sort_by_key(|i| rows[*i].summary.started_ms);
    by_start(&mut tops);
    for list in &mut children {
        by_start(list);
    }

    let mut placed = vec![false; rows.len()];
    let mut order: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(usize, usize)> = tops.iter().rev().map(|i| (*i, 0)).collect();
    loop {
        while let Some((i, depth)) = stack.pop() {
            if std::mem::replace(&mut placed[i], true) {
                continue;
            }
            order.push((i, depth));
            stack.extend(children[i].iter().rev().map(|c| (*c, depth + 1)));
        }
        // Rows on a parent cycle have no top; start from the first.
        match placed.iter().position(|p| !p) {
            Some(i) => stack.push((i, 0)),
            None => break,
        }
    }

    let mut rows: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|(i, depth)| {
            rows[i].take().map(|row| ProcessSummary {
                depth,
                ..row.summary
            })
        })
        .collect()
}

/// Send the table to the UI whenever it changes.
pub fn spawn_publisher(
    table: Arc<ProcessTable>,
    commands: mpsc::Sender<UiCommand>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last: Option<Vec<ProcessSummary>> = None;
        let mut tick = tokio::time::interval(PUBLISH_INTERVAL);
        loop {
            tick.tick().await;
            let table = Arc::clone(&table);
            let Ok(rows) = tokio::task::spawn_blocking(move || table.snapshot()).await else {
                continue;
            };
            if last.as_ref() == Some(&rows) {
                continue;
            }
            if commands
                .send(UiCommand::SetProcesses(rows.clone()))
                .await
                .is_err()
            {
                return;
            }
            last = Some(rows);
        }
    })
}

/// Hands `run_command`'s background commands to the [`ProcessManager`],
/// which reports each one's exit to the agent that started it.
pub struct BackgroundCommands {
    manager: Arc<ProcessManager>,
    store: Arc<dyn ReadStore>,
    log_dir: PathBuf,
}

impl BackgroundCommands {
    #[must_use]
    pub fn new(manager: Arc<ProcessManager>, store: Arc<dyn ReadStore>, log_dir: PathBuf) -> Self {
        Self {
            manager,
            store,
            log_dir,
        }
    }

    /// The agent's latest recorded transaction, which the completion
    /// refers back to.
    fn latest_tx(&self, agent: AgentId) -> Option<Hash> {
        let seq = self.store.get_head_seq(agent).ok()?;
        self.store
            .get_record_entry(agent, seq)
            .ok()
            .map(|entry| entry.tx.hash)
    }
}

impl BackgroundProcessHook for BackgroundCommands {
    fn log_dir(&self) -> PathBuf {
        self.log_dir.clone()
    }

    fn adopt(&self, agent_id: AgentId, process: BackgroundProcess) -> Result<(), String> {
        let reference = self
            .latest_tx(agent_id)
            .unwrap_or_else(|| Hash::from_content(process.command.as_bytes()));
        if tokio::runtime::Handle::try_current().is_err() {
            let mut child = process.child;
            let _ = child.kill();
            let _ = child.wait();
            return Err("no async runtime to watch the process".to_string());
        }
        self.manager.register_with(
            agent_id,
            reference,
            process.action_id.unwrap_or_else(ActionId::generate),
            process.process_id,
            process.child,
            process.command,
            ProcessMeta {
                cwd: Some(process.cwd),
                log_path: Some(process.log_path),
                tool_use_id: process.tool_use_id,
            },
        );
        Ok(())
    }
}

/// Log a kill that could not be carried out.
pub(crate) fn warn_kill_failed(id: &str, problem: &str) {
    warn!(process = id, %problem, "Kill failed");
}

#[cfg(test)]
mod tests;
