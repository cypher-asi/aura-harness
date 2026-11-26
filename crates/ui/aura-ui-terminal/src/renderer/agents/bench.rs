//! The agents panel's BENCH tab: every benchmark, newest first, one row
//! each with its name, what it is about and how long ago it started, and
//! "+ New bench" pinned to the bottom row:
//!
//! ```text
//!  ⠋  Compare #3  fix the parser    2m
//!  ✓  Compare #2  add retries       3h
//!
//!  +  New bench
//! ```
//!
//! What a row shows, a benchmark's numbers or the form, is in the chat
//! panel. While the panel is focused the keyboard's row is lit; otherwise
//! the row of what the chat panel shows is.

use super::{draw_list, split_pinned, working_icon};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{
    cell_style, format_ago, list_row, list_row_with_detail, now_ms, CellState, StatusIcon,
};
use crate::events::{BenchStatus, BenchView};
use crate::renderer::chrome::ScrollView;
use crate::renderer::side_list::empty_line;
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

/// Draw the BENCH tab into `body`, the inside of the panel bordered by
/// `area`.
pub(super) fn render_bench(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let (rows_area, bottom_row) = split_pinned(body);
    let visible = usize::from(rows_area.height);
    app.set_bench_rows_area(rows_area);
    app.set_new_bench_area(bottom_row);
    app.sync_bench_list_scroll(visible);

    let width = usize::from(body.width);
    let lit = if is_focused {
        Some(app.selected_bench())
    } else {
        app.shown_bench_row()
    };
    let first = app.bench_list_scroll();
    let now = now_ms();
    let benches = &app.bench().benches;
    let lines: Vec<Line> = if benches.is_empty() {
        let text = if app.bench().loaded() {
            "No benchmarks yet."
        } else {
            "Loading…"
        };
        vec![empty_line(text, body.width, theme)]
    } else {
        benches
            .iter()
            .enumerate()
            .skip(first)
            .take(visible)
            .map(|(index, view)| {
                let state = row_state(app, index, lit == Some(index));
                bench_row(app, width, view, (state, now), theme)
            })
            .collect()
    };

    let new_index = benches.len();
    let state = row_state(app, new_index, lit == Some(new_index));
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
    let new_bench = list_row(width, 0, icon, "New bench", cell, None);

    let view = ScrollView {
        total: benches.len(),
        visible,
        first,
    };
    let pinned = Some((bottom_row, new_bench));
    draw_list(
        frame,
        (area, rows_area),
        lines,
        pinned,
        view,
        theme,
        is_focused,
    );
}

/// Whether row `index` is lit, by the keyboard or as what the chat panel
/// shows, or under the pointer.
fn row_state(app: &App, index: usize, selected: bool) -> CellState {
    let hovered = app.hover() == Some(HoverTarget::BenchRow(index));
    CellState::of(selected, hovered)
}

/// `✓  Compare #2  add retries  3h`: the spinner while it runs, `■` once
/// stopped and `✓` once its runs ended, as the PROCESSES tab marks a
/// process, then its name, what it is about, and how long ago it started.
fn bench_row<'a>(
    app: &App,
    width: usize,
    view: &BenchView,
    (state, now): (CellState, u64),
    theme: &Theme,
) -> Line<'a> {
    let icon = match view.status {
        BenchStatus::Running => working_icon(app, theme),
        BenchStatus::Cancelled => StatusIcon::new("■", theme.colors.muted),
        _ => StatusIcon::new("✓", theme.colors.success),
    };
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    let about = view
        .title
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let ago = (view.started_ms > 0).then(|| format_ago(view.started_ms, now));
    list_row_with_detail(
        width,
        0,
        icon,
        (&view.name, Some(about)),
        cell,
        ago.as_deref(),
    )
}
