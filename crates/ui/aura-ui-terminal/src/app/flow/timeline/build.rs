//! Laying a flow out as rows: each agent under the agent that spawned
//! it, a benchmark's runs by what they worked on.

use super::lane_work::{lane_work, LaneWork};
use super::{RowFacts, Timeline, TimelineRow, TurnSpan};
use crate::events::{arm_letter, FlowBench, FlowLane, FlowNode, TraceNodeStatus};
use std::collections::{HashMap, HashSet};

/// A row before the folds are applied.
struct Item {
    key: String,
    lane: Option<usize>,
    name: String,
    note: String,
    children: Vec<usize>,
    folded_by_default: bool,
}

/// The worst of two statuses: an error over work under way over work done.
const fn worse(a: TraceNodeStatus, b: TraceNodeStatus) -> TraceNodeStatus {
    match (a, b) {
        (TraceNodeStatus::Error, _) | (_, TraceNodeStatus::Error) => TraceNodeStatus::Error,
        (TraceNodeStatus::Warn, _) | (_, TraceNodeStatus::Warn) => TraceNodeStatus::Warn,
        (TraceNodeStatus::Pending, _) | (_, TraceNodeStatus::Pending) => TraceNodeStatus::Pending,
        _ => TraceNodeStatus::Ok,
    }
}

/// `A 2/2 · B 1/2`: how many of each arm's runs in a group passed their
/// check, else how many ran.
fn group_note(runs: &[usize], lanes: &[FlowLane]) -> String {
    let mut parts = Vec::new();
    for arm in 0..2 {
        let of_arm: Vec<_> = runs
            .iter()
            .filter_map(|&lane| lanes[lane].bench.as_ref())
            .filter(|bench| bench.arm == arm)
            .collect();
        if of_arm.is_empty() {
            continue;
        }
        let checked = of_arm.iter().filter(|b| b.check_passed.is_some()).count();
        let passed = of_arm
            .iter()
            .filter(|b| b.check_passed == Some(true))
            .count();
        parts.push(if checked > 0 {
            format!("{} {passed}/{checked}", arm_letter(arm))
        } else {
            format!("{} {}×", arm_letter(arm), of_arm.len())
        });
    }
    parts.join(" · ")
}

/// The number at the end of a run's name: 3 for `a-3`.
fn run_number(run: &str) -> u64 {
    run.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Lay the flow out as rows: each agent under the agent that spawned it,
/// a benchmark's runs by what they worked on, folded as `toggled` says
/// against each row's default. With `issues_only`, only rows that failed
/// or have a row under them that did.
#[must_use]
pub fn build_timeline(
    lanes: &[FlowLane],
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    toggled: &HashSet<String>,
    issues_only: bool,
) -> Timeline {
    let work = work_of_lanes(lanes, lane_index, nodes);
    let start_ms = work.iter().filter_map(|w| w.first).min().unwrap_or(0);
    let end_ms = work.iter().map(|w| w.last).max().unwrap_or(0).max(start_ms);
    let (children, roots) = lane_tree(lanes, lane_index);
    let (items, top) = items_of(&roots, lanes, &children);
    let (extent, issue) = roll_up(&items, &work, lanes);
    let tree = Tree {
        lanes,
        work: &work,
        items: &items,
        extent: &extent,
        issue: &issue,
    };
    Timeline {
        rows: tree.rows(&top, toggled, issues_only),
        start_ms,
        end_ms,
    }
}

/// What each lane did, from its nodes in the order they were recorded.
fn work_of_lanes(
    lanes: &[FlowLane],
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
) -> Vec<LaneWork> {
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }
    for list in &mut per_lane {
        list.sort_by_key(|&at| (nodes[at].seq, at));
    }
    per_lane.iter().map(|list| lane_work(list, nodes)).collect()
}

/// The lanes each lane spawned, and the lanes nothing spawned.
fn lane_tree(
    lanes: &[FlowLane],
    lane_index: &HashMap<String, usize>,
) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (lane, it) in lanes.iter().enumerate() {
        match it.parent.as_ref().and_then(|p| lane_index.get(p)) {
            Some(&parent) if parent != lane && lane != 0 => children[parent].push(lane),
            _ => roots.push(lane),
        }
    }
    (children, roots)
}

/// The tree of rows, before folds, and its top items.
fn items_of(
    roots: &[usize],
    lanes: &[FlowLane],
    children: &[Vec<usize>],
) -> (Vec<Item>, Vec<usize>) {
    let mut adder = ItemAdder {
        lanes,
        children,
        visited: vec![false; lanes.len()],
        items: Vec::new(),
    };
    let mut top: Vec<usize> = Vec::new();
    for &root in roots {
        if let Some(item) = adder.add_lane(root) {
            top.push(item);
        }
    }
    (adder.items, top)
}

