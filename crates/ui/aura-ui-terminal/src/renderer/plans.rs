//! PLANS tab: the workspace's plans, each with its todos underneath.
//!
//! ```text
//!  ▾  Auth rework              1/3
//!    ✓  Add session table
//!    ▸  Write the login handler
//!    ○  Wire the middleware
//!  ▸  Parser cleanup           5/5
//! ```

use super::panels::{cell_style, list_row, render_scrollbar, CellState, ScrollView, StatusIcon};
use super::plan::{marker, todo_style};
use crate::app::plans::PlanRow;
use crate::app::{HoverTarget, PanelFocus};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Hint shown at the bottom of the tab when the panel has focus.
const PLANS_HINT: &str = "⏎ fold / open  ⇧←→ tabs";

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
        let lines: Vec<Line> = [
            "  No plans yet.",
            "  /plan asks the agent",
            "  to write one.",
        ]
        .into_iter()
        .map(|text| Line::from(Span::styled(text, Style::default().fg(theme.colors.muted))))
        .collect();
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let show_hint = is_focused && inner.height >= 4;
    let rows_area = Rect {
        height: inner.height.saturating_sub(u16::from(show_hint)),
        ..inner
    };
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
                    let color = if list.is_current(plan) {
                        theme.colors.warning
                    } else {
                        theme.colors.foreground
                    };
                    let name = Style::default().fg(color).add_modifier(Modifier::BOLD);
                    let (closed, total) = plan.progress();
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
                    let style = todo_style(todo.state, theme);
                    let icon = StatusIcon::new(
                        marker(todo.state),
                        style.fg.unwrap_or(theme.colors.foreground),
                    );
                    let content = todo.content.lines().next().unwrap_or("");
                    list_row(
                        width,
                        1,
                        icon,
                        content,
                        cell_style(theme, state, style),
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

    if show_hint {
        let hint = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {PLANS_HINT}"),
                Style::default().fg(theme.colors.muted),
            )),
            hint,
        );
    }
}
