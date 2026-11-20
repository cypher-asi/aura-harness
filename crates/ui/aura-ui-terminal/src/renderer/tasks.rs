//! TASKS tab: the work agents did or are doing, each request with the
//! parts it handed to other agents underneath, and what each has done and
//! has left.
//!
//! ```text
//!  ⢎⡱ Fix the login bug  Agent       1/3
//!      ✓  Find the handler
//!      ▸  Write the fix  → coder
//!      ○  Add a test  → tester
//!    ⢎⡱ Write the fix  → coder       0/2
//!  ✓  Rename the crate  Agent       2/2
//! ```

use super::panels::{
    aura_icon, cell_style, list_row_with_detail, render_scrollbar, CellState, ScrollView,
    StatusIcon,
};
use super::plans::{marker, marker_color, todo_style};
use super::side_list::{
    name_style, render_empty, render_hidden, render_hint, split_hint, Tone, TASKS_HINT,
};
use crate::app::tasks::TaskRow;
use crate::app::{HoverTarget, PanelFocus};
use crate::events::{TaskState, TaskView};
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Modifier, text::Line, widgets::Paragraph, Frame};

/// A task's icon: the aura while an agent works on it, else how it ended.
fn task_icon(task: &TaskView, app: &App, theme: &Theme) -> StatusIcon {
    match task.state {
        TaskState::Running => aura_icon(app),
        TaskState::Open => StatusIcon::new("○", theme.colors.warning),
        TaskState::Completed => StatusIcon::new("✓", theme.colors.success),
        TaskState::Failed => StatusIcon::new("✗", theme.colors.error),
        TaskState::Cancelled => StatusIcon::new("✗", theme.colors.muted),
    }
}

/// Draw the tasks into `inner`, the body below the tab bar of the panel
/// bordered by `area`.
pub(super) fn render_tasks(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let list = app.task_list();
    if list.rows().is_empty() && list.hidden() > 0 {
        render_hidden(frame, inner, ("tasks", list.hidden()), theme);
        return;
    }
    if list.rows().is_empty() {
        let message = ["No tasks yet.", "Work agents do", "shows up here."];
        render_empty(frame, inner, &message, theme);
        return;
    }

    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = rows_area.height as usize;
    let scroll = app.sync_tasks_scroll(visible);
    app.set_task_rows_area(rows_area, scroll);
    let hovered = match app.hover() {
        Some(HoverTarget::TaskRow(index)) => Some(index),
        _ => None,
    };
    let width = inner.width as usize;
    let app = &*app;
    let list = app.task_list();
    let lines: Vec<Line> = list
        .rows()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, &row)| {
            let state = CellState::of(is_focused && i == list.selected(), hovered == Some(i));
            let task = &list.tasks()[row.task()];
            match row {
                TaskRow::Task { depth, .. } => {
                    let mut name = name_style(Tone::active_if(!task.state.is_closed()), theme);
                    if task.state == TaskState::Cancelled {
                        name = name.add_modifier(Modifier::CROSSED_OUT);
                    }
                    let who = task.assignee.as_ref().map_or_else(
                        || task.agent_name.clone(),
                        |assignee| format!("→ {assignee}"),
                    );
                    let (closed, total) = task.progress();
                    let progress = (total > 0).then(|| format!("{closed}/{total}"));
                    list_row_with_detail(
                        width,
                        depth,
                        task_icon(task, app, theme),
                        (task.title.lines().next().unwrap_or(""), Some(&who)),
                        cell_style(theme, state, name),
                        progress.as_deref(),
                    )
                }
                TaskRow::Item { item, depth, .. } => {
                    let item = &task.items[item];
                    let icon = StatusIcon::new(marker(item.state), marker_color(item.state, theme));
                    let agent = item.assignee.as_deref().map(|agent| format!("→ {agent}"));
                    list_row_with_detail(
                        width,
                        depth,
                        icon,
                        (item.content.lines().next().unwrap_or(""), agent.as_deref()),
                        cell_style(theme, state, todo_style(item.state, theme)),
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
        render_hint(frame, hint, TASKS_HINT, theme);
    }
}