/// Bottom up: what each item and the items under it did, and whether
/// any of them had an issue.
fn roll_up(
    items: &[Item],
    work: &[LaneWork],
    lanes: &[FlowLane],
) -> (Vec<Option<(u64, u64)>>, Vec<bool>) {
    let mut extent: Vec<Option<(u64, u64)>> = vec![None; items.len()];
    let mut issue = vec![false; items.len()];
    for at in (0..items.len()).rev() {
        let own = items[at].lane.map(|lane| own_extent(lane, work, lanes));
        let (mut span, mut bad) = own.unwrap_or((None, false));
        for &child in &items[at].children {
            bad |= issue[child];
            span = match (span, extent[child]) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                (a, b) => a.or(b),
            };
        }
        extent[at] = span;
        issue[at] = bad;
    }
    (extent, issue)
}

/// When lane `lane` itself worked, and whether it had an issue.
fn own_extent(lane: usize, work: &[LaneWork], lanes: &[FlowLane]) -> (Option<(u64, u64)>, bool) {
    let lane_issue = work[lane]
        .spans
        .iter()
        .any(|s| s.status == TraceNodeStatus::Error)
        || lanes[lane]
            .bench
            .as_ref()
            .is_some_and(|b| b.check_passed == Some(false) || b.error.is_some());
    let span = work[lane].first.map(|first| (first, work[lane].last));
    (span, lane_issue)
}

/// The tree of items with what each did, to lay out as rows.
struct Tree<'a> {
    lanes: &'a [FlowLane],
    work: &'a [LaneWork],
    items: &'a [Item],
    extent: &'a [Option<(u64, u64)>],
    issue: &'a [bool],
}

impl Tree<'_> {
    /// The rows shown, depth first, skipping what folds hide.
    fn rows(
        &self,
        top: &[usize],
        toggled: &HashSet<String>,
        issues_only: bool,
    ) -> Vec<TimelineRow> {
        let mut rows = Vec::new();
        let mut stack: Vec<(usize, usize)> = top.iter().rev().map(|&item| (item, 0)).collect();
        while let Some((at, depth)) = stack.pop() {
            if issues_only && !self.issue[at] {
                continue;
            }
            let item = &self.items[at];
            let folds = !item.children.is_empty();
            let folded = folds && (item.folded_by_default != toggled.contains(&item.key));
            rows.push(self.row(at, depth, folds, folded));
            if !folded {
                for &child in item.children.iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
        rows
    }

    fn row(&self, at: usize, depth: usize, folds: bool, folded: bool) -> TimelineRow {
        let item = &self.items[at];
        let (spans, mut facts, mut status, error) = match item.lane {
            Some(lane) => self.lane_row(lane),
            None => (Vec::new(), RowFacts::default(), TraceNodeStatus::Ok, None),
        };
        if item.lane.is_none() {
            self.add_up_runs(item, &mut facts);
            if self.issue[at] {
                status = TraceNodeStatus::Error;
            }
        }
        let hidden = if folded {
            self.hidden_under(item)
        } else {
            Vec::new()
        };
        TimelineRow {
            key: item.key.clone(),
            lane: item.lane,
            depth,
            name: item.name.clone(),
            note: item.note.clone(),
            spans,
            hidden,
            extent: self.extent[at],
            folds,
            folded,
            facts,
            status,
            error,
        }
    }

    /// The bars, facts, status and error of lane `lane`'s row.
    fn lane_row(&self, lane: usize) -> (Vec<TurnSpan>, RowFacts, TraceNodeStatus, Option<String>) {
        let work = &self.work[lane];
        let mut facts = work.facts;
        let mut status = work
            .spans
            .iter()
            .fold(TraceNodeStatus::Ok, |s, span| worse(s, span.status));
        let mut error = None;
        if let Some(bench) = &self.lanes[lane].bench {
            facts.cost_usd = Some(bench.cost_usd);
            if bench.wall_ms > 0 {
                facts.wall_ms = Some(bench.wall_ms);
            }
            facts.check = bench.check_passed;
            if bench.check_passed == Some(false) || bench.error.is_some() {
                status = TraceNodeStatus::Error;
            }
            error.clone_from(&bench.error);
        }
        (work.spans.clone(), facts, status, error)
    }

    /// A group adds up its runs.
    fn add_up_runs(&self, item: &Item, facts: &mut RowFacts) {
        for &child in &item.children {
            let Some(lane) = self.items[child].lane else {
                continue;
            };
            facts.turns += self.work[lane].facts.turns;
            facts.calls += self.work[lane].facts.calls;
            facts.errors += self.work[lane].facts.errors;
            if let Some(bench) = &self.lanes[lane].bench {
                facts.cost_usd = Some(facts.cost_usd.unwrap_or(0.0) + bench.cost_usd);
                facts.wall_ms = Some(facts.wall_ms.unwrap_or(0).max(bench.wall_ms));
            }
        }
    }

    /// The time the rows under a folded `item` worked, sorted.
    fn hidden_under(&self, item: &Item) -> Vec<(u64, u64)> {
        let mut hidden = Vec::new();
        let mut under: Vec<usize> = item.children.clone();
        while let Some(child) = under.pop() {
            if let Some(lane) = self.items[child].lane {
                hidden.extend(self.work[lane].spans.iter().map(|s| (s.start_ms, s.end_ms)));
            }
            under.extend(&self.items[child].children);
        }
        hidden.sort_unstable();
        hidden
    }
}

/// Builds the tree of items from the lanes, a lane at a time.
struct ItemAdder<'a> {
    lanes: &'a [FlowLane],
    children: &'a [Vec<usize>],
    visited: Vec<bool>,
    items: Vec<Item>,
}

