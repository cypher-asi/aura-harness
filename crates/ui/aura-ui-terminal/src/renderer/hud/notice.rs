//! The notice beside the footer, and its longer form raised over a panel.

use super::super::text::{display_width, wrap_words};
use crate::app::NotificationType;
use crate::{App, Theme};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use std::time::Duration;

/// Narrowest gap in the footer a notice goes in beside the stats.
pub(super) const MIN_NOTICE_WIDTH: u16 = 24;

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
pub(in crate::renderer) fn render_notice(
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
    let lines = wrap_words(text, width);
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
    draw_rule(frame, panel, rule);
    let body_lines = raised_lines(lines, &lead, (first, shown), theme);
    frame.render_widget(Clear, body);
    frame.render_widget(Paragraph::new(body_lines), body);
    #[allow(clippy::cast_precision_loss)]
    let read = NOTICE_LEAD + Duration::from_secs_f32(overflow as f32 * row_read);
    Some(if overflow == 0 { Duration::ZERO } else { read })
}

/// A rule across `panel` at `rule`, in the color of the panel's border.
fn draw_rule(frame: &mut Frame, panel: Rect, rule: Rect) {
    // The rule takes the color of the panel's border, focused or not.
    let border = frame.buffer_mut()[(panel.x, rule.y)].style();
    let rule_line = format!(
        "├{}┤",
        "─".repeat(usize::from(panel.width.saturating_sub(2)))
    );
    frame.render_widget(Paragraph::new(Span::styled(rule_line, border)), rule);
}

/// The `shown` rows of `lines` from `first`, `lead` before the top one
/// and later ones indented under its text.
fn raised_lines<'a>(
    lines: Vec<String>,
    lead: &Span<'a>,
    (first, shown): (usize, usize),
    theme: &Theme,
) -> Vec<Line<'a>> {
    let indent = " ".repeat(display_width(&lead.content));
    let text_style = Style::default().fg(theme.colors.foreground);
    lines
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
        .collect()
}

/// The glyph that leads a notification of `kind`, and its color.
pub(in crate::renderer) const fn notice_icon(
    kind: NotificationType,
    theme: &Theme,
) -> (&'static str, Color) {
    match kind {
        NotificationType::Success => ("✓", theme.colors.success),
        NotificationType::Warning => ("!", theme.colors.warning),
        NotificationType::Error => ("✗", theme.colors.error),
    }
}

/// The `width` columns of `text` after its first `skip`, with a space for
/// half of a wide character cut at either end.
pub(super) fn columns(text: &str, skip: usize, width: usize) -> String {
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
