//! TASKS tab: the work agents did or are doing, grouped under the plan it
//! works on, the groups under the day they last moved on, headed in
//! muted capitals as the Archive heads its days. Each plan's header shows
//! its todo progress, then its todos. Work of no listed plan goes last in
//! each day, under `No Plan`. Requests have no row: their todo items are
//! listed, and a task handed out sits one level under the todo or item it
//! does, titled by it. Builds of a todo list the plan no longer has are
//! listed under an earlier round's header (see [`crate::app::tasks`]).
//! Every other row's icon sits under its header's fold arrow.
//!
//! ```text
//!  TODAY
//!  ▾  Auth rework                 1/3
//!  ✓  Add session table
//!  ◐  Write the login handler
//!    ⢎⡱ Write the login handler  → coder
//!  ☐  Wire the middleware
//!  ▸  Build the auth              4/4
//!  ▾  No Plan
//!  ✓  Find the uses  → explorer
//!    ✓  Find the uses  → explorer
//!  ✓  Fix the typo  → coder
//!  YESTERDAY
//!  ▸  Parser cleanup            ✓ 5/5
//! ```

use super::agents::aura_icon;
use super::chrome::{Body, ScrollView};
use super::plans::{marker, marker_color, todo_style};
use super::side_list::{name_style, render_empty, render_hidden, Tone};
use crate::app::days::day_name;
use crate::app::tasks::{TaskList, TaskRow};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{
    cell_style, list_row, list_row_with_detail, truncate_record_info, CellState, StatusIcon,
    ROW_INSET,
};
use crate::events::{PlanView, TaskState, TaskView, TodoView};
use crate::{App, Theme};
use chrono::{Local, NaiveDate};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// Label of the header of the tasks that work on no listed plan.
const NO_PLAN: &str = "No Plan";

/// A task's icon: the aura while an agent works on it, else how it ended.
/// A failure a newer task made up for (`stale`) is no longer an error.
fn task_icon(task: &TaskView, stale: bool, app: &App, theme: &Theme) -> StatusIcon {
    match task.state {
        TaskState::Running => aura_icon(app),
        TaskState::Open => StatusIcon::new("○", theme.colors.warning),
        TaskState::Completed => StatusIcon::new("✓", theme.colors.success),
        TaskState::Failed if stale => StatusIcon::new("✗", theme.colors.muted),
        TaskState::Failed => StatusIcon::new("✗", theme.colors.error),
        TaskState::Cancelled => StatusIcon::new("✗", theme.colors.muted),
    }
}

/// Draw the plans and tasks into `inner`, the body below the tab bar of
/// the panel bordered by `area`.
pub(super) fn render_tasks(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let list = app.task_list();
    if list.rows().is_empty() {
        // Hidden tasks are named first: the plans are left out with them.
        if list.hidden() > 0 {
            render_hidden(frame, inner, ("tasks", list.hidden()), theme);
        } else if list.hidden_plans() > 0 {
            render_hidden(frame, inner, ("plans", list.hidden_plans()), theme);
        } else if app.sandbox().is_remote() && app.machine_asleep() {
            let text = "The machine is asleep; its plans show when it wakes.";
            render_empty(frame, inner, text, theme);
        } else {
            render_empty(frame, inner, "No tasks yet.", theme);
        }
        return;
    }

    let body = Body::list(app, area, inner, is_focused);
    let scroll = app.sync_tasks_scroll(body.visible());
    app.set_task_rows_area(body.rows(), scroll);
    let hovered = match app.hover() {
        Some(HoverTarget::TaskRow(index)) => Some(index),
        _ => None,
    };
    let view = TreeView {
        first: scroll,
        focused: is_focused,
        hovered,
    };
    let (lines, scroll) = task_tree_lines(app.task_list(), view, &body, app, theme);
    body.draw(frame, lines, scroll, theme);
}

/// How a task tree is drawn: where it starts and which row is lit.
#[derive(Debug, Clone, Copy)]
struct TreeView {
    /// First row drawn
    first: usize,
    /// Whether the panel has focus, so the highlighted row is lit
    focused: bool,
    /// Row under the mouse
    hovered: Option<usize>,
}

/// The rows of `list` from `view.first` that fit in `body`, and how much
/// of the list they show.
fn task_tree_lines(
    list: &TaskList,
    view: TreeView,
    body: &Body,
    app: &App,
    theme: &Theme,
) -> (Vec<Line<'static>>, ScrollView) {
    let visible = body.visible();
    let width = body.width();
    let lines = list
        .rows()
        .iter()
        .enumerate()
        .skip(view.first)
        .take(visible)
        .map(|(i, &row)| {
            let lit = view.focused && i == list.selected();
            let state = CellState::of(lit, view.hovered == Some(i));
            row_line(list, row, (width, state), app, theme)
        })
        .collect();
    let scroll = ScrollView {
        total: list.rows().len(),
        visible,
        first: view.first,
    };
    (lines, scroll)
}

