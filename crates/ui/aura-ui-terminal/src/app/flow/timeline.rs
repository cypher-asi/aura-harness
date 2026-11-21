//! The GRAPH tab's timeline: what happened, an agent a row, before the
//! cards of any one turn.
//!
//! Each row is an agent, under the agent that spawned it; its turns are
//! bars over the run's time, marked where Gate routed, the judge reviewed
//! or something failed. A benchmark's runs are listed by what they worked
//! on instead of by arm, so the runs that went head to head are next to
//! each other under a row of their own:
//!
//! ```text
//!   ▾ ✗ coding/fix-shell-bug A 2/2 · B 1/2  ─────────────────
//!       ✓ a-1 gate                          ┃━━◆━━━┃━━┅┅┅
//!       ✗ b-1 opus                          ┃━━━━━━━━━━━━✗━━
//! ```
//!
//! Folding a row hides the rows under it; their time shows on it faintly.
//! A run that spawned agents starts folded.

use crate::events::{arm_letter, FlowKind, FlowLane, FlowNode, TraceNodeStatus};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};

/// Rows a flow has above which the GRAPH tab opens on its timeline.
pub const TIMELINE_LANES: usize = 4;

/// What a mark on a bar stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Gate asked what the prompt is
    Gate,
    /// Gate's judge reviewed the work
    Judge,
    /// Something failed: a model call, a tool call, a process
    Error,
}

/// A mark on a bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub at_ms: u64,
    pub kind: MarkKind,
}

/// One turn, as a bar.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Index into the flow's nodes of the node the turn starts at
    pub head: usize,
    pub status: TraceNodeStatus,
    pub marks: Vec<Mark>,
    /// Model calls in it
    pub calls: usize,
}

/// What a row says against the right edge.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RowFacts {
    pub turns: usize,
    pub calls: usize,
    pub cost_usd: Option<f64>,
    pub wall_ms: Option<u64>,
    /// Whether a benchmark run's check passed
    pub check: Option<bool>,
    /// Nodes that failed
    pub errors: usize,
}

/// One row of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRow {
    /// What its fold and the selection are kept by: the agent's hex id,
    /// or `group:` and the benchmark's agent and the group
    pub key: String,
    /// Index into the flow's lanes; `None` for the row of a group of
    /// benchmark runs
    pub lane: Option<usize>,
    pub depth: usize,
    pub name: String,
    /// What follows the name, muted: the arm a run is on, or how a
    /// group's runs did
    pub note: String,
    pub spans: Vec<TurnSpan>,
    /// Time the rows its fold hides worked
    pub hidden: Vec<(u64, u64)>,
    /// From the first thing it or a row under it did to the last
    pub extent: Option<(u64, u64)>,
    /// Whether rows go under it
    pub folds: bool,
    pub folded: bool,
    pub facts: RowFacts,
    pub status: TraceNodeStatus,
    /// Error the benchmark gave for the run, if it did not finish
    pub error: Option<String>,
}

/// The timeline of a flow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timeline {
    pub rows: Vec<TimelineRow>,
    /// When the first and last things in the flow happened
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Timeline {
    /// The row kept by `key`.
    #[must_use]
    pub fn row_of(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.key == key)
    }
}

/// A row before the folds are applied.
struct Item {
    key: String,
    lane: Option<usize>,
    name: String,
    note: String,
    children: Vec<usize>,
    folded_by_default: bool,
}

/// What a lane did, from its nodes.
#[derive(Default)]
struct LaneWork {
    spans: Vec<TurnSpan>,
    facts: RowFacts,
    first: Option<u64>,
    last: u64,
}

