//! What the agent on screen has used and is working on: the footer under
//! the input, the notice beside it, and the full todo list that `/todos`
//! opens.
//!
//! ```text
//! qwen3:4b · auto thinking · 23% context used · 2 calls        ● Ready
//! / for commands · @ for files · $ for a terminal · Ctrl+O steps
//! ```

mod notice;

pub(super) use notice::render_notice;

use super::text::{display_width, wrap_words};
use crate::app::format::format_tokens;
use crate::app::hud::{Hud, SessionStats};
use crate::events::{TodoState, TodoView};
use crate::mode_command::PermissionMode;
use crate::{App, Theme};
use notice::MIN_NOTICE_WIDTH;
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
    push_mode_part(&mut parts, hud.mode, theme);
    push_context_part(&mut parts, stats, model, theme);
    push_usage_parts(&mut parts, stats, model, theme);
    push_work_parts(&mut parts, hud, theme);
    parts
}

/// The permission mode, unless it is the default agent mode.
fn push_mode_part(parts: &mut Vec<(String, Color)>, mode: Option<PermissionMode>, theme: &Theme) {
    // Modes are a setting, not a warning: they take the theme's accent.
    match mode {
        None | Some(PermissionMode::Agent) => {}
        Some(PermissionMode::Plan) => {
            parts.push((
                "plan mode (/plan to leave)".to_string(),
                theme.colors.success,
            ));
        }
        Some(mode) => parts.push((format!("{mode} mode"), theme.colors.success)),
    }
}

/// How much of the model's context window is used, on one model.
fn push_context_part(
    parts: &mut Vec<(String, Color)>,
    stats: &SessionStats,
    model: FooterModel,
    theme: &Theme,
) {
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
}