/// The line of one row, `width` columns wide.
fn row_line(
    list: &TaskList,
    row: TaskRow,
    (width, state): (usize, CellState),
    app: &App,
    theme: &Theme,
) -> Line<'static> {
    let depth = row.depth();
    match row {
        TaskRow::Day { day } => day_line(day, width, theme),
        TaskRow::Plan { plan, open, .. } => {
            let fold = StatusIcon::new(if open { "▾" } else { "▸" }, theme.colors.muted);
            match plan.and_then(|plan| list.plans().get(plan)) {
                Some(plan) => plan_line(plan, fold, (width, state), theme),
                None => list_row(
                    width,
                    depth,
                    fold,
                    NO_PLAN,
                    cell_style(theme, state, name_style(Tone::Settled, theme)),
                    None,
                ),
            }
        }
        TaskRow::Round {
            first,
            open,
            progress,
        } => list.tasks().get(first).map_or_else(Line::default, |first| {
            let fold = StatusIcon::new(if open { "▾" } else { "▸" }, theme.colors.muted);
            round_line(first, fold, progress, (width, state), theme)
        }),
        TaskRow::Todo { plan, todo, .. } => list
            .plans()
            .get(plan)
            .and_then(|plan| plan.todos.get(todo))
            .map_or_else(Line::default, |todo| {
                todo_line(todo, depth, (width, state), theme)
            }),
        TaskRow::Task { task, stale, .. } => {
            list.tasks().get(task).map_or_else(Line::default, |task| {
                let title = list.title_of(row);
                task_line(task, (title, stale), depth, (width, state), app, theme)
            })
        }
        TaskRow::Item { task, item, .. } => {
            list.item(task, item).map_or_else(Line::default, |item| {
                todo_line(item, depth, (width, state), theme)
            })
        }
    }
}

/// The header of `day`, from the icon column on, in muted capitals as
/// the Archive draws its days. It is named as of now, so `TODAY` turns
/// `YESTERDAY` at midnight with nothing else changing.
fn day_line(day: Option<NaiveDate>, width: usize, theme: &Theme) -> Line<'static> {
    let label = day_name(day, Local::now().date_naive()).to_uppercase();
    let label = truncate_record_info(&label, width.saturating_sub(2 * ROW_INSET));
    Line::from(Span::styled(
        format!("{:ROW_INSET$}{label}", ""),
        Style::default().fg(theme.colors.muted),
    ))
}

/// A plan's header: its fold glyph, its name and its todo progress.
fn plan_line(
    plan: &PlanView,
    fold: StatusIcon,
    (width, state): (usize, CellState),
    theme: &Theme,
) -> Line<'static> {
    let (closed, total) = plan.progress();
    // A plan with nothing left to do settles like its todos, marked done;
    // one with no todos yet is still open.
    let open = total == 0 || plan.has_open_todos();
    let tone = Tone::active_if(open);
    let progress = if open {
        format!("{closed}/{total}")
    } else {
        format!("✓ {closed}/{total}")
    };
    list_row(
        width,
        0,
        fold,
        &plan.name,
        cell_style(theme, state, name_style(tone, theme)),
        Some(&progress),
    )
}

/// An earlier round's header: its fold glyph, what its first request
/// asked and its items' progress.
fn round_line(
    first: &TaskView,
    fold: StatusIcon,
    (closed, total): (usize, usize),
    (width, state): (usize, CellState),
    theme: &Theme,
) -> Line<'static> {
    let tone = Tone::active_if(closed < total);
    list_row(
        width,
        0,
        fold,
        first.title.lines().next().unwrap_or(""),
        cell_style(theme, state, name_style(tone, theme)),
        Some(&format!("{closed}/{total}")),
    )
}

/// A handed-out task's row: its icon, its `title` and the kind of agent
/// it was handed to. Its agent's own checklist is not listed, so no
/// progress either. `stale` when a newer task does its todo instead.
fn task_line(
    task: &TaskView,
    (title, stale): (&str, bool),
    depth: usize,
    (width, state): (usize, CellState),
    app: &App,
    theme: &Theme,
) -> Line<'static> {
    let mut name = name_style(Tone::active_if(!task.state.is_closed()), theme);
    if task.state == TaskState::Cancelled {
        name = name.add_modifier(Modifier::CROSSED_OUT);
    }
    let who = task
        .assignee
        .as_ref()
        .map(|assignee| format!("→ {assignee}"));
    list_row_with_detail(
        width,
        depth,
        task_icon(task, stale, app, theme),
        (title, who.as_deref()),
        cell_style(theme, state, name),
        None,
    )
}

/// A plan's todo or a task's item: its box, its first line and who does
/// it.
fn todo_line(
    todo: &TodoView,
    depth: usize,
    (width, state): (usize, CellState),
    theme: &Theme,
) -> Line<'static> {
    let icon = StatusIcon::new(marker(todo.state), marker_color(todo.state, theme));
    let agent = todo.assignee.as_deref().map(|agent| format!("→ {agent}"));
    list_row_with_detail(
        width,
        depth,
        icon,
        (todo.content.lines().next().unwrap_or(""), agent.as_deref()),
        cell_style(theme, state, todo_style(todo.state, theme)),
        None,
    )
}

#[cfg(test)]
mod tests;
