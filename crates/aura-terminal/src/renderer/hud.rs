//! What the agent on screen has used and is working on: the status line
//! under the input, its todo progress on the Chat panel's bottom border,
//! and the full todo list that `/todos` opens.

use super::text::display_width;
use crate::app::format::{format_cost, format_tokens};
use crate::app::hud::Hud;
use crate::events::{TodoState, TodoView};
use crate::{App, Theme};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Cells of the context gauge.
const GAUGE_CELLS: u64 = 5;

/// Most todo items listed at once.
const MAX_TODO_ROWS: usize = 10;

/// Columns of the bottom border left for the "lines below" hint.
const BORDER_HINT_ROOM: usize = 20;

const SEPARATOR: &str = " · ";

/// The parts of the status line, most important first.
pub(super) fn status_parts(hud: &Hud, theme: &Theme) -> Vec<(String, Color)> {
    let stats = &hud.stats;
    let mut parts = Vec::new();
    if stats.context_tokens > 0 {
        let mut text = format!("ctx {}", format_tokens(stats.context_tokens));
        let mut color = theme.colors.muted;
        if let (Some(percent), Some(limit)) = (stats.context_percent(), stats.context_limit) {
            text = format!(
                "ctx {percent}% {} {}/{}",
                gauge(percent),
                format_tokens(stats.context_tokens),
                format_tokens(limit)
            );
            color = match percent {
                90.. => theme.colors.error,
                75.. => theme.colors.warning,
                _ => theme.colors.muted,
            };
        }
        parts.push((text, color));
    }
    if let Some(hit) = stats.cache_hit_percent() {
        parts.push((format!("cache {hit}%"), theme.colors.muted));
    }
    if stats.priced {
        parts.push((format_cost(stats.cost_usd), theme.colors.muted));
    }
    if stats.calls > 0 {
        let plural = if stats.calls == 1 { "" } else { "s" };
        parts.push((format!("{} call{plural}", stats.calls), theme.colors.muted));
    }
    let jobs = hud.running_jobs();
    if jobs > 0 {
        parts.push((format!("{jobs} bg"), theme.colors.primary));
    }
    if hud.queued > 0 {
        parts.push((format!("{} queued", hud.queued), theme.colors.primary));
    }
    if stats.compactions > 0 {
        parts.push((
            format!("summarized {}×", stats.compactions),
            theme.colors.muted,
        ));
    }
    parts
}

/// `▰▰▰▱▱` for `percent` full.
fn gauge(percent: u64) -> String {
    let full = (percent * GAUGE_CELLS).div_ceil(100).min(GAUGE_CELLS);
    let full = usize::try_from(full).unwrap_or(0);
    let cells = usize::try_from(GAUGE_CELLS).unwrap_or(0);
    format!("{}{}", "▰".repeat(full), "▱".repeat(cells - full))
}

/// Render the status line right-aligned in `row`, dropping trailing parts
/// that don't fit.
pub(super) fn render_status_line(frame: &mut Frame, row: Rect, app: &App, theme: &Theme) {
    let parts = status_parts(app.hud(), theme);
    let room = usize::from(row.width);
    let mut used = 0;
    let mut spans = Vec::new();
    for (text, color) in parts {
        let sep = if spans.is_empty() { 0 } else { SEPARATOR.len() };
        let width = display_width(&text);
        if used + sep + width > room {
            break;
        }
        if sep > 0 {
            spans.push(Span::styled(
                SEPARATOR,
                Style::default().fg(theme.colors.muted),
            ));
        }
        spans.push(Span::styled(text, Style::default().fg(color)));
        used += sep + width;
    }
    if spans.is_empty() {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        row,
    );
}

/// ` [3/7] ▸ Write the parser ` on the left of the Chat panel's bottom
/// border.
pub(super) fn render_todo_strip(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let hud = app.hud();
    if hud.todos.is_empty() || chat.width < 8 || chat.height < 2 {
        return;
    }
    let (done, total) = hud.todo_progress();
    let room = usize::from(chat.width - 2).saturating_sub(BORDER_HINT_ROOM);
    let count = format!(" [{done}/{total}]");
    let mut spans = vec![Span::styled(
        count.clone(),
        Style::default().fg(theme.colors.primary),
    )];
    if let Some(todo) = hud.current_todo() {
        let label_room = room.saturating_sub(display_width(&count) + 4);
        if label_room > 3 {
            spans.push(Span::styled(
                format!(" ▸ {} ", truncate(&todo.content, label_room)),
                Style::default().fg(theme.colors.foreground),
            ));
        }
    } else {
        spans.push(Span::raw(" "));
    }
    let border_row = Rect {
        x: chat.x + 1,
        y: chat.bottom() - 1,
        width: chat.width - 2,
        height: 1,
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), border_row);
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
        .border_style(Style::default().fg(theme.colors.primary));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn todo_line<'a>(todo: &TodoView, room: usize, theme: &Theme) -> Line<'a> {
    let (mark, style) = match todo.state {
        TodoState::Completed => ("✓", Style::default().fg(theme.colors.success)),
        TodoState::InProgress => ("▸", Style::default().fg(theme.colors.primary)),
        TodoState::Pending => ("○", Style::default().fg(theme.colors.foreground)),
        TodoState::Cancelled => (
            "✗",
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::CROSSED_OUT),
        ),
    };
    let text_style = match todo.state {
        TodoState::Completed => Style::default().fg(theme.colors.muted),
        _ => style,
    };
    Line::from(vec![
        Span::styled(format!(" {mark} "), style),
        Span::styled(truncate(&todo.content, room), text_style),
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
    fn gauge_fills_by_fifths() {
        assert_eq!(gauge(0), "▱▱▱▱▱");
        assert_eq!(gauge(20), "▰▱▱▱▱");
        assert_eq!(gauge(62), "▰▰▰▰▱");
        assert_eq!(gauge(100), "▰▰▰▰▰");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
    }
}
