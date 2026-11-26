//! The TIME tab: what happened, an agent a row, before the GRAPH tab's
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

mod build;
mod lane_work;

pub use build::build_timeline;

use crate::events::{FlowLane, FlowNode, TraceNodeStatus};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};

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
        let marker = view.area.x + u16::try_from(it.depth * 2).unwrap_or(u16::MAX);
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
