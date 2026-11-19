//! What the agent on screen has used and is working on: the footer under
//! the input, and the full todo list that `/todos` opens.
//!
//! ```text
//! qwen3:4b · 23% context used · 2 calls                        ● Ready
//! / for commands · @ for files · $ for a terminal · Ctrl+O steps
//! ```

use super::text::display_width;
use crate::app::format::{format_cost, format_tokens};
use crate::app::hud::Hud;
use crate::events::{TodoState, TodoView};
use crate::mode_command::PermissionMode;
use crate::{App, Theme};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Most todo items listed at once.
const MAX_TODO_ROWS: usize = 10;

/// Columns kept clear between the stats and the status at the right.
const STATUS_GAP: usize = 2;

const SEPARATOR: &str = " · ";

/// Rows the footer takes under the input box.
pub(super) const FOOTER_ROWS: u16 = 1;

/// What the status line reports usage against: `gate`, or one model and
/// its context window when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterModel {
    Gate,
    Model { window: Option<u64> },
}

impl FooterModel {
    fn of(app: &App) -> Self {
        if app.current_model() == "gate" {
            Self::Gate
        } else {
            Self::Model {
                window: app.current_model_option().and_then(|m| m.context_window),
            }
        }
    }
}

/// The parts of the status line after the model, most important first.
/// On one model the context share is of that model's window, so it is
/// right as soon as the model is picked. Under `gate` it is left out,
/// since each turn may run on a model with a different window, and the
/// models used are counted instead.
pub(super) fn status_parts(hud: &Hud, model: FooterModel, theme: &Theme) -> Vec<(String, Color)> {
    let stats = &hud.stats;
    let mut parts = Vec::new();
    match hud.mode {
        None | Some(PermissionMode::Agent) => {}
        Some(PermissionMode::Plan) => {
            parts.push((
                "plan mode (/plan to leave)".to_string(),
                theme.colors.warning,
            ));
        }
        Some(mode) => parts.push((format!("{mode} mode"), theme.colors.warning)),
    }
    let window = match model {
        FooterModel::Gate => None,
        FooterModel::Model { window } => Some(window),
    };
    if let Some(window) = window.filter(|_| stats.context_tokens > 0) {
        let mut text = format!("{} context", format_tokens(stats.context_tokens));
        let mut color = theme.colors.muted;
        if let Some(percent) = stats.context_percent_in(window.or(stats.context_limit)) {
            text = format!("{percent}% context used");
            color = match percent {
                90.. => theme.colors.error,
                75.. => theme.colors.warning,
                _ => theme.colors.muted,
            };
        }
        parts.push((text, color));
    }
    if let Some(hit) = stats.cache_hit_percent() {
        parts.push((format!("{hit}% cached"), theme.colors.muted));
    }
    if stats.priced {
        parts.push((format_cost(stats.cost_usd), theme.colors.muted));
    }
    if model == FooterModel::Gate && !stats.models.is_empty() {
        let models = stats.models.len();
        let plural = if models == 1 { "" } else { "s" };
        parts.push((format!("{models} model{plural}"), theme.colors.muted));
    }
    if stats.calls > 0 {
        let plural = if stats.calls == 1 { "" } else { "s" };
        parts.push((format!("{} call{plural}", stats.calls), theme.colors.muted));
    }
    let jobs = hud.running_jobs();
    if jobs > 0 {
        parts.push((format!("{jobs} in background"), theme.colors.foreground));
    }
    if hud.queued > 0 {
        parts.push((format!("{} queued", hud.queued), theme.colors.foreground));
    }
    if stats.compactions > 0 {
        parts.push((
            format!("summarized {}×", stats.compactions),
            theme.colors.muted,
        ));
    }
    parts
}

/// `● Ready`, the spinner while thinking, or what the agent is waiting on.
fn status_spans<'a>(app: &'a App, theme: &Theme) -> Vec<Span<'a>> {
    let status = app.status();
    let (mark, mark_color, text_color) = if status == "Ready" {
        ("●".to_string(), theme.colors.success, theme.colors.muted)
    } else if status.contains("Thinking") {
        (
            app.spinner_char().to_string(),
            theme.colors.muted,
            theme.colors.muted,
        )
    } else {
        ("◐".to_string(), theme.colors.warning, theme.colors.warning)
    };
    vec![
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(status, Style::default().fg(text_color)),
    ]
}

