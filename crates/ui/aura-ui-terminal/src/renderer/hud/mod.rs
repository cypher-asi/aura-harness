//! What the agent on screen has used and is working on: the footer under
//! the input, the notice beside it, and the to-do header and rows the
//! to-do drawer draws.
//!
//! ```text
//! qwen3:4b · auto thinking · 23% context used · 2 calls        ● Ready
//! / for commands · @ for files · $ for a terminal · Ctrl+O steps
//! ```

mod notice;

pub(super) use notice::render_notice;

use super::text::display_width;
use crate::app::format::format_tokens;
use crate::app::hud::{Hud, SessionStats};
use crate::events::TodoView;
use crate::mode_command::PermissionMode;
use crate::{App, Theme};
use notice::MIN_NOTICE_WIDTH;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Most todo items listed at once.
pub(super) const MAX_TODO_ROWS: usize = 10;

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

/// `⬢ Working on 3 to-dos` while the agent is `working`, with `Working`
/// shimmering on an animation `frame`; `⬢ 3 to-dos` otherwise.
pub(super) fn checklist_header(
    hud: &Hud,
    theme: &Theme,
    working: bool,
    frame: Option<usize>,
) -> Line<'static> {
    let (_, total) = hud.todo_progress();
    let plural = if total == 1 { "" } else { "s" };
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut header = vec![Span::styled("⬢ ", Style::default().fg(theme.colors.faint))];
    if !working {
        header.push(Span::styled(
            format!("{total} to-do{plural}"),
            label_style.fg(theme.colors.foreground),
        ));
        return Line::from(header);
    }
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
pub(super) fn todo_line(todo: &TodoView, room: usize, theme: &Theme) -> Line<'static> {
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
