//! Killing rows and building the snapshot the Processes tab shows.

use super::arrange::arrange;
use super::os_processes::{self, OsProcess};
use super::{agent_row_id, command_row_id, lock, Inner, Killed, ProcessTable, Row};
use aura_agent::ProcessInfo;
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, ProcessId};
use aura_engine::agents::LifecycleAction;
use aura_engine::{AgentInfo, AgentState};
use aura_ui::events::{ProcessKind, ProcessStatus, ProcessSummary};
use std::collections::HashSet;
use std::sync::MutexGuard;

/// What a snapshot merges in, gathered before the table is locked.
struct Sources {
    agents: Vec<(AgentId, Vec<AgentInfo>)>,
    commands: Vec<ProcessInfo>,
    os: Vec<OsProcess>,
}

impl ProcessTable {
    /// Kill row `id`.
    ///
    /// # Errors
    /// When the row is unknown, already finished, or can't be killed.
    pub(crate) fn kill(&self, id: &str) -> Result<Killed, String> {
        let inner = lock(&self.inner);
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
        self.kill_found(inner, id, &summary)?;
        Ok(done)
    }

    /// Stop the work behind killable row `summary`, by whichever means
    /// runs it.
    fn kill_found(
        &self,
        mut inner: MutexGuard<'_, Inner>,
        id: &str,
        summary: &ProcessSummary,
    ) -> Result<(), String> {
        let cancel = inner
            .live
            .iter()
            .find(|live| live.summary.id == id)
            .and_then(|live| live.cancel.clone());
        if let Some(cancel) = cancel {
            cancel.cancel();
            inner.killed.insert(id.to_string());
            return Ok(());
        }
        if let Some(process) = inner.os.get(id).cloned() {
            drop(inner);
            os_processes::kill(&process)?;
            lock(&self.inner).killed.insert(id.to_string());
            return Ok(());
        }
        if let Some(hex) = id.strip_prefix("cmd:") {
            let process = ProcessId::from_hex(hex).map_err(|e| e.to_string())?;
            let manager = self.commands.as_ref().ok_or("no process manager")?;
            inner.killed.insert(id.to_string());
            drop(inner);
            if !manager.cancel(&process) {
                return Err(format!("{} has already exited", summary.label));
            }
            return Ok(());
        }
        if let Some(hex) = id.strip_prefix("agent:") {
            let agent = AgentId::from_hex(hex).map_err(|e| e.to_string())?;
            let host = self.host.as_ref().ok_or("no agent host")?;
            inner.killed.insert(id.to_string());
            drop(inner);
            host.lifecycle(agent, LifecycleAction::Stop)?;
            return Ok(());
        }
        Err(format!("{} can't be killed", summary.label))
    }

    /// The rows to show: live work as a tree, then finished rows, newest
    /// first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProcessSummary> {
        self.snapshot_scanning(true)
    }

    /// [`Self::snapshot`], looking through `/proc` for tagged processes
    /// only when `scan`; otherwise the ones the last scan found are kept.
    #[must_use]
    pub fn snapshot_scanning(&self, scan: bool) -> Vec<ProcessSummary> {
        let sources = self.sources(scan);
        let mut inner = lock(&self.inner);
        let (found, os) = found_rows(&inner, sources);
        reconcile(&mut inner, &found, os);
        let mut rows = live_rows(&inner);
        rows.extend(found);
        let mut out = arrange(rows);
        out.extend(inner.finished.iter().cloned());
        out
    }

    /// Background agents, commands and (when `scan`) OS processes.
    fn sources(&self, scan: bool) -> Sources {
        let roots = lock(&self.inner).roots.clone();
        let agents: Vec<(AgentId, Vec<AgentInfo>)> =
            self.host.as_ref().map_or_else(Vec::new, |h| {
                roots.iter().map(|root| (*root, h.tree(*root))).collect()
            });
        let commands = self
            .commands
            .as_ref()
            .map_or_else(Vec::new, |m| m.snapshot());
        let os = match (self.scan_os, scan) {
            (false, _) => Vec::new(),
            (true, true) => os_processes::tagged_processes(),
            (true, false) => lock(&self.inner).os.values().cloned().collect(),
        };
        Sources {
            agents,
            commands,
            os,
        }
    }
}

/// Rows for what runs on its own, and the OS processes kept among them.
fn found_rows(inner: &Inner, sources: Sources) -> (Vec<Row>, Vec<OsProcess>) {
    let Sources {
        agents,
        commands,
        os,
    } = sources;
    let mut found: Vec<Row> = Vec::new();
    for (root, tree) in &agents {
        for info in tree.iter().filter(|info| info.state != AgentState::Root) {
            found.push(agent_row(inner, *root, tree, info));
        }
    }
    let command_pids: HashSet<u32> = commands.iter().map(|c| c.pid).collect();
    for info in &commands {
        found.push(command_row(inner, info));
    }
    let os: Vec<OsProcess> = os
        .into_iter()
        .filter(|p| !command_pids.contains(&p.pid))
        .filter(|p| {
            p.harness_pid == Some(std::process::id()) || inner.names.contains_key(&p.agent_id)
        })
        .collect();
    for process in &os {
        found.push(os_row(inner, process));
    }
    (found, os)
}

/// Retire rows that vanished since the last snapshot and remember the
/// ones `found` now.
fn reconcile(inner: &mut Inner, found: &[Row], os: Vec<OsProcess>) {
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
    } = inner;
    killed.retain(|id| live.iter().any(|l| l.summary.id == *id) || seen.contains_key(id));
}

/// Rows for the work the event loop tracks itself.
fn live_rows(inner: &Inner) -> Vec<Row> {
    inner
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
        .collect()
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

pub(super) fn command_row(inner: &Inner, info: &ProcessInfo) -> Row {
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
        .is_some_and(super::super::run_command::is_user_tool_use);
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