/// `parts` joined by separators, dropping the trailing ones that don't fit
/// in `room` columns.
fn fitting<'a>(parts: Vec<Vec<Span<'a>>>, room: usize, theme: &Theme) -> Vec<Span<'a>> {
    let mut used = 0;
    let mut spans = Vec::new();
    for part in parts {
        let width: usize = part.iter().map(|s| display_width(&s.content)).sum();
        if width == 0 {
            continue;
        }
        let sep = if spans.is_empty() { 0 } else { SEPARATOR.len() };
        if used + sep + width > room {
            break;
        }
        if sep > 0 {
            spans.push(Span::styled(
                SEPARATOR,
                Style::default().fg(theme.colors.faint),
            ));
        }
        spans.extend(part);
        used += sep + width;
    }
    spans
}

/// Render the footer into `area`: the model and what the agent has used on
/// the left, with its status at the right.
pub(super) fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let row = Rect { height: 1, ..area };
    let status = status_spans(app, theme);
    let status_width: usize = status.iter().map(|s| display_width(&s.content)).sum();
    let room = usize::from(row.width).saturating_sub(status_width + STATUS_GAP);

    let mut stats = status_parts(app.hud(), FooterModel::of(app), theme).into_iter();
    let mut parts = Vec::new();
    if app.mode() != PermissionMode::Agent {
        if let Some((text, color)) = stats.next() {
            parts.push(vec![Span::styled(text, Style::default().fg(color))]);
        }
    }
    parts.push(super::model_indicator_spans(app, theme));
    parts.extend(stats.map(|(text, color)| vec![Span::styled(text, Style::default().fg(color))]));
    frame.render_widget(Paragraph::new(Line::from(fitting(parts, room, theme))), row);
    frame.render_widget(
        Paragraph::new(Line::from(status)).alignment(Alignment::Right),
        row,
    );
}