impl<'a> ItemAdder<'a> {
    /// Add the item of lane `lane` and of every lane under it. A benchmark's
    /// arms give no row: their runs go under a row for each thing they
    /// worked on.
    fn add_lane(&mut self, lane: usize) -> Option<usize> {
        if std::mem::replace(&mut self.visited[lane], true) {
            return None;
        }
        let lanes = self.lanes;
        let it = &lanes[lane];
        let at = self.items.len();
        let note = it
            .bench
            .as_ref()
            .map(|bench| bench.arm_label.clone())
            .unwrap_or_default();
        self.items.push(Item {
            key: it.agent_id.clone(),
            lane: Some(lane),
            name: it.name.clone(),
            note,
            children: Vec::new(),
            folded_by_default: it.bench.is_some() && !self.children[lane].is_empty(),
        });
        let (mut runs, plain) = self.sort_children(lane);
        let mut kids: Vec<usize> = plain
            .into_iter()
            .filter_map(|child| self.add_lane(child))
            .collect();
        runs.sort_by_key(|&(_, bench)| (run_number(&bench.run), bench.arm));
        for (group, list) in group_runs(runs) {
            kids.push(self.add_group(&it.agent_id, group, list));
        }
        self.items[at].children = kids;
        Some(at)
    }

    /// The benchmark runs under lane `lane`, through its arms, and the
    /// other lanes under it.
    fn sort_children(&mut self, lane: usize) -> (Vec<(usize, &'a FlowBench)>, Vec<usize>) {
        let lanes = self.lanes;
        let it = &lanes[lane];
        let mut runs = Vec::new();
        let mut plain: Vec<usize> = Vec::new();
        for &child in &self.children[lane] {
            if lanes[child].bench_arm {
                self.visited[child] = true;
                for &grandchild in &self.children[child] {
                    match lanes[grandchild].bench.as_ref() {
                        Some(bench) => runs.push((grandchild, bench)),
                        None => plain.push(grandchild),
                    }
                }
            } else if let Some(bench) = lanes[child].bench.as_ref().filter(|_| it.bench.is_none()) {
                runs.push((child, bench));
            } else {
                plain.push(child);
            }
        }
        (runs, plain)
    }

    /// Add the row of a group of runs of agent `agent_id`, and the runs.
    fn add_group(&mut self, agent_id: &str, group: String, list: Vec<usize>) -> usize {
        let item = self.items.len();
        self.items.push(Item {
            key: format!("group:{agent_id}:{group}"),
            lane: None,
            note: group_note(&list, self.lanes),
            name: group,
            children: Vec::new(),
            folded_by_default: false,
        });
        let under: Vec<usize> = list
            .into_iter()
            .filter_map(|run| self.add_lane(run))
            .collect();
        self.items[item].children = under;
        item
    }
}

/// Runs by the thing they worked on, in the order they come.
fn group_runs(runs: Vec<(usize, &FlowBench)>) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (run, bench) in runs {
        let group = &bench.group;
        match groups.iter_mut().find(|(name, _)| name == group) {
            Some((_, list)) => list.push(run),
            None => groups.push((group.clone(), vec![run])),
        }
    }
    groups
}
