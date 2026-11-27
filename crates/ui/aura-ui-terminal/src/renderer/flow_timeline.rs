//! The TIME tab: an agent a row, its turns as bars over the
//! run's time, with what each row did against the right edge and what the
//! selection is under the rows.
//!
//! ```text
//!  agents                     0s        1m        2m             cost   time
//!  ▾ ✗ coding/fix-shell-bug A 2/2 · B 1/2  ────────────────     $0.41  2m40s
//!      ✓ a-1 gate              ┃━━◆━━━┃━━┄┄                     $0.08  1m02s ✓
//!      ✗ b-1 opus              ┃━━━━━━━━━━━━━━━✗━━━━            $0.21  2m40s ✗
//!   b-1 · opus · 3 turns · 41 calls · $0.21 · 2m40s · check failed
//!   turn 2 of 3 · run the tests again · 1m10s
//! ```
//!
//! The rows and the heads over their columns keep a list's column at the
//! left; the lines under them, and what an empty timeline says, keep the
//! side padding of a panel's text.

use super::chrome::{pad_sides, render_scrollbar, ScrollView};
use super::graph::status_glyph;
use super::text::display_width;
use crate::app::flow::timeline::{MarkKind, TimelineRow, TimelineView, TurnSpan};
use crate::app::format::format_cost;
use crate::components::truncate_record_info;
use crate::events::TraceNodeStatus;
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// Columns of the cost, time and check against the right edge.
const FACTS: u16 = 17;
/// Fewest columns between two time labels on the axis.
const TICK_ROOM: f64 = 10.0;

/// Draw the timeline in `inner`, the inside of the chat panel `area`
/// below its blank top row.
pub(super) fn render_timeline(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let footer: u16 = if inner.height >= 8 { 2 } else { 0 };
    let rows_shown = usize::from(inner.height.saturating_sub(1 + footer)).max(1);
    let label = (inner.width / 3)
        .clamp(16, 44)
        .min(inner.width.saturating_sub(10));
    let facts = if inner.width >= 70 { FACTS } else { 0 };
    let view = timeline_view(app, inner, rows_shown, label, facts);

    let state = app.flow().timeline();
    let mut draw = Draw {
        buffer: frame.buffer_mut(),
        theme,
        view,
    };
    draw.axis(inner, label, state.timeline().start_ms, facts > 0);
    draw.rows(app, inner, label, facts);
    if footer > 0 {
        draw.footer(app, inner);
    }

    let state = app.flow().timeline();
    render_scrollbar(
        frame,
        area,
        Rect {
            y: view.rows_top,
            height: u16::try_from(rows_shown).unwrap_or(u16::MAX),
            ..inner
        },
        ScrollView {
            total: state.timeline().rows.len(),
            visible: rows_shown,
            first: state.scroll(),
        },
        is_focused,
        theme,
    );
}

/// Follow the selection in `rows_shown` rows, store where the rows and
/// bars go, and return it.
fn timeline_view(
    app: &mut App,
    inner: Rect,
    rows_shown: usize,
    label: u16,
    facts: u16,
) -> TimelineView {
    let bar_x = inner.x + label + 1;
    let bar_width = inner
        .width
        .saturating_sub(label + 1 + facts + u16::from(facts > 0))
        .max(1);

    let flow = app.flow_mut();
    flow.ensure_timeline();
    flow.timeline_mut().follow(rows_shown);
    let window = flow.timeline().window();
    let view = TimelineView {
        area: inner,
        rows_top: inner.y + 1,
        rows_shown,
        bar_x,
        bar_width,
        window,
    };
    flow.timeline_mut().set_view(Some(view));
    view
}

/// What the selected row is and did, and what its selected turn is, or
/// whether only issues are listed when no turn is selected.
fn footer_lines(app: &App) -> (String, String) {
    let flow = app.flow();
    let state = flow.timeline();
    let Some(at) = state.selected_row() else {
        return (String::new(), String::new());
    };
    let row = &state.timeline().rows[at];
    (row_line(row), turn_line(app, row))
}

