//! TASKS tab: the work agents did or are doing, grouped under the plan it
//! works on. Each plan's header shows its todo progress, then its todos.
//! Work of no listed plan goes last, under
//! `No Plan`. Requests have no row: their todo items are listed, and a
//! task handed out sits one level under the todo or item it does (see
//! [`crate::app::tasks`]). Every other row's icon sits under its
//! header's fold arrow.
//!
//! ```text
//!  ▾  Auth rework                 1/3
//!  ☒  Add session table
//!  ▣  Write the login handler
//!    ⢎⡱ Write the login handler  → coder
//!  ☐  Wire the middleware
//!  ▸  Parser cleanup              5/5
//!  ▾  No Plan
//!  ☒  Find the uses  → explorer
//!    ✓  Find the uses of calc_total  → explorer
//!  ✓  Fix the typo  → coder
//! ```

use super::panels::{
    aura_icon, cell_style, list_row, list_row_with_detail, render_scrollbar, CellState, ScrollView,
    StatusIcon,
};
use super::plans::{marker, marker_color, todo_style};
use super::side_list::{
    name_style, render_empty, render_hidden, render_hint, split_hint, Tone, TASKS_HINT,
};
use crate::app::tasks::{TaskList, TaskRow};
use crate::app::{HoverTarget, PanelFocus};
use crate::events::{PlanView, TaskState, TaskView, TodoView};
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Modifier, text::Line, widgets::Paragraph, Frame};

/// Label of the header of the tasks that work on no listed plan.
const NO_PLAN: &str = "No Plan";

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
        } else {
            render_empty(frame, inner, "No tasks yet.", theme);
        }
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
    let view = TreeView {
        first: scroll,
        focused: is_focused,
        hovered,
    };
    render_task_tree(frame, (area, rows_area), app.task_list(), view, app, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, TASKS_HINT, theme);
    }
}

/// How a task tree is drawn: where it starts and which row is lit.
#[derive(Debug, Clone, Copy)]
pub(super) struct TreeView {
    /// First row drawn
    pub(super) first: usize,
    /// Whether the panel has focus, so the highlighted row is lit
    pub(super) focused: bool,
    /// Row under the mouse
    pub(super) hovered: Option<usize>,
}

/// Draw `list`'s rows from `view.first` into `rows_area`, inside the
/// panel bordered by `area`, with the scrollbar on its border. The TASKS
/// tab and the channel's PROJECTS tab both draw their tasks with this.
pub(super) fn render_task_tree(
    frame: &mut Frame,
    (area, rows_area): (Rect, Rect),
    list: &TaskList,
    view: TreeView,
    app: &App,
    theme: &Theme,
) {
    let visible = usize::from(rows_area.height);
    let width = usize::from(rows_area.width);
    let lines: Vec<Line> = list
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
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, scroll, view.focused, theme);
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
        TaskRow::Plan { plan, open } => {
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
        TaskRow::Todo { plan, todo, .. } => list
            .plans()
            .get(plan)
            .and_then(|plan| plan.todos.get(todo))
            .map_or_else(Line::default, |todo| {
                todo_line(todo, depth, (width, state), theme)
            }),
        TaskRow::Task { task, .. } => list.tasks().get(task).map_or_else(Line::default, |task| {
            task_line(task, depth, (width, state), app, theme)
        }),
        TaskRow::Item { task, item, .. } => {
            list.item(task, item).map_or_else(Line::default, |item| {
                todo_line(item, depth, (width, state), theme)
            })
        }
    }
}

/// A plan's header: its fold glyph, its name and its todo progress.
fn plan_line(
    plan: &PlanView,
    fold: StatusIcon,
    (width, state): (usize, CellState),
    theme: &Theme,
) -> Line<'static> {
    let (closed, total) = plan.progress();
    // A plan with nothing left to do settles like its todos; one with no
    // todos yet is still open.
    let tone = Tone::active_if(total == 0 || plan.has_open_todos());
    list_row(
        width,
        0,
        fold,
        &plan.name,
        cell_style(theme, state, name_style(tone, theme)),
        Some(&format!("{closed}/{total}")),
    )
}

/// A handed-out task's row: its icon, its title and the kind of agent it
/// was handed to. Its agent's own checklist is not listed, so no
/// progress either.
fn task_line(
    task: &TaskView,
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
        task_icon(task, app, theme),
        (task.title.lines().next().unwrap_or(""), who.as_deref()),
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
mod tests {
    use super::*;
    use crate::events::{AgentSummary, TodoState, UiCommand};
    use ratatui::{backend::TestBackend, Terminal};

    /// The screen after drawing the TASKS tab into a 60x4 frame.
    fn drawn(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_tasks(frame, area, area, app, &Theme::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn plans_of_other_agents_only_say_so_when_no_task_is_hidden() {
        let mut app = App::new();
        let agent = |id: &str| AgentSummary {
            id: id.into(),
            name: id.to_uppercase(),
            project_id: Some("site".into()),
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![agent("a"), agent("b")]));
        app.process_command(UiCommand::SetActiveAgent("a".into()));
        app.process_command(UiCommand::SetPlans(vec![PlanView {
            path: ".aura/plans/b.plan.md".into(),
            name: "b".into(),
            todos: vec![TodoView {
                content: "step".into(),
                state: TodoState::Pending,
                assignee: None,
                task: Some("tb".into()),
            }],
        }]));
        assert!(app.task_list().rows().is_empty());
        assert_eq!(app.task_list().hidden(), 0);
        assert_eq!(app.task_list().hidden_plans(), 1);

        let screen = drawn(&mut app);
        assert!(screen.contains("No plans for this agent"), "{screen}");
        assert!(!screen.contains("No tasks yet."), "{screen}");
    }

    #[test]
    fn row_icons_sit_under_the_headers_arrow_and_tasks_one_level_in() {
        let mut app = App::new();
        app.process_command(UiCommand::SetTasks(vec![
            TaskView {
                id: "part".into(),
                title: "Parse".into(),
                state: TaskState::Completed,
                agent_name: "Coder".into(),
                assignee: Some("coder".into()),
                parent: Some("request".into()),
                items: vec![TodoView {
                    content: "lex".into(),
                    state: TodoState::Pending,
                    assignee: None,
                    task: None,
                }],
                ..TaskView::default()
            },
            TaskView {
                id: "request".into(),
                title: "Build it".into(),
                agent_name: "Lead".into(),
                items: vec![TodoView {
                    content: "Write the parser".into(),
                    state: TodoState::InProgress,
                    assignee: Some("coder".into()),
                    task: Some("part".into()),
                }],
                ..TaskView::default()
            },
        ]));
        let screen = drawn(&mut app);
        let lines: Vec<&str> = screen.lines().collect();
        let column = |line: &str, glyph: char| {
            line.chars()
                .position(|c| c == glyph)
                .unwrap_or_else(|| panic!("no {glyph} in:\n{screen}"))
        };
        assert!(lines[0].contains("No Plan"), "{screen}");
        assert!(lines[1].contains("Write the parser  → coder"), "{screen}");
        assert!(lines[2].contains("Parse  → coder"), "{screen}");
        assert_eq!(column(lines[1], '▣'), column(lines[0], '▾'), "{screen}");
        assert_eq!(
            column(lines[2], '✓').saturating_sub(column(lines[0], '▾')),
            2,
            "{screen}"
        );
        assert!(
            !screen.contains("Build it"),
            "the request has no row: {screen}"
        );
        assert!(!screen.contains("Lead"), "{screen}");
        assert!(
            !screen.contains("lex"),
            "a task's own list is not listed: {screen}"
        );
    }
}
