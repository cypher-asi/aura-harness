//! The agents panel's BENCH tab: every benchmark, newest first, one row
//! each with its number and what it ran, its arms and how long ago it
//! started, and "+ New bench" pinned to the bottom row (see [`label`]).
//! A benchmark with agents to unfold over has an arrow in its icon
//! column, the spinner while it runs; unfolded, they hang from its name
//! as the projects list hangs spawns: the agent of each arm, muted, over
//! its runs, and under a run the agents it spawned.
//!
//! ```text
//! ⠋  #3 fix the parser  gate vs opus-5-5      2m
//! ▾  #2 swe-pro ×20  gate vs opus-5-5         3h
//!    ├── arm A
//!    │   ├── run A1                           3h
//!    │   │   └── scout                        3h
//!    │   └── run A2                           3h
//!    └── arm B
//!        └── run B1                           3h
//!
//! +  New bench
//! ```
//!
//! What a row shows, a benchmark's numbers, an agent's chat or the form,
//! is in the chat panel. While the panel is focused the keyboard's row is
//! lit; otherwise the row of what the chat panel shows is.

mod label;
#[cfg(test)]
mod tests;

use super::projects::spawn_guides_below;
use super::{draw_list, split_pinned, working_icon};
use crate::app::{AgentRowEnd, BenchAgent, BenchRow, HoverTarget, PanelFocus};
use crate::components::{
    cell_style, format_ago, list_row, list_row_with_detail, tree_row, CellState, StatusIcon,
};
use crate::events::BenchStatus;
use crate::renderer::chrome::ScrollView;
use crate::renderer::side_list::empty_line;
use crate::{App, Theme};
use aura_core_types::time::now_ms;
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
    let (rows_area, bottom_row) = split_pinned(app, area, body);
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
    let rows = app.bench_rows();
    let new_bench = rows.len() - 1;
    let line = |index: usize| {
        let state = row_state(app, index, lit == Some(index));
        row_line(app, width, rows[index], (state, now), theme)
    };
    let lines: Vec<Line> = if new_bench == 0 {
        let text = if app.bench().loaded() {
            "No benchmarks yet."
        } else {
            "Loading…"
        };
        vec![empty_line(text, body.width, theme)]
    } else {
        (first..new_bench).take(visible).map(line).collect()
    };
    let pinned = Some((bottom_row, line(new_bench)));

    let view = ScrollView {
        total: new_bench,
        visible,
        first,
    };
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

fn row_line<'a>(
    app: &App,
    width: usize,
    row: BenchRow,
    (state, now): (CellState, u64),
    theme: &Theme,
) -> Line<'a> {
    match row {
        BenchRow::Bench(index) => bench_row(app, width, index, (state, now), theme),
        BenchRow::Agent(agent) => agent_row(app, width, agent, (state, now), theme),
        BenchRow::New => {
            let cell = cell_style(theme, state, Style::default().fg(theme.colors.muted));
            let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
            list_row(width, 0, icon, "New bench", cell, None)
        }
    }
}

/// `▾  #2 swe-pro ×20  gate vs opus-5-5  3h`: in the icon column the
/// spinner while it runs, else `▸` folded over its agents, `▾` unfolded,
/// or nothing with none listed; then its number and work, its arms muted,
/// and how long ago it started. A benchmark the user stopped is muted.
fn bench_row<'a>(
    app: &App,
    width: usize,
    index: usize,
    (state, now): (CellState, u64),
    theme: &Theme,
) -> Line<'a> {
    let view = &app.bench().benches[index];
    let icon = if view.status == BenchStatus::Running {
        working_icon(app, theme)
    } else if !app.bench_has_agents(index) {
        StatusIcon::new("", theme.colors.muted)
    } else if app.bench_unfolded(index) {
        StatusIcon::new("▾", theme.colors.muted)
    } else {
        StatusIcon::new("▸", theme.colors.muted)
    };
    let color = if view.status == BenchStatus::Cancelled {
        theme.colors.muted
    } else {
        theme.colors.foreground
    };
    let cell = cell_style(theme, state, Style::default().fg(color));
    let ago = (view.started_ms > 0).then(|| format_ago(view.started_ms, now));
    list_row_with_detail(
        width,
        0,
        icon,
        (&label::name(view), label::arms(view).as_deref()),
        cell,
        ago.as_deref(),
    )
}

/// An agent of an unfolded benchmark, on the tree hanging from the
/// benchmark's name, the spinner on it while it works: an arm's, muted
/// and with no time, as it only labels the runs under it; a run; or,
/// muted, an agent a run spawned.
fn agent_row<'a>(
    app: &App,
    width: usize,
    at: BenchAgent,
    (state, now): (CellState, u64),
    theme: &Theme,
) -> Line<'a> {
    let list = app.bench_agent_list(at.archived);
    let agent = &list[at.index];
    let guides = spawn_guides_below(list, at.index, at.top);
    let icon = if app.agent_thinking(agent) {
        working_icon(app, theme)
    } else {
        StatusIcon::new("", theme.colors.muted)
    };
    let color = if at.arm || agent.depth > at.top + 2 {
        theme.colors.muted
    } else {
        theme.colors.foreground
    };
    let cell = cell_style(theme, state, Style::default().fg(color));
    let ago = agent
        .last_active_ms
        .filter(|_| !at.arm)
        .map(|ms| format_ago(ms, now));
    tree_row(
        width,
        (&guides, theme.colors.muted),
        icon,
        (&agent.name, None),
        cell,
        &AgentRowEnd::agent(ago.as_deref()),
        &[],
    )
}
