//! PLANS tab: the workspace's plans, each with its todos underneath.
//!
//! ```text
//!  ▾  Auth rework              1/3
//!    ✓  Add session table
//!    ▸  Write the login handler
//!    ○  Wire the middleware
//!  ▸  Parser cleanup           5/5
//! ```

use super::panels::{
    cell_style, list_row, list_row_with_detail, render_scrollbar, CellState, ScrollView, StatusIcon,
};
use super::side_list::{name_style, render_empty, render_hint, split_hint, Tone, PLANS_HINT};
use crate::app::plans::PlanRow;
use crate::app::{HoverTarget, PanelFocus};
use crate::events::TodoState;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

pub(super) const fn marker(status: TodoState) -> &'static str {
    match status {
        TodoState::Pending => "○",
        TodoState::InProgress => "▸",
        TodoState::Completed => "✓",
        TodoState::Cancelled => "✗",
    }
}

/// Color of a todo's marker: the status color, as on every side-panel icon.
pub(super) const fn marker_color(state: TodoState, theme: &Theme) -> Color {
    match state {
        TodoState::InProgress => theme.colors.pending,
        TodoState::Pending | TodoState::Cancelled => theme.colors.muted,
        TodoState::Completed => theme.colors.success,
    }
}

/// How a todo's text is drawn: the shared tone (open todos `Active`,
/// closed ones `Settled`), struck through when cancelled.
pub(super) fn todo_style(state: TodoState, theme: &Theme) -> Style {
    let style = name_style(Tone::active_if(!state.is_closed()), theme);
    if state == TodoState::Cancelled {
        style.add_modifier(Modifier::CROSSED_OUT)
    } else {
        style
    }
}

/// How a todo under a plan is drawn: always muted gray, even while open,
/// so the plan's own name stands out. Struck through when cancelled.
/// The TASKS panel keeps the shared `todo_style`.
pub(super) fn plan_todo_style(state: TodoState, theme: &Theme) -> Style {
    let style = name_style(Tone::Settled, theme);
    if state == TodoState::Cancelled {
        style.add_modifier(Modifier::CROSSED_OUT)
    } else {
        style
    }
}

/// Draw the plans into `inner`, the body below the tab bar of the panel
/// bordered by `area`.
pub(super) fn render_plans(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    if app.plan_list().plans().is_empty() {
        let message = ["No plans yet.", "/plan asks the agent", "to write one."];
        render_empty(frame, inner, &message, theme);
        return;
    }

    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = rows_area.height as usize;
    let scroll = app.sync_plans_scroll(visible);
    app.set_plan_rows_area(rows_area, scroll);
    let hovered = match app.hover() {
        Some(HoverTarget::PlanRow(index)) => Some(index),
        _ => None,
    };
    let width = inner.width as usize;
    let list = app.plan_list();
    let lines: Vec<Line> = list
        .rows()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, &row)| {
            let state = CellState::of(is_focused && i == list.selected(), hovered == Some(i));
            let plan = &list.plans()[row.plan()];
            match row {
                PlanRow::Plan { open, .. } => {
                    let (closed, total) = plan.progress();
                    // A plan with nothing left to do settles like its todos;
                    // one with no todos yet is still open.
                    let tone = Tone::active_if(total == 0 || plan.has_open_todos());
                    let name = name_style(tone, theme);
                    let fold = if open { "▾" } else { "▸" };
                    list_row(
                        width,
                        0,
                        StatusIcon::new(fold, theme.colors.muted),
                        &plan.name,
                        cell_style(theme, state, name),
                        Some(&format!("{closed}/{total}")),
                    )
                }
                PlanRow::Todo { todo, .. } => {
                    let todo = &plan.todos[todo];
                    let icon = StatusIcon::new(marker(todo.state), marker_color(todo.state, theme));
                    let content = todo.content.lines().next().unwrap_or("");
                    let agent = todo.assignee.as_deref().map(|agent| format!("→ {agent}"));
                    list_row_with_detail(
                        width,
                        1,
                        icon,
                        (content, agent.as_deref()),
                        cell_style(theme, state, plan_todo_style(todo.state, theme)),
                        None,
                    )
                }
            }
        })
        .collect();
    let view = ScrollView {
        total: list.rows().len(),
        visible,
        first: scroll,
    };
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, PLANS_HINT, theme);
    }
}