/// `b-1 · opus · 3 turns · 41 calls · $0.21 · 2m40s · check failed`.
fn row_line(row: &TimelineRow) -> String {
    let mut parts = vec![row.name.clone()];
    if !row.note.is_empty() {
        parts.push(row.note.clone());
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    parts.push(plural(row.facts.turns, "turn", "turns"));
    parts.push(plural(row.facts.calls, "call", "calls"));
    if let Some(cost) = row.facts.cost_usd {
        parts.push(format_cost(cost));
    }
    if let Some(ms) = row.facts.wall_ms {
        parts.push(short_ms(ms));
    }
    match row.facts.check {
        Some(true) => parts.push("check passed".to_string()),
        Some(false) => parts.push("check failed".to_string()),
        None => {}
    }
    if row.facts.errors > 0 {
        parts.push(plural(row.facts.errors, "error", "errors"));
    }
    if let Some(error) = &row.error {
        parts.push(error.clone());
    }
    parts.join(" · ")
}

/// The selected turn of `row`, or whether only issues are listed when no
/// turn is selected.
fn turn_line(app: &App, row: &TimelineRow) -> String {
    let flow = app.flow();
    let state = flow.timeline();
    match state.selected_span() {
        Some((_, span)) => {
            let turn = &row.spans[span];
            let head = &flow.nodes()[turn.head];
            format!(
                "turn {} of {} · {} · {}",
                span + 1,
                row.spans.len(),
                head.label,
                short_ms(turn.end_ms.saturating_sub(turn.start_ms)),
            )
        }
        None if state.issues_only() => "issues only".to_string(),
        None => String::new(),
    }
}

/// `850ms`, `42s`, `3m05s`, `1h02m`.
fn short_ms(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{}s", ms / 1_000),
        60_000..3_600_000 => format!("{}m{:02}s", ms / 60_000, ms % 60_000 / 1_000),
        _ => format!("{}h{:02}m", ms / 3_600_000, ms % 3_600_000 / 60_000),
    }
}

struct Draw<'a> {
    buffer: &'a mut Buffer,
    theme: &'a Theme,
    view: TimelineView,
}

