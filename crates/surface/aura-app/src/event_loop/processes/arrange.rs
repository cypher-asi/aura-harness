//! Ordering rows as a tree, and publishing them to the UI.

use super::{ProcessTable, Row, OS_SCAN_EVERY, PUBLISH_INTERVAL};
use aura_ui::events::{JobView, ProcessKind, ProcessStatus, ProcessSummary};
use aura_ui::UiCommand;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Order `rows` as a tree, oldest first among siblings, and set depths.
///
/// A row's parent is the row of the process that forked it, else the row
/// of the agent that owns it (a background agent or task before that
/// agent's turn). Turns are always at the top.
pub(super) fn arrange(rows: Vec<Row>) -> Vec<ProcessSummary> {
    let parents = parents(&rows);
    let (tops, children) = tops_and_children(&rows, &parents);
    let order = tree_order(rows.len(), &tops, &children);
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

/// Each row's parent row, if any.
fn parents(rows: &[Row]) -> Vec<Option<usize>> {
    let mut by_pid: HashMap<u32, usize> = HashMap::new();
    let mut by_subject: HashMap<&str, usize> = HashMap::new();
    let mut by_subject_not_turn: HashMap<&str, usize> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        if let Some(pid) = row.summary.pid {
            by_pid.entry(pid).or_insert(i);
        }
        if let Some(subject) = row.subject.as_deref() {
            by_subject.entry(subject).or_insert(i);
            if row.summary.kind != ProcessKind::Turn {
                by_subject_not_turn.entry(subject).or_insert(i);
            }
        }
    }
    let parent_of = |row: &Row| -> Option<usize> {
        if let Some(i) = row.ppid.and_then(|ppid| by_pid.get(&ppid)) {
            return Some(*i);
        }
        if row.summary.kind == ProcessKind::Turn {
            return None;
        }
        let owner = row.summary.owner_agent_id.as_str();
        by_subject_not_turn
            .get(owner)
            .or_else(|| by_subject.get(owner))
            .copied()
    };
    rows.iter()
        .enumerate()
        .map(|(i, row)| parent_of(row).filter(|p| *p != i))
        .collect()
}

/// The top rows and each row's children, oldest first.
fn tops_and_children(rows: &[Row], parents: &[Option<usize>]) -> (Vec<usize>, Vec<Vec<usize>>) {
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
    (tops, children)
}

/// Depth-first order of `len` rows as `(row, depth)`.
fn tree_order(len: usize, tops: &[usize], children: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let mut placed = vec![false; len];
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
    order
}

/// Each agent's background commands in `rows`, as its status line lists
/// them, by agent hex id.
pub(crate) fn job_views(rows: &[ProcessSummary]) -> HashMap<String, Vec<JobView>> {
    let mut jobs: HashMap<String, Vec<JobView>> = HashMap::new();
    for row in rows.iter().filter(|row| row.kind == ProcessKind::Command) {
        let Some(id) = row.id.strip_prefix("cmd:") else {
            continue;
        };
        jobs.entry(row.owner_agent_id.clone())
            .or_default()
            .push(JobView {
                id: id.to_string(),
                command: row.label.clone(),
                running: row.status == ProcessStatus::Running,
            });
    }
    jobs
}

/// Send the table to the UI whenever it changes, and every agent whose
/// background jobs changed its new list.
pub fn spawn_publisher(
    table: Arc<ProcessTable>,
    commands: mpsc::Sender<UiCommand>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last: Option<Vec<ProcessSummary>> = None;
        let mut last_jobs: HashMap<String, Vec<JobView>> = HashMap::new();
        let mut tick = tokio::time::interval(PUBLISH_INTERVAL);
        let mut publishes: u32 = 0;
        loop {
            tick.tick().await;
            let scan = publishes % OS_SCAN_EVERY == 0;
            publishes = publishes.wrapping_add(1);
            let table = Arc::clone(&table);
            let snapshot = move || {
                let rows = table.snapshot_scanning(scan);
                table.save();
                rows
            };
            let Ok(rows) = tokio::task::spawn_blocking(snapshot).await else {
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
            let jobs = job_views(&rows);
            publish_jobs(&commands, &jobs, &last_jobs).await;
            last_jobs = jobs;
            last = Some(rows);
        }
    })
}

/// Send each agent whose background jobs changed its new list.
async fn publish_jobs(
    commands: &mpsc::Sender<UiCommand>,
    jobs: &HashMap<String, Vec<JobView>>,
    last_jobs: &HashMap<String, Vec<JobView>>,
) {
    let agents: HashSet<&String> = jobs.keys().chain(last_jobs.keys()).collect();
    for agent in agents {
        let now = jobs.get(agent).cloned().unwrap_or_default();
        if last_jobs.get(agent).is_some_and(|before| *before == now) {
            continue;
        }
        let _ = commands
            .send(UiCommand::ForAgent {
                agent_id: agent.clone(),
                command: Box::new(UiCommand::SetJobs(now)),
            })
            .await;
    }
}