/// The full todo list over the bottom of the Chat panel, while `/todos`
/// has it open.
pub(super) fn render_todo_panel(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let hud = app.hud();
    if !app.todos_open() || hud.todos.is_empty() || chat.height < 3 || chat.width < 8 {
        return;
    }
    let rows = hud.todos.len().min(MAX_TODO_ROWS);
    let height = u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(chat.height);
    let area = Rect {
        y: chat.bottom() - height,
        height,
        ..chat
    };
    let text_room = usize::from(chat.width).saturating_sub(6);
    // Long lists start at the item being worked on, so it stays in view.
    let first = hud
        .todos
        .iter()
        .position(|t| t.state == TodoState::InProgress)
        .map_or(0, |at| at.min(hud.todos.len() - rows));
    let lines: Vec<Line<'_>> = hud
        .todos
        .iter()
        .skip(first)
        .take(rows)
        .map(|todo| todo_line(todo, text_room, theme))
        .collect();
    let (done, total) = hud.todo_progress();
    let block = Block::default()
        .title(format!(" TODOS {done}/{total} "))
        .title_bottom(Line::from(" /todos close ").right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `⬢ Working on 3 to-dos` over the to-do list, as the chat's live tail
/// while a turn is in flight.
pub(super) fn todo_checklist(hud: &Hud, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if hud.todos.is_empty() {
        return Vec::new();
    }
    let (_, total) = hud.todo_progress();
    let plural = if total == 1 { "" } else { "s" };
    let mut lines = vec![Line::from(vec![
        Span::styled("⬢ ", Style::default().fg(theme.colors.faint)),
        Span::styled(
            "Working",
            Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" on {total} to-do{plural}"),
            Style::default().fg(theme.colors.secondary),
        ),
    ])];
    let room = width.saturating_sub(4);
    lines.extend(
        hud.todos
            .iter()
            .take(MAX_TODO_ROWS)
            .map(|todo| todo_line(todo, room, theme)),
    );
    let more = hud.todos.len().saturating_sub(MAX_TODO_ROWS);
    if more > 0 {
        lines.push(Line::from(Span::styled(
            format!("    {more} more"),
            Style::default().fg(theme.colors.muted),
        )));
    }
    lines
}

/// `  ▣ Write the parser`: a to-do's box, then its text.
fn todo_line(todo: &TodoView, room: usize, theme: &Theme) -> Line<'static> {
    let (mark, mark_color, text_style) = match todo.state {
        TodoState::Completed => (
            "☒",
            theme.colors.faint,
            Style::default().fg(theme.colors.faint),
        ),
        TodoState::InProgress => (
            "▣",
            theme.colors.foreground,
            Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        TodoState::Pending => (
            "☐",
            theme.colors.muted,
            Style::default().fg(theme.colors.muted),
        ),
        TodoState::Cancelled => (
            "☐",
            theme.colors.faint,
            Style::default()
                .fg(theme.colors.faint)
                .add_modifier(Modifier::CROSSED_OUT),
        ),
    };
    let agent = todo
        .assignee
        .as_deref()
        .map(|agent| format!("  → {agent}"))
        .filter(|agent| display_width(agent) + 8 <= room)
        .unwrap_or_default();
    let room = room.saturating_sub(display_width(&agent));
    Line::from(vec![
        Span::styled(format!("  {mark} "), Style::default().fg(mark_color)),
        Span::styled(truncate(&todo.content, room), text_style),
        Span::styled(agent, Style::default().fg(theme.colors.faint)),
    ])
}

/// `text` cut to `width` columns, ending in `…` when cut.
fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 1;
    for c in text.chars() {
        let w = display_width(c.encode_utf8(&mut [0; 4]));
        if used + w > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_status_says_how_to_leave() {
        let theme = Theme::default();
        let hud = crate::app::hud::Hud {
            mode: Some(PermissionMode::Plan),
            ..crate::app::hud::Hud::default()
        };
        let parts = status_parts(&hud, FooterModel::Model { window: None }, &theme);
        assert_eq!(parts[0].0, "plan mode (/plan to leave)");
    }

    #[test]
    fn agent_mode_is_left_out_of_the_status() {
        let theme = Theme::default();
        let hud = crate::app::hud::Hud {
            mode: Some(PermissionMode::Agent),
            ..crate::app::hud::Hud::default()
        };
        assert!(status_parts(&hud, FooterModel::Model { window: None }, &theme).is_empty());
    }

    #[test]
    fn checklist_boxes_each_state() {
        let theme = Theme::default();
        let todo = |content: &str, state| TodoView {
            content: content.to_string(),
            state,
            assignee: None,
            task: None,
        };
        let hud = Hud {
            todos: vec![
                todo("read", TodoState::Completed),
                todo("fix", TodoState::InProgress),
                todo("test", TodoState::Pending),
                todo("drop", TodoState::Cancelled),
            ],
            ..Hud::default()
        };
        let lines = todo_checklist(&hud, 40, &theme);
        let text = |line: &Line<'_>| -> String { line.spans.iter().map(|s| &*s.content).collect() };
        let rows: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            rows,
            [
                "⬢ Working on 3 to-dos",
                "  ☒ read",
                "  ▣ fix",
                "  ☐ test",
                "  ☐ drop"
            ]
        );
        let item = |row: usize| lines[row].spans[1].style;
        assert_eq!(item(1).fg, Some(theme.colors.faint));
        assert_eq!(item(2).fg, Some(theme.colors.foreground));
        assert!(item(2).add_modifier.contains(Modifier::BOLD));
        assert_eq!(item(3).fg, Some(theme.colors.muted));
        assert!(item(4).add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn checklist_counts_what_it_leaves_out() {
        let theme = Theme::default();
        let hud = Hud {
            todos: (0..MAX_TODO_ROWS + 2)
                .map(|i| TodoView {
                    content: format!("step {i}"),
                    state: TodoState::Pending,
                    assignee: None,
                    task: None,
                })
                .collect(),
            ..Hud::default()
        };
        let lines = todo_checklist(&hud, 40, &theme);
        assert_eq!(lines.len(), MAX_TODO_ROWS + 2);
        let last: String = lines[MAX_TODO_ROWS + 1]
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert_eq!(last.trim(), "2 more");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
    }
}