impl Draw<'_> {
    fn muted(&self) -> Style {
        Style::default().fg(self.theme.colors.muted)
    }

    fn selection(&self) -> Style {
        Style::default()
            .bg(self.theme.colors.selection)
            .fg(self.theme.colors.on_selection)
            .add_modifier(Modifier::BOLD)
    }

    fn put(&mut self, x: u16, y: u16, symbol: &str, style: Style) {
        if let Some(cell) = self.buffer.cell_mut((x, y)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }

    /// `text` from `(x, y)`, in at most `width` columns. Returns the
    /// column after it.
    fn text(&mut self, x: u16, y: u16, text: &str, width: u16, style: Style) -> u16 {
        let mut at = x;
        let end = x.saturating_add(width);
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let w = u16::try_from(c.width().unwrap_or(0)).unwrap_or(0);
            if w == 0 {
                continue;
            }
            if at + w > end {
                break;
            }
            self.put(at, y, c.encode_utf8(&mut buf), style);
            if w == 2 {
                self.put(at + 1, y, " ", style);
            }
            at += w;
        }
        at
    }

    /// The bar column time `at_ms` is in, which may be off the bars.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn column(&self, at_ms: u64) -> i64 {
        let (from, to) = self.view.window;
        ((at_ms as f64 - from) / (to - from) * f64::from(self.view.bar_width)).floor() as i64
    }

    /// The rows in view, or what an empty timeline says.
    fn rows(&mut self, app: &App, inner: Rect, label: u16, facts: u16) {
        let view = self.view;
        let state = app.flow().timeline();
        let timeline = state.timeline();
        let selected = state.selected_row();
        let span = state.selected_span().map(|(_, span)| span);
        for (at, row) in timeline
            .rows
            .iter()
            .enumerate()
            .skip(state.scroll())
            .take(view.rows_shown)
        {
            let y = view.rows_top + u16::try_from(at - state.scroll()).unwrap_or(u16::MAX);
            let is_selected = selected == Some(at);
            self.label(inner.x, y, label, row, is_selected);
            self.bars(y, row, is_selected.then_some(span).flatten());
            if facts > 0 {
                self.facts(inner.right() - FACTS, y, row);
            }
        }
        if timeline.rows.is_empty() {
            let text = if state.issues_only() {
                "Nothing failed."
            } else {
                "Nothing recorded yet."
            };
            let room = pad_sides(inner);
            self.text(room.x, view.rows_top, text, room.width, self.muted());
        }
    }

    /// The two lines under the rows: see [`footer_lines`].
    fn footer(&mut self, app: &App, inner: Rect) {
        let (first, second) = footer_lines(app);
        let y = inner.bottom() - 2;
        let room = pad_sides(inner);
        let style = Style::default().fg(self.theme.colors.foreground);
        self.text(room.x, y, &first, room.width, style);
        self.text(room.x, y + 1, &second, room.width, self.muted());
    }

    /// The header: `agents`, the time from the run's start along the bars,
    /// and the names of the facts.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn axis(&mut self, inner: Rect, label: u16, start_ms: u64, facts: bool) {
        let muted = self.muted();
        self.text(inner.x + 1, inner.y, "agents", label, muted);
        let (from, to) = self.view.window;
        let per_column = (to - from) / f64::from(self.view.bar_width);
        let want = per_column * TICK_ROOM;
        let step = [
            10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1e3, 2e3, 5e3, 1e4, 15e3, 3e4, 6e4, 12e4, 3e5,
            6e5, 9e5, 18e5, 36e5, 72e5, 216e5,
        ]
        .into_iter()
        .find(|&step| step >= want)
        .unwrap_or(want.max(1.0));
        let start = start_ms as f64;
        let mut tick = ((from - start) / step).ceil() * step;
        while start + tick < to {
            let column = self.column((start + tick) as u64);
            if column >= 0 {
                let x = self.view.bar_x + column as u16;
                let text = short_ms(tick as u64);
                let width = u16::try_from(display_width(&text)).unwrap_or(0);
                if x + width <= self.view.bar_x + self.view.bar_width {
                    self.text(x, inner.y, &text, width, muted);
                }
            }
            tick += step;
        }
        if facts {
            let x = inner.right() - FACTS;
            self.text(x, inner.y, "   cost   time  ", FACTS, muted);
        }
    }

    /// `  ▾ ✓ a-1 gate`: indent, fold marker, status, name and note.
    fn label(&mut self, x: u16, y: u16, width: u16, row: &TimelineRow, selected: bool) {
        let fill = if selected {
            self.selection()
        } else {
            Style::default()
        };
        for column in x..x + width {
            self.put(column, y, " ", fill);
        }
        let indent = u16::try_from(row.depth * 2)
            .unwrap_or(u16::MAX)
            .min(width / 2);
        let mut at = x + 1 + indent;
        let end = x + width;
        if row.folds {
            let marker = if row.folded { "▸" } else { "▾" };
            let style = if selected {
                fill
            } else {
                Style::default().fg(self.theme.colors.secondary)
            };
            at = self.text(at, y, marker, end.saturating_sub(at), style) + 1;
        } else {
            at += 2;
        }
        let (glyph, color) = status_glyph(row.status, self.theme);
        let style = if selected {
            fill
        } else {
            Style::default().fg(color)
        };
        at = self.text(at, y, glyph, end.saturating_sub(at), style) + 1;
        let name_style = if selected {
            fill
        } else if row.lane.is_none() {
            Style::default()
                .fg(self.theme.colors.secondary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.colors.foreground)
        };
        let room = end.saturating_sub(at + 1);
        let name = truncate_record_info(&row.name, usize::from(room));
        at = self.text(at, y, &name, room, name_style) + 1;
        if !row.note.is_empty() && at + 3 < end {
            let room = end.saturating_sub(at + 1);
            let note = truncate_record_info(&row.note, usize::from(room));
            let style = if selected { fill } else { self.muted() };
            self.text(at, y, &note, room, style);
        }
    }

    /// The cell of bar column `column`, which is on the bars.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn bar_cell(&self, column: i64) -> u16 {
        self.view.bar_x + column as u16
    }

    /// The bar columns from `from` to `to`, cut to the bars.
    fn cells(&self, from: i64, to: i64) -> Range<i64> {
        from.max(0)..(to + 1).min(i64::from(self.view.bar_width))
    }

    /// `symbol` in every bar cell from time `start` to `end`.
    fn fill(&mut self, y: u16, (start, end): (u64, u64), symbol: &str, style: Style) {
        for column in self.cells(self.column(start), self.column(end)) {
            self.put(self.bar_cell(column), y, symbol, style);
        }
    }

    /// The row's bars: its turns, the time of the rows its fold hides,
    /// and a group's extent.
    fn bars(&mut self, y: u16, row: &TimelineRow, selected_span: Option<usize>) {
        let muted = self.muted();
        if row.lane.is_none() && !row.folded {
            if let Some(extent) = row.extent {
                self.fill(y, extent, "─", muted);
            }
        }
        for &hidden in &row.hidden {
            self.fill(y, hidden, "┄", muted);
        }
        for (index, span) in row.spans.iter().enumerate() {
            self.turn(y, span, selected_span == Some(index));
        }
        // Errors win a cell over everything else in it.
        let width = i64::from(self.view.bar_width);
        for span in &row.spans {
            for mark in span.marks.iter().filter(|m| m.kind == MarkKind::Error) {
                let column = self.column(mark.at_ms);
                if (0..width).contains(&column) {
                    let style = Style::default().fg(self.theme.colors.error);
                    self.put(self.bar_cell(column), y, "✗", style);
                }
            }
        }
    }

    /// One turn's bar and its marks.
    fn turn(&mut self, y: u16, span: &TurnSpan, selected: bool) {
        let width = i64::from(self.view.bar_width);
        let color = match span.status {
            TraceNodeStatus::Error => self.theme.colors.error,
            TraceNodeStatus::Pending | TraceNodeStatus::Warn => self.theme.colors.warning,
            TraceNodeStatus::Ok => self.theme.colors.secondary,
        };
        let style = if selected {
            self.selection()
        } else {
            Style::default().fg(color)
        };
        let first = self.column(span.start_ms);
        let last = self.column(span.end_ms).max(first);
        for column in self.cells(first, last) {
            let symbol = if column == first { "┃" } else { "━" };
            self.put(self.bar_cell(column), y, symbol, style);
        }
        for mark in &span.marks {
            let column = self.column(mark.at_ms);
            if !(0..width).contains(&column) {
                continue;
            }
            let (symbol, color) = match mark.kind {
                MarkKind::Gate => ("◆", self.theme.colors.primary),
                MarkKind::Judge => ("◇", self.theme.colors.warning),
                MarkKind::Error => ("✗", self.theme.colors.error),
            };
            let style = if selected {
                self.selection()
            } else {
                Style::default().fg(color)
            };
            self.put(self.bar_cell(column), y, symbol, style);
        }
    }

    /// `  $0.21  2m40s ✗`: cost, time and a benchmark run's check.
    fn facts(&mut self, x: u16, y: u16, row: &TimelineRow) {
        let muted = self.muted();
        let cost = row.facts.cost_usd.map(format_cost).unwrap_or_default();
        let time = row.facts.wall_ms.map(short_ms).unwrap_or_default();
        self.text(x, y, &format!("{cost:>7} {time:>6} "), FACTS, muted);
        let check = match row.facts.check {
            Some(true) => Some(("✓", self.theme.colors.success)),
            Some(false) => Some(("✗", self.theme.colors.error)),
            None => None,
        };
        if let Some((glyph, color)) = check {
            self.put(x + FACTS - 2, y, glyph, Style::default().fg(color));
        }
    }
}