/// The cache hits, cost, models used and calls made.
fn push_usage_parts(
    parts: &mut Vec<(String, Color)>,
    stats: &SessionStats,
    model: FooterModel,
    theme: &Theme,
) {
    if let Some(hit) = stats.cache_hit_percent() {
        parts.push((format!("{hit}% cached"), theme.colors.muted));
    }
    if let Some(cost) = stats.cost_label() {
        parts.push((cost, theme.colors.muted));
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
}

/// The background jobs, queued prompts and summaries so far.
fn push_work_parts(parts: &mut Vec<(String, Color)>, hud: &Hud, theme: &Theme) {
    let stats = &hud.stats;
    let jobs = hud.running_jobs();
    if jobs > 0 {
        parts.push((format!("{jobs} in background"), theme.colors.foreground));
    }
    let queued = hud.queued();
    if queued > 0 {
        parts.push((format!("{queued} queued"), theme.colors.foreground));
    }
    if stats.compactions > 0 {
        parts.push((
            format!("summarized {}×", stats.compactions),
            theme.colors.muted,
        ));
    }
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
        // In-progress status (iterations, tool runs): the theme's accent.
        ("◐".to_string(), theme.colors.success, theme.colors.success)
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

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

/// Render the footer into `area`: the model, its thinking level and what
/// the agent has used on the left, with its status at the right. Parts
/// that don't fit drop from the end, so the stats give way before the
/// thinking level, and it gives way before the model. Returns where a
/// notice can go on the row: the gap between the two, or everything left
/// of the status when that gap is too narrow to read.
pub(super) fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> Rect {
    if area.height == 0 || area.width == 0 {
        return Rect { height: 0, ..area };
    }
    let row = super::host_tab::split_footer(frame, Rect { height: 1, ..area }, app, theme);
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
    parts.push(super::thinking_indicator_spans(app, theme));
    parts.extend(stats.map(|(text, color)| vec![Span::styled(text, Style::default().fg(color))]));
    let stats = fitting(parts, room, theme);
    let stats_width = spans_width(&stats);
    frame.render_widget(Paragraph::new(Line::from(stats)), row);
    frame.render_widget(
        Paragraph::new(Line::from(status)).alignment(Alignment::Right),
        row,
    );

    let left_of_status = Rect {
        width: u16::try_from(room).unwrap_or(u16::MAX),
        ..row
    };
    let used = u16::try_from(stats_width + STATUS_GAP).unwrap_or(u16::MAX);
    let gap = Rect {
        x: left_of_status.x.saturating_add(used),
        width: left_of_status.width.saturating_sub(used),
        ..left_of_status
    };
    if gap.width >= MIN_NOTICE_WIDTH {
        gap
    } else {
        left_of_status
    }
}

/// The full todo list over the bottom of the Chat panel, while `/todos`
/// has it open.
pub(super) fn render_todo_panel(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let hud = app.hud();
    // Todos are an agent feature; the channel app's chat and BENCH have
    // none.
    if app.channel_app()
        || app.no_agent_on_screen()
        || !app.todos_open()
        || hud.todos.is_empty()
        || chat.height < 3
        || chat.width < 8
    {
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
/// while a turn is in flight. With an animation `frame`, a band of light
/// sweeps across `Working` to show the work goes on. Short, it lists the
/// first [`MAX_TODO_ROWS`] items cut to one row each and counts the rest;
/// `expanded`, it lists every item in full and offers to show less.
pub(super) fn todo_checklist(
    hud: &Hud,
    width: usize,
    theme: &Theme,
    frame: Option<usize>,
    expanded: bool,
) -> TodoChecklist {
    if hud.todos.is_empty() {
        return TodoChecklist::default();
    }
    let mut lines = vec![checklist_header(hud, theme, frame)];
    let room = width.saturating_sub(2);
    let footer = if expanded {
        lines.extend(
            hud.todos
                .iter()
                .flat_map(|todo| todo_lines_in_full(todo, room, theme)),
        );
        Some("  show less".to_string())
    } else {
        lines.extend(
            hud.todos
                .iter()
                .take(MAX_TODO_ROWS)
                .map(|todo| todo_line(todo, room, theme)),
        );
        let more = hud.todos.len().saturating_sub(MAX_TODO_ROWS);
        (more > 0).then(|| format!("  {more} more"))
    };
    let has_footer = footer.is_some();
    lines.extend(
        footer.map(|text| Line::from(Span::styled(text, Style::default().fg(theme.colors.muted)))),
    );
    TodoChecklist { lines, has_footer }
}

/// The rows of the chat's to-do checklist.
#[derive(Debug, Default)]
pub(super) struct TodoChecklist {
    pub(super) lines: Vec<Line<'static>>,
    /// Whether the last row is `N more` or `show less`, which with the
    /// header is what lights up under the mouse.
    pub(super) has_footer: bool,
}

/// `⬢ Working on 3 to-dos`, with `Working` shimmering on an animation
/// `frame`.
fn checklist_header(hud: &Hud, theme: &Theme, frame: Option<usize>) -> Line<'static> {
    let (_, total) = hud.todo_progress();
    let plural = if total == 1 { "" } else { "s" };
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut header = vec![Span::styled("⬢ ", Style::default().fg(theme.colors.faint))];
    match frame {
        Some(frame) => header.extend(crate::animation::shimmer_spans(
            "Working",
            frame,
            label_style,
            (theme.colors.muted, theme.colors.foreground),
        )),
        None => header.push(Span::styled(
            "Working",
            label_style.fg(theme.colors.foreground),
        )),
    }
    header.push(Span::styled(
        format!(" on {total} to-do{plural}"),
        Style::default().fg(theme.colors.secondary),
    ));
    Line::from(header)
}

/// `▣ Write the parser`: a to-do's box, under the header's `⬢`, then its
/// text.
fn todo_line(todo: &TodoView, room: usize, theme: &Theme) -> Line<'static> {
    let mark = super::plans::marker(todo.state);
    let mark_color = super::plans::marker_color(todo.state, theme);
    let text_style = super::plans::todo_style(todo.state, theme);
    let agent = todo
        .assignee
        .as_deref()
        .map(|agent| format!("  → {agent}"))
        .filter(|agent| display_width(agent) + 8 <= room)
        .unwrap_or_default();
    let room = room.saturating_sub(display_width(&agent));
    Line::from(vec![
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(truncate(&todo.content, room), text_style),
        Span::styled(agent, Style::default().fg(theme.colors.faint)),
    ])
}

/// A to-do's box and its whole text, wrapped to `room` columns, with
/// later rows indented to start under the text rather than the box. The
/// `→ agent` it is assigned to ends the last row when it fits there.
fn todo_lines_in_full(todo: &TodoView, room: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mark = super::plans::marker(todo.state);
    let mark_color = super::plans::marker_color(todo.state, theme);
    let text_style = super::plans::todo_style(todo.state, theme);
    let lead = format!("{mark} ");
    let indent = " ".repeat(display_width(&lead));
    let rows = wrap_words(&todo.content, room);
    let last = rows.len().saturating_sub(1);
    rows.into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mut spans = vec![if i == 0 {
                Span::styled(lead.clone(), Style::default().fg(mark_color))
            } else {
                Span::raw(indent.clone())
            }];
            let agent = todo
                .assignee
                .as_deref()
                .filter(|_| i == last)
                .map(|agent| format!("  → {agent}"))
                .filter(|agent| display_width(&text) + display_width(agent) <= room);
            spans.push(Span::styled(text, text_style));
            spans.extend(
                agent.map(|agent| Span::styled(agent, Style::default().fg(theme.colors.faint))),
            );
            Line::from(spans)
        })
        .collect()
}

/// `text` cut to `width` columns, ending in `…` when cut.
pub(super) fn truncate(text: &str, width: usize) -> String {
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
mod tests;
