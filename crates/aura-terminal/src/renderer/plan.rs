//! Plan strip: the active plan's todos, drawn between the panels and the
//! input line while some are still open.

use super::{inset_edges, text::display_width};
use crate::{
    events::{PlanView, TodoState},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Most todo rows shown; the window follows the first open todo.
const MAX_TODO_ROWS: usize = 5;
/// Rows the panels keep before the strip is dropped.
const MIN_PANEL_ROWS: u16 = 8;

/// The plan the strip shows, when it has open todos.
fn shown_plan(app: &App) -> Option<&PlanView> {
    app.active_plan().filter(|plan| plan.has_open_todos())
}

/// Rows the strip takes out of `screen_height`, 0 when it is hidden.
pub(super) fn strip_height(app: &App, screen_height: u16) -> u16 {
    let Some(plan) = shown_plan(app) else {
        return 0;
    };
    let rows = u16::try_from(plan.todos.len().min(MAX_TODO_ROWS) + 1).unwrap_or(u16::MAX);
    if screen_height.saturating_sub(rows) < MIN_PANEL_ROWS + 4 {
        return 0;
    }
    rows
}

/// First todo row to show so the first open todo is visible.
fn window_start(plan: &PlanView) -> usize {
    let first_open = plan
        .todos
        .iter()
        .position(|t| !t.state.is_closed())
        .unwrap_or(0);
    let last_start = plan.todos.len().saturating_sub(MAX_TODO_ROWS);
    first_open.saturating_sub(1).min(last_start)
}

pub(super) const fn marker(status: TodoState) -> &'static str {
    match status {
        TodoState::Pending => "○",
        TodoState::InProgress => "▸",
        TodoState::Completed => "✓",
        TodoState::Cancelled => "✗",
    }
}

/// How a todo in `state` is drawn.
pub(super) fn todo_style(state: TodoState, theme: &Theme) -> Style {
    match state {
        TodoState::InProgress => Style::default().fg(theme.colors.primary),
        TodoState::Pending => Style::default().fg(theme.colors.foreground),
        TodoState::Completed => Style::default().fg(theme.colors.success),
        TodoState::Cancelled => Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::CROSSED_OUT),
    }
}

/// Render the strip into `area`.
pub(super) fn render_plan_strip(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(plan) = shown_plan(app) else {
        return;
    };
    if area.height == 0 {
        return;
    }
    let area = inset_edges(area);
    let width = usize::from(area.width);
    let muted = Style::default().fg(theme.colors.muted);
    let (closed, total) = plan.progress();

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "PLAN ",
            Style::default()
                .fg(theme.colors.warning)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            fit(&plan.name, width.saturating_sub(12)),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(format!("  {closed}/{total}"), muted),
    ])];
    for todo in plan
        .todos
        .iter()
        .skip(window_start(plan))
        .take(usize::from(area.height) - 1)
    {
        let style = todo_style(todo.state, theme);
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", marker(todo.state)), style),
            Span::styled(fit(&todo.content, width.saturating_sub(4)), style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// `text` cut to `width` columns, with `…` when shortened.
fn fit(text: &str, width: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if display_width(line) <= width {
        return line.to_string();
    }
    let mut out = String::new();
    for ch in line.chars() {
        if display_width(&out) + 2 > width {
            break;
        }
        out.push(ch);
    }
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TodoView;

    fn plan(statuses: &[TodoState]) -> PlanView {
        PlanView {
            path: ".aura/plans/p.plan.md".into(),
            name: "P".into(),
            todos: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| TodoView {
                    content: format!("todo {i}"),
                    state: *s,
                })
                .collect(),
        }
    }

    #[test]
    fn window_keeps_the_first_open_todo_in_view() {
        use TodoState::{Completed as C, Pending as P};
        assert_eq!(window_start(&plan(&[P, P])), 0);
        assert_eq!(window_start(&plan(&[C, C, C, C, C, C, P, P])), 3);
        assert_eq!(window_start(&plan(&[C, C, P, P, P, P, P, P])), 1);
    }

    #[test]
    fn fit_shortens_long_text() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a much longer line", 8), "a much…");
    }
}
