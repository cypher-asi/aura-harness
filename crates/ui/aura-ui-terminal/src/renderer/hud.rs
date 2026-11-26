//! What the agent on screen has used and is working on: the footer under
//! the input, the notice beside it, and the to-do header and rows the
//! to-do drawer draws.
//!
//! ```text
//! qwen3:4b · auto thinking · 23% context used · 2 calls        ● Ready
//! / for commands · @ for files · $ for a terminal · Ctrl+O steps
//! ```

use super::text::display_width;
use crate::app::format::format_tokens;
use crate::app::hud::Hud;
use crate::app::NotificationType;
use crate::events::TodoView;
use crate::mode_command::PermissionMode;
use crate::{App, Theme};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use std::time::Duration;

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
    // Modes are a setting, not a warning: they take the theme's accent.
    match hud.mode {
        None | Some(PermissionMode::Agent) => {}
        Some(PermissionMode::Plan) => {
            parts.push((
                "plan mode (/plan to leave)".to_string(),
                theme.colors.success,
            ));
        }
        Some(mode) => parts.push((format!("{mode} mode"), theme.colors.success)),
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

/// Narrowest gap in the footer a notice goes in beside the stats.
const MIN_NOTICE_WIDTH: u16 = 24;

/// How long a notice too long for its row waits before it scrolls.
const NOTICE_LEAD: Duration = Duration::from_millis(1500);

/// How fast a notice too long for its row scrolls, in columns a second.
const NOTICE_SPEED: f32 = 16.0;

/// Render the notification on one row of `area`, the bottom-right of the
/// screen that no panel covers. Text that fits sits at the right. Longer
/// text wraps over the bottom third of `panel`, the lowest panel of the
/// right column, when there is one; otherwise it waits, then scrolls left
/// until its end shows. Returns how long the text takes to scroll to its
/// end, zero when it fits.
pub(super) fn render_notice(
    frame: &mut Frame,
    (area, panel): (Rect, Option<Rect>),
    app: &App,
    theme: &Theme,
) -> Option<Duration> {
    let (msg, kind) = app.notification()?;
    if area.height == 0 || area.width == 0 {
        return None;
    }
    let row = Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    };
    let (icon, color) = notice_icon(*kind, theme);
    let lead = format!("{icon} ");
    let room = usize::from(row.width).saturating_sub(display_width(&lead));
    let text = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    let overflow = display_width(&text).saturating_sub(room);
    if overflow > 0 {
        let lead = Span::styled(lead.clone(), Style::default().fg(color));
        let raised = panel.and_then(|panel| {
            render_raised_notice(frame, panel, (lead, &text), app.notification_age(), theme)
        });
        if raised.is_some() {
            return raised;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let read = NOTICE_LEAD + Duration::from_secs_f32(overflow as f32 / NOTICE_SPEED);

    let scrolled = app.notification_age().saturating_sub(NOTICE_LEAD);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let offset = ((scrolled.as_secs_f32() * NOTICE_SPEED) as usize).min(overflow);
    let line = Line::from(vec![
        Span::styled(lead, Style::default().fg(color)),
        Span::styled(
            columns(&text, offset, room),
            Style::default().fg(theme.colors.foreground),
        ),
    ]);
    frame.render_widget(Clear, row);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), row);
    Some(if overflow == 0 { Duration::ZERO } else { read })
}

/// Narrowest a notice is wrapped to over a panel.
const MIN_RAISED_WIDTH: usize = 16;