fn lane_work(list: &[usize], nodes: &[FlowNode]) -> LaneWork {
    let mut work = LaneWork::default();
    // Gate classifies a prompt before the prompt is recorded.
    let mut routing: Vec<usize> = Vec::new();
    let mut cost: Option<f64> = None;
    let add = |span: &mut TurnSpan, node: &FlowNode| {
        span.end_ms = span.end_ms.max(node.at_ms);
        match node.kind {
            FlowKind::Gate => span.marks.push(Mark {
                at_ms: node.at_ms,
                kind: MarkKind::Gate,
            }),
            FlowKind::Judge => span.marks.push(Mark {
                at_ms: node.at_ms,
                kind: MarkKind::Judge,
            }),
            _ => {}
        }
        if node.status == TraceNodeStatus::Error {
            span.marks.push(Mark {
                at_ms: node.at_ms,
                kind: MarkKind::Error,
            });
            if node.kind.is_model_call() {
                span.status = TraceNodeStatus::Error;
            }
        }
        if node.kind.is_model_call() {
            span.calls += 1;
        }
    };
    for &at in list {
        let node = &nodes[at];
        work.first = Some(work.first.map_or(node.at_ms, |first| first.min(node.at_ms)));
        work.last = work.last.max(node.at_ms);
        if node.status == TraceNodeStatus::Error {
            work.facts.errors += 1;
        }
        if let Some(spent) = node.usage.and_then(|usage| usage.cost_usd) {
            cost = Some(cost.unwrap_or(0.0) + spent);
        }
        if node.kind == FlowKind::Gate {
            routing.push(at);
            continue;
        }
        if node.kind == FlowKind::Prompt || work.spans.is_empty() {
            let start = routing
                .first()
                .map_or(node.at_ms, |&gate| nodes[gate].at_ms.min(node.at_ms));
            let status = match node.kind {
                FlowKind::Prompt => node.status,
                _ => TraceNodeStatus::Ok,
            };
            let mut span = TurnSpan {
                start_ms: start,
                end_ms: node.at_ms,
                head: at,
                status,
                marks: Vec::new(),
                calls: 0,
            };
            for gate in routing.drain(..) {
                add(&mut span, &nodes[gate]);
            }
            add(&mut span, node);
            work.spans.push(span);
            continue;
        }
        let span = work.spans.last_mut().expect("a turn was started");
        for gate in routing.drain(..) {
            add(span, &nodes[gate]);
        }
        add(span, node);
    }
    if let Some(&head) = routing.first() {
        if work.spans.is_empty() {
            work.spans.push(TurnSpan {
                start_ms: nodes[head].at_ms,
                end_ms: nodes[head].at_ms,
                head,
                status: TraceNodeStatus::Ok,
                marks: Vec::new(),
                calls: 0,
            });
        }
        let span = work.spans.last_mut().expect("a turn was started");
        for gate in routing {
            add(span, &nodes[gate]);
        }
    }
    work.facts.turns = work.spans.len();
    work.facts.calls = work.spans.iter().map(|span| span.calls).sum();
    work.facts.cost_usd = cost;
    work.facts.wall_ms = work.first.map(|first| work.last - first);
    work
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
#[allow(clippy::too_many_lines)]
pub fn build_timeline(
    lanes: &[FlowLane],
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    toggled: &HashSet<String>,
    issues_only: bool,
) -> Timeline {
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }
    for list in &mut per_lane {
        list.sort_by_key(|&at| (nodes[at].seq, at));
    }
    let work: Vec<LaneWork> = per_lane.iter().map(|list| lane_work(list, nodes)).collect();
    let start_ms = work.iter().filter_map(|w| w.first).min().unwrap_or(0);
    let end_ms = work.iter().map(|w| w.last).max().unwrap_or(0).max(start_ms);

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (lane, it) in lanes.iter().enumerate() {
        match it.parent.as_ref().and_then(|p| lane_index.get(p)) {
            Some(&parent) if parent != lane && lane != 0 => children[parent].push(lane),
            _ => roots.push(lane),
        }
    }

    // The tree of rows, before folds.
    let mut items: Vec<Item> = Vec::new();
    let mut visited = vec![false; lanes.len()];
    let mut top: Vec<usize> = Vec::new();
    for &root in &roots {
        if let Some(item) = add_lane(root, lanes, &children, &mut visited, &mut items) {
            top.push(item);
        }
    }

    // Bottom up: what each item and the items under it did.
    let mut extent: Vec<Option<(u64, u64)>> = vec![None; items.len()];
    let mut issue = vec![false; items.len()];
    for at in (0..items.len()).rev() {
        let own = items[at].lane.map(|lane| {
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
        });
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

    let mut rows = Vec::new();
    let mut stack: Vec<(usize, usize)> = top.iter().rev().map(|&item| (item, 0)).collect();
    while let Some((at, depth)) = stack.pop() {
        if issues_only && !issue[at] {
            continue;
        }
        let item = &items[at];
        let folds = !item.children.is_empty();
        let folded = folds && (item.folded_by_default != toggled.contains(&item.key));
        let (spans, mut facts, mut status, error) = match item.lane {
            Some(lane) => {
                let mut facts = work[lane].facts;
                let mut status = work[lane]
                    .spans
                    .iter()
                    .fold(TraceNodeStatus::Ok, |s, span| worse(s, span.status));
                let mut error = None;
                if let Some(bench) = &lanes[lane].bench {
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
                (work[lane].spans.clone(), facts, status, error)
            }
            None => (Vec::new(), RowFacts::default(), TraceNodeStatus::Ok, None),
        };
        if item.lane.is_none() {
            // A group adds up its runs.
            for &child in &item.children {
                let Some(lane) = items[child].lane else {
                    continue;
                };
                facts.turns += work[lane].facts.turns;
                facts.calls += work[lane].facts.calls;
                facts.errors += work[lane].facts.errors;
                if let Some(bench) = &lanes[lane].bench {
                    facts.cost_usd = Some(facts.cost_usd.unwrap_or(0.0) + bench.cost_usd);
                    facts.wall_ms = Some(facts.wall_ms.unwrap_or(0).max(bench.wall_ms));
                }
            }
            if issue[at] {
                status = TraceNodeStatus::Error;
            }
        }
        let mut hidden = Vec::new();
        if folded {
            let mut under: Vec<usize> = item.children.clone();
            while let Some(child) = under.pop() {
                if let Some(lane) = items[child].lane {
                    hidden.extend(work[lane].spans.iter().map(|s| (s.start_ms, s.end_ms)));
                }
                under.extend(&items[child].children);
            }
            hidden.sort_unstable();
        }
        rows.push(TimelineRow {
            key: item.key.clone(),
            lane: item.lane,
            depth,
            name: item.name.clone(),
            note: item.note.clone(),
            spans,
            hidden,
            extent: extent[at],
            folds,
            folded,
            facts,
            status,
            error,
        });
        if !folded {
            for &child in item.children.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    Timeline {
        rows,
        start_ms,
        end_ms,
    }
}

/// Add the item of lane `lane` and of every lane under it. A benchmark's
/// arms give no row: their runs go under a row for each thing they
/// worked on.
fn add_lane(
    lane: usize,
    lanes: &[FlowLane],
    children: &[Vec<usize>],
    visited: &mut [bool],
    items: &mut Vec<Item>,
) -> Option<usize> {
    if std::mem::replace(&mut visited[lane], true) {
        return None;
    }
    let it = &lanes[lane];
    let at = items.len();
    let note = it
        .bench
        .as_ref()
        .map(|bench| bench.arm_label.clone())
        .unwrap_or_default();
    items.push(Item {
        key: it.agent_id.clone(),
        lane: Some(lane),
        name: it.name.clone(),
        note,
        children: Vec::new(),
        folded_by_default: it.bench.is_some() && !children[lane].is_empty(),
    });
    let mut runs: Vec<usize> = Vec::new();
    let mut plain: Vec<usize> = Vec::new();
    for &child in &children[lane] {
        if lanes[child].bench_arm {
            visited[child] = true;
            for &grandchild in &children[child] {
                if lanes[grandchild].bench.is_some() {
                    runs.push(grandchild);
                } else {
                    plain.push(grandchild);
                }
            }
        } else if lanes[child].bench.is_some() && it.bench.is_none() {
            runs.push(child);
        } else {
            plain.push(child);
        }
    }
    let mut kids: Vec<usize> = plain
        .into_iter()
        .filter_map(|child| add_lane(child, lanes, children, visited, items))
        .collect();
    let bench = |lane: usize| lanes[lane].bench.as_ref().expect("a run");
    runs.sort_by_key(|&run| (run_number(&bench(run).run), bench(run).arm));
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for run in runs {
        let group = &bench(run).group;
        match groups.iter_mut().find(|(name, _)| name == group) {
            Some((_, list)) => list.push(run),
            None => groups.push((group.clone(), vec![run])),
        }
    }
    for (group, list) in groups {
        let item = items.len();
        items.push(Item {
            key: format!("group:{}:{group}", it.agent_id),
            lane: None,
            note: group_note(&list, lanes),
            name: group,
            children: Vec::new(),
            folded_by_default: false,
        });
        let under: Vec<usize> = list
            .into_iter()
            .filter_map(|run| add_lane(run, lanes, children, visited, items))
            .collect();
        items[item].children = under;
        kids.push(item);
    }
    items[at].children = kids;
    Some(at)
}

/// Where the timeline was drawn in the last frame, for the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineView {
    pub area: Rect,
    /// Screen row of the first row shown
    pub rows_top: u16,
    pub rows_shown: usize,
    /// Screen column the bars start at, and how many columns they have
    pub bar_x: u16,
    pub bar_width: u16,
    /// The time the bars show, in Unix ms
    pub window: (f64, f64),
}

impl TimelineView {
    /// The time at the middle of screen column `column`.
    #[must_use]
    pub fn time_at(&self, column: u16) -> Option<f64> {
        let into = column.checked_sub(self.bar_x)?;
        (into < self.bar_width).then(|| {
            let (from, to) = self.window;
            from + (f64::from(into) + 0.5) / f64::from(self.bar_width) * (to - from)
        })
    }
}

/// The timeline, and where in it the user is.
#[derive(Debug, Default)]
pub struct TimelineState {
    pub(super) timeline: Timeline,
    pub(super) stale: bool,
    /// Rows whose fold is the other way from their default, by key
    pub(super) toggled: HashSet<String>,
    pub(super) issues_only: bool,
    /// Key of the selected row
    pub(super) selected: Option<String>,
    /// The selected turn of that row
    pub(super) span: Option<usize>,
    /// First row shown
    pub(super) scroll: usize,
    /// How many times the run's time the bars are wide; 1 or more
    pub(super) zoom: f64,
    /// Where the bars start, as a share of the run's time
    pub(super) from: f64,
    pub(super) view: Option<TimelineView>,
}

/// The longest zoom of the timeline, in times the run's length.
const MAX_ZOOM: f64 = 64.0;
/// What one press of `+` or `-` zooms the timeline by.
pub(super) const ZOOM_STEP: f64 = 1.5;

impl TimelineState {
    /// The timeline as of the last rebuild.
    #[must_use]
    pub const fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// The selected row.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        self.timeline.row_of(self.selected.as_deref()?)
    }

    /// The selected turn of the selected row.
    #[must_use]
    pub fn selected_span(&self) -> Option<(usize, usize)> {
        let row = self.selected_row()?;
        let span = self.span?;
        (span < self.timeline.rows[row].spans.len()).then_some((row, span))
    }

    /// First row shown.
    #[must_use]
    pub const fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether only rows with problems are shown.
    #[must_use]
    pub const fn issues_only(&self) -> bool {
        self.issues_only
    }

    fn zoom(&self) -> f64 {
        self.zoom.max(1.0)
    }

    /// The time the bars show, in Unix ms.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn window(&self) -> (f64, f64) {
        let start = self.timeline.start_ms as f64;
        let length = (self.timeline.end_ms as f64 - start).max(1.0);
        let shown = length / self.zoom();
        let from = start + self.from.clamp(0.0, 1.0 - 1.0 / self.zoom()) * length;
        (from, from + shown)
    }

    pub(super) fn rebuild(
        &mut self,
        lanes: &[FlowLane],
        lane_index: &HashMap<String, usize>,
        nodes: &[FlowNode],
    ) {
        if !self.stale {
            return;
        }
        self.stale = false;
        self.timeline = build_timeline(lanes, lane_index, nodes, &self.toggled, self.issues_only);
        if self.selected_row().is_none() {
            self.selected = self.timeline.rows.first().map(|row| row.key.clone());
            self.span = None;
        }
    }

    pub(super) fn select_row(&mut self, row: usize) {
        let Some(it) = self.timeline.rows.get(row) else {
            return;
        };
        // Keep to about the same time on the new row.
        let at = self
            .selected_span()
            .map(|(r, s)| self.timeline.rows[r].spans[s].start_ms);
        self.span = match at {
            Some(at) if !it.spans.is_empty() => it
                .spans
                .iter()
                .enumerate()
                .min_by_key(|(_, span)| {
                    if (span.start_ms..=span.end_ms).contains(&at) {
                        0
                    } else {
                        span.start_ms.abs_diff(at).min(span.end_ms.abs_diff(at))
                    }
                })
                .map(|(index, _)| index),
            _ => None,
        };
        self.selected = Some(it.key.clone());
        self.show_span();
    }

    pub(super) fn move_row(&mut self, by: isize) {
        let count = self.timeline.rows.len();
        if count == 0 {
            return;
        }
        let row = self.selected_row().unwrap_or(0);
        let to = row.saturating_add_signed(by).min(count - 1);
        self.select_row(to);
    }

    pub(super) fn move_span(&mut self, forward: bool) {
        let Some(row) = self.selected_row() else {
            return self.move_row(0);
        };
        let spans = self.timeline.rows[row].spans.len();
        if spans == 0 {
            return;
        }
        self.span = Some(match (self.span, forward) {
            (None, true) => 0,
            (None, false) => spans - 1,
            (Some(at), true) => (at + 1).min(spans - 1),
            (Some(at), false) => at.saturating_sub(1),
        });
        self.show_span();
    }

    pub(super) fn select_span(&mut self, row: usize, span: Option<usize>) {
        if let Some(it) = self.timeline.rows.get(row) {
            self.selected = Some(it.key.clone());
            self.span = span.filter(|&s| s < it.spans.len());
        }
    }

    /// Fold or unfold row `row`.
    pub(super) fn toggle(&mut self, row: usize) {
        let Some(it) = self.timeline.rows.get(row).filter(|it| it.folds) else {
            return;
        };
        let key = it.key.clone();
        if !self.toggled.remove(&key) {
            self.toggled.insert(key.clone());
        }
        self.selected = Some(key);
        self.span = None;
        self.stale = true;
    }

    /// Fold every row that folds, or unfold them all when all are folded.
    pub(super) fn toggle_all(&mut self) {
        let rows: Vec<(String, bool)> = self
            .timeline
            .rows
            .iter()
            .filter(|row| row.folds && row.depth > 0)
            .map(|row| (row.key.clone(), row.folded))
            .collect();
        let fold = !rows.iter().all(|(_, folded)| *folded);
        for (key, folded) in rows {
            if folded != fold && !self.toggled.remove(&key) {
                self.toggled.insert(key);
            }
        }
        self.stale = true;
    }

    pub(super) fn toggle_issues(&mut self) {
        self.issues_only = !self.issues_only;
        self.stale = true;
    }

    /// Zoom the time by `factor` around the selected turn, else the
    /// middle; `None` fits the whole run.
    pub(super) fn zoom_by(&mut self, factor: Option<f64>) {
        let Some(factor) = factor else {
            self.zoom = 1.0;
            self.from = 0.0;
            return;
        };
        let (from, to) = self.window();
        #[allow(clippy::cast_precision_loss)]
        let centre = self
            .selected_span()
            .map_or((from + to) / 2.0, |(row, span)| {
                let span = &self.timeline.rows[row].spans[span];
                (span.start_ms as f64 + span.end_ms as f64) / 2.0
            });
        self.zoom = (self.zoom() * factor).clamp(1.0, MAX_ZOOM);
        self.centre_on(centre);
    }

    #[allow(clippy::cast_precision_loss)]
    fn centre_on(&mut self, at: f64) {
        let start = self.timeline.start_ms as f64;
        let length = (self.timeline.end_ms as f64 - start).max(1.0);
        let half = 0.5 / self.zoom();
        self.from = ((at - start) / length - half).clamp(0.0, 1.0 - 2.0 * half);
    }

    /// Move the time shown by `share` of what is shown.
    pub(super) fn pan(&mut self, share: f64) {
        let most = 1.0 - 1.0 / self.zoom();
        self.from = (self.from + share / self.zoom()).clamp(0.0, most.max(0.0));
    }

    /// Bring the selected turn's start into the time shown.
    #[allow(clippy::cast_precision_loss)]
    fn show_span(&mut self) {
        let Some((row, span)) = self.selected_span() else {
            return;
        };
        let span = &self.timeline.rows[row].spans[span];
        let (from, to) = self.window();
        let (start, end) = (span.start_ms as f64, span.end_ms as f64);
        if start < from || end > to {
            self.centre_on((start + end) / 2.0);
        }
    }

    /// Keep the selected row among the `shown` rows on screen.
    pub(crate) fn follow(&mut self, shown: usize) {
        let count = self.timeline.rows.len();
        let shown = shown.max(1);
        if let Some(row) = self.selected_row() {
            if row < self.scroll {
                self.scroll = row;
            } else if row >= self.scroll + shown {
                self.scroll = row + 1 - shown;
            }
        }
        self.scroll = self.scroll.min(count.saturating_sub(shown));
    }

    pub(super) fn scroll_by(&mut self, rows: isize) {
        let count = self.timeline.rows.len();
        let shown = self.view.map_or(1, |view| view.rows_shown);
        self.scroll = self
            .scroll
            .saturating_add_signed(rows)
            .min(count.saturating_sub(shown));
        // The selection stays where it is, so `follow` must not undo this.
        if let Some(row) = self.selected_row() {
            if row < self.scroll || row >= self.scroll + shown {
                let to = row.clamp(self.scroll, self.scroll + shown - 1);
                self.select_row(to.min(count.saturating_sub(1)));
            }
        }
    }

    pub(crate) fn set_view(&mut self, view: Option<TimelineView>) {
        self.view = view;
    }

    /// The row and turn at screen cell `(column, row)`; the turn is `None`
    /// off every bar. `Err` is a row's fold marker.
    pub(super) fn spot_at(
        &self,
        column: u16,
        row: u16,
    ) -> Option<Result<(usize, Option<usize>), usize>> {
        let view = self.view?;
        let into = row.checked_sub(view.rows_top)?;
        if usize::from(into) >= view.rows_shown {
            return None;
        }
        let index = self.scroll + usize::from(into);
        let it = self.timeline.rows.get(index)?;
        let marker = view.area.x + 1 + u16::try_from(it.depth * 2).unwrap_or(u16::MAX);
        if it.folds && (marker..=marker + 1).contains(&column) {
            return Some(Err(index));
        }
        let Some(at) = view.time_at(column) else {
            return Some(Ok((index, None)));
        };
        // A bar a cell wide is hit anywhere in its cell.
        let slack = (view.window.1 - view.window.0) / f64::from(view.bar_width.max(1));
        #[allow(clippy::cast_precision_loss)]
        let span = it.spans.iter().position(|span| {
            at >= span.start_ms as f64 - slack && at <= span.end_ms as f64 + slack
        });
        Some(Ok((index, span)))
    }
}
