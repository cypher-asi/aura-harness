//! The members of an agent's run: every `task` spawn and background agent
//! seen in the process table, and the rows listed for the agent on screen.

use std::collections::{HashMap, HashSet};

use super::MemberRow;
use crate::events::{ProcessKind, ProcessSummary};

/// Members kept, across every agent's runs; the oldest go first.
const MAX_SEEN: usize = 1000;

/// State of the MEMBERS tab.
#[derive(Debug, Default)]
pub(crate) struct Members {
    /// The latest row of every `task` spawn and background agent seen,
    /// oldest first
    seen: Vec<ProcessSummary>,
    /// When each agent's run began, in Unix milliseconds, for agents
    /// that started another with `/new`
    pub(super) since: HashMap<String, u64>,
    /// Rows listed for the agent on screen
    pub(super) rows: Vec<MemberRow>,
    /// Highlighted row
    pub(super) selected: usize,
    /// First row on screen
    pub(in crate::app) scroll: usize,
}

/// Hex id of the agent a process row of a `task` spawn or a background
/// agent stands for: what follows `task:` or `agent:` in its id.
fn member_id(process: &ProcessSummary) -> Option<&str> {
    if !matches!(process.kind, ProcessKind::Task | ProcessKind::Agent) {
        return None;
    }
    process
        .id
        .split_once(':')
        .map(|(_, id)| id)
        .filter(|id| !id.is_empty())
}

/// Process rows that may belong to a run, with their agents' ids.
type Candidates<'a> = [(&'a str, &'a ProcessSummary)];

impl Members {
    /// Keep the latest row of every member in `processes`.
    pub(super) fn absorb(&mut self, processes: &[ProcessSummary]) {
        for process in processes {
            let Some(id) = member_id(process) else {
                continue;
            };
            match self
                .seen
                .iter_mut()
                .find(|seen| member_id(seen) == Some(id))
            {
                Some(seen) => *seen = process.clone(),
                None => self.seen.push(process.clone()),
            }
        }
        let over = self.seen.len().saturating_sub(MAX_SEEN);
        self.seen.drain(..over);
    }

    /// List `lead` and the members of its run, keeping the highlight on
    /// the same agent when it is still listed.
    pub(super) fn rebuild(&mut self, lead: Option<MemberRow>) {
        let kept = self.rows.get(self.selected).map(|row| row.agent_id.clone());
        self.rows = lead.map_or_else(Vec::new, |lead| self.run_of(lead));
        self.selected = kept
            .and_then(|id| self.rows.iter().position(|row| row.agent_id == id))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    /// `lead`, then every agent its run brought in: the ones it started
    /// since its run began, and the ones they started, each under the
    /// member that started it, oldest first.
    fn run_of(&self, lead: MemberRow) -> Vec<MemberRow> {
        let lead_id = lead.agent_id.clone();
        let since = self.since.get(&lead_id).copied().unwrap_or(0);
        let candidates: Vec<(&str, &ProcessSummary)> = self
            .seen
            .iter()
            .filter(|process| process.started_ms >= since)
            .filter_map(|process| Some((member_id(process)?, process)))
            .filter(|(id, _)| *id != lead_id)
            .collect();
        let (joined, members) = join_run(&candidates, &lead_id);
        let children = children_of(&candidates, &joined, &members, &lead_id);
        place_rows(lead, &candidates, &members, &children)
    }

    /// Rows listed for the agent on screen.
    pub(in crate::app) fn rows(&self) -> &[MemberRow] {
        &self.rows
    }

    pub(super) fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }
}

/// The ids in the run (the lead's included) and the candidates that
/// joined it, in the order they joined.
fn join_run<'a>(candidates: &Candidates<'a>, lead_id: &'a str) -> (HashSet<&'a str>, Vec<usize>) {
    // An agent joins when the one that started it, or one above
    // that, is in the run. Agents a member started can come before
    // it in the table, so look again until no one else joins.
    let mut joined: HashSet<&str> = HashSet::from([lead_id]);
    let mut members: Vec<usize> = Vec::new();
    loop {
        let before = members.len();
        for (i, &(id, process)) in candidates.iter().enumerate() {
            if joined.contains(id) {
                continue;
            }
            let mut starters =
                std::iter::once(&process.owner_agent_id).chain(&process.parent_chain);
            if starters.any(|id| joined.contains(id.as_str())) {
                joined.insert(id);
                members.push(i);
            }
        }
        if members.len() == before {
            break;
        }
    }
    (joined, members)
}

/// The members under each agent of the run, oldest first.
fn children_of<'a>(
    candidates: &Candidates<'a>,
    joined: &HashSet<&str>,
    members: &[usize],
    lead_id: &'a str,
) -> HashMap<&'a str, Vec<usize>> {
    // Each member goes under the nearest agent above it in the run.
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for &i in members {
        let process = candidates[i].1;
        let parent = std::iter::once(&process.owner_agent_id)
            .chain(&process.parent_chain)
            .map(String::as_str)
            .find(|id| joined.contains(id) && *id != candidates[i].0)
            .unwrap_or(lead_id);
        children.entry(parent).or_default().push(i);
    }
    for list in children.values_mut() {
        list.sort_by_key(|&i| (candidates[i].1.started_ms, i));
    }
    children
}

/// `lead`, then each member under the one that started it.
fn place_rows(
    lead: MemberRow,
    candidates: &Candidates<'_>,
    members: &[usize],
    children: &HashMap<&str, Vec<usize>>,
) -> Vec<MemberRow> {
    let mut stack: Vec<(usize, usize)> = children
        .get(lead.agent_id.as_str())
        .map(|list| list.iter().rev().map(|&i| (i, 1)).collect())
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(members.len() + 1);
    rows.push(lead);
    let mut placed: HashSet<usize> = HashSet::new();
    loop {
        while let Some((i, depth)) = stack.pop() {
            if !placed.insert(i) {
                continue;
            }
            let (id, process) = candidates[i];
            rows.push(member_row(id, process, depth));
            if let Some(list) = children.get(id) {
                stack.extend(list.iter().rev().map(|&child| (child, depth + 1)));
            }
        }
        // Members whose starters only start each other hang under the
        // lead.
        match members.iter().find(|&&i| !placed.contains(&i)) {
            Some(&i) => stack.push((i, 1)),
            None => break,
        }
    }
    rows
}

fn member_row(id: &str, process: &ProcessSummary, depth: usize) -> MemberRow {
    MemberRow {
        agent_id: id.to_string(),
        label: process.label.clone(),
        depth,
        status: process.status,
        kind: Some(process.kind),
        started_ms: process.started_ms,
        ended_ms: process.ended_ms,
        process: Some(process.clone()),
    }
}