/// Render `text` wrapped over the bottom third of `panel`, inside its
/// borders under a rule across it, with `lead` before the top row shown.
/// Text with more rows than the third holds waits, then scrolls up a row
/// at a time, as fast as a row reads at [`NOTICE_SPEED`]. Returns how long
/// it takes to scroll to its end, or `None` when the third has no room.
fn render_raised_notice(
    frame: &mut Frame,
    panel: Rect,
    (lead, text): (Span<'_>, &str),
    age: Duration,
    theme: &Theme,
) -> Option<Duration> {
    let lead_width = display_width(&lead.content);
    let rows = usize::from(panel.height / 3).saturating_sub(1);
    // A column clear of each border.
    let width = usize::from(panel.width).saturating_sub(4 + lead_width);
    if rows == 0 || width < MIN_RAISED_WIDTH {
        return None;
    }
    let lines = super::text::wrap_words(text, width);
    let shown = lines.len().min(rows);
    let overflow = lines.len() - shown;
    #[allow(clippy::cast_precision_loss)]
    let row_read = width as f32 / NOTICE_SPEED;
    let scrolled = age.saturating_sub(NOTICE_LEAD).as_secs_f32();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let first = ((scrolled / row_read) as usize).min(overflow);

    let shown_rows = u16::try_from(shown).unwrap_or(u16::MAX);
    let body = Rect {
        x: panel.x + 1,
        y: panel.bottom() - 1 - shown_rows,
        width: panel.width - 2,
        height: shown_rows,
    };
    let rule = Rect {
        x: panel.x,
        y: body.y - 1,
        width: panel.width,
        height: 1,
    };
    // The rule takes the color of the panel's border, focused or not.
    let border = frame.buffer_mut()[(panel.x, rule.y)].style();
    let rule_line = format!(
        "├{}┤",
        "─".repeat(usize::from(panel.width.saturating_sub(2)))
    );
    frame.render_widget(Paragraph::new(Span::styled(rule_line, border)), rule);

    let indent = " ".repeat(lead_width);
    let text_style = Style::default().fg(theme.colors.foreground);
    let body_lines: Vec<Line<'_>> = lines
        .into_iter()
        .skip(first)
        .take(shown)
        .enumerate()
        .map(|(i, line)| {
            let lead = if i == 0 {
                lead.clone()
            } else {
                Span::raw(indent.clone())
            };
            Line::from(vec![Span::raw(" "), lead, Span::styled(line, text_style)])
        })
        .collect();
    frame.render_widget(Clear, body);
    frame.render_widget(Paragraph::new(body_lines), body);
    #[allow(clippy::cast_precision_loss)]
    let read = NOTICE_LEAD + Duration::from_secs_f32(overflow as f32 * row_read);
    Some(if overflow == 0 { Duration::ZERO } else { read })
}

const fn notice_icon(kind: NotificationType, theme: &Theme) -> (&'static str, Color) {
    match kind {
        NotificationType::Success => ("✓", theme.colors.success),
        NotificationType::Warning => ("!", theme.colors.warning),
        NotificationType::Error => ("✗", theme.colors.error),
    }
}

/// The `width` columns of `text` after its first `skip`, with a space for
/// half of a wide character cut at either end.
fn columns(text: &str, skip: usize, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let (mut at, mut used) = (0, 0);
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        let start = at;
        at += w;
        if at <= skip {
            continue;
        }
        if start < skip {
            out.push(' ');
            used += 1;
            continue;
        }
        if used + w > width {
            if used < width {
                out.push(' ');
            }
            break;
        }
        out.push(c);
        used += w;
    }
    out
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
mod tests {
    use super::*;
    use crate::events::TodoState;

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
    fn non_agent_modes_are_drawn_in_the_accent_color() {
        let theme = Theme::default();
        for mode in [
            PermissionMode::Plan,
            PermissionMode::ReadOnly,
            PermissionMode::Debug,
        ] {
            let hud = crate::app::hud::Hud {
                mode: Some(mode),
                ..crate::app::hud::Hud::default()
            };
            let parts = status_parts(&hud, FooterModel::Model { window: None }, &theme);
            assert_eq!(parts[0].1, theme.colors.success, "{mode:?}");
        }
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
    fn the_status_marks_a_partly_priced_total_and_drops_an_unpriced_one() {
        use crate::app::hud::SessionStats;
        let theme = Theme::default();
        let cost_part = |stats: SessionStats| {
            let hud = Hud {
                stats,
                ..Hud::default()
            };
            status_parts(&hud, FooterModel::Gate, &theme)
                .into_iter()
                .map(|(text, _)| text)
                .find(|text| text.contains('$'))
        };
        let stats = |priced, unpriced_calls| SessionStats {
            calls: 3,
            cost_usd: 0.25,
            priced,
            unpriced_calls,
            ..SessionStats::default()
        };
        assert_eq!(cost_part(stats(true, 0)).as_deref(), Some("$0.250"));
        assert_eq!(cost_part(stats(true, 2)).as_deref(), Some("$0.250+?"));
        assert_eq!(cost_part(stats(false, 3)), None);
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
        let mut lines = vec![checklist_header(&hud, &theme, true, None)];
        lines.extend(hud.todos.iter().map(|todo| todo_line(todo, 38, &theme)));
        let text = |line: &Line<'_>| -> String { line.spans.iter().map(|s| &*s.content).collect() };
        let rows: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            rows,
            [
                "⬢ Working on 3 to-dos",
                "☒ read",
                "▣ fix",
                "☐ test",
                "☐ drop"
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
    fn checklist_working_label_shimmers_when_animated() {
        let theme = Theme::default();
        let hud = Hud {
            todos: vec![TodoView {
                content: "fix".to_string(),
                state: TodoState::InProgress,
                assignee: None,
                task: None,
            }],
            ..Hud::default()
        };
        let header = |frame| checklist_header(&hud, &theme, true, frame).spans;
        let text = |spans: &[Span<'_>]| -> String { spans.iter().map(|s| &*s.content).collect() };
        // Mid-pass, the band lights part of `Working` and leaves the rest.
        let lit = header(Some(4));
        assert_eq!(text(&lit), "⬢ Working on 1 to-do");
        assert!(lit.len() > 3, "band splits the label: {lit:?}");
        assert!(lit[1..lit.len() - 1]
            .iter()
            .all(|s| s.style.add_modifier.contains(Modifier::BOLD)));
        // Without animation the label is one steady span.
        let steady = header(None);
        assert_eq!(steady.len(), 3);
        assert_eq!(steady[1].style.fg, Some(theme.colors.foreground));
    }

    #[test]
    fn columns_slice_by_width_and_pad_cut_wide_characters() {
        assert_eq!(columns("hello world", 6, 5), "world");
        assert_eq!(columns("hello world", 0, 5), "hello");
        assert_eq!(columns("a漢字b", 2, 3), " 字");
        assert_eq!(columns("a漢字b", 0, 2), "a ");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
    }
}
