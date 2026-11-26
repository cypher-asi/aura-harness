//! The LOG tab's header, column heads and keys.

use super::super::benchmarks::fit;
use super::super::text::display_width;
use super::rows::columns;
use super::{COLUMN_HEADS, COPIED_BUTTON, COPY_BUTTON, KEYS};
use crate::app::run_log::LogState;
use crate::events::{LogStatus, LogView};
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

/// The header's three lines, and where `Copy log` was drawn: the run's
/// status and title with the button, its totals with where the log is
/// stored, and a rule.
pub(super) fn header_lines(
    state: &LogState,
    view: &LogView,
    area: Rect,
    spinner: &'static str,
    theme: &Theme,
) -> (Vec<Line<'static>>, Option<Rect>) {
    let width = usize::from(area.width);
    let root = view.root();
    let status = root.map_or(LogStatus::Ok, |r| r.status);
    let title = root.map_or("Run log", |r| r.title.as_str());
    let detail = root.map_or("", |r| r.detail.as_str());
    let (title_line, lead, button) = title_line(state, (status, title), area, spinner, theme);
    let info = info_line(view, (status, detail), lead, width, theme);
    (vec![title_line, info, rule(width, theme)], button)
}

/// The run's status and title with `Copy log` at the right when it fits;
/// also the columns before the title, and where the button was drawn.
fn title_line(
    state: &LogState,
    (status, title): (LogStatus, &str),
    area: Rect,
    spinner: &'static str,
    theme: &Theme,
) -> (Line<'static>, usize, Option<Rect>) {
    let width = usize::from(area.width);
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let (label, color) = if state.just_copied() {
        (COPIED_BUTTON, theme.colors.success)
    } else {
        (COPY_BUTTON, theme.colors.text)
    };
    let button_width = display_width(label);
    let mut spans = vec![root_glyph(status, spinner, theme), Span::raw(" ")];
    let lead: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let room = width.saturating_sub(lead + button_width + 2);
    spans.push(Span::styled(fit(title, room), bold));
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let button = (width > used + button_width).then(|| {
        spans.push(Span::raw(" ".repeat(width - used - button_width)));
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(color)
                .bg(theme.colors.surface)
                .add_modifier(Modifier::BOLD),
        ));
        Rect {
            x: area.x + u16::try_from(width - button_width).unwrap_or(0),
            y: area.y,
            width: u16::try_from(button_width).unwrap_or(0),
            height: 1,
        }
    });
    (Line::from(spans), lead, button)
}

/// The run's totals under its title, `live` while it runs, and where the
/// log is stored at the right when a useful part of it fits.
fn info_line(
    view: &LogView,
    (status, detail): (LogStatus, &str),
    lead: usize,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.colors.muted);
    let faint = Style::default().fg(theme.colors.faint);
    let live = status == LogStatus::Running;
    let live_width = if live { display_width(" · live") } else { 0 };
    let mut info = vec![
        Span::raw(" ".repeat(lead)),
        Span::styled(fit(detail, width.saturating_sub(lead + live_width)), muted),
    ];
    if live {
        if !detail.is_empty() {
            info.push(Span::styled(" · ", muted));
        }
        info.push(Span::styled(
            "live",
            Style::default().fg(theme.colors.success),
        ));
    }
    let used: usize = info.iter().map(|s| display_width(&s.content)).sum();
    // The path goes at the right when a useful part of it fits.
    let left = width.saturating_sub(used + 3);
    if left >= 12 {
        let path = fit_start(&view.path, left);
        let gap = width - used - display_width(&path);
        info.push(Span::raw(" ".repeat(gap)));
        info.push(Span::styled(path, faint));
    }
    Line::from(info)
}

/// The run's status before its title: a spinner while it runs, else a
/// dot in the colour of how it went.
fn root_glyph(status: LogStatus, spinner: &'static str, theme: &Theme) -> Span<'static> {
    let (glyph, color) = match status {
        LogStatus::Running => (spinner, theme.colors.pending),
        LogStatus::Ok => ("●", theme.colors.success),
        LogStatus::Warn => ("●", theme.colors.warning),
        LogStatus::Error => ("●", theme.colors.error),
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// The heads of the columns, over the columns of the rows below.
pub(super) fn column_heads(width: usize, theme: &Theme) -> Line<'static> {
    let heads = columns(COLUMN_HEADS, width);
    if heads.trim().is_empty() {
        return Line::default();
    }
    Line::from(vec![
        Span::raw(" ".repeat(width.saturating_sub(display_width(&heads)))),
        Span::styled(heads, Style::default().fg(theme.colors.faint)),
    ])
}

/// The keys at the bottom, as many as fit in `width`.
pub(super) fn keys_line(width: usize, theme: &Theme) -> Line<'static> {
    let key = Style::default().fg(theme.colors.muted);
    let what = Style::default().fg(theme.colors.faint);
    let mut spans = Vec::new();
    let mut used = 0;
    for (i, (k, does)) in KEYS.iter().enumerate() {
        let gap = if i == 0 { 0 } else { 2 };
        let needs = gap + display_width(k) + 1 + display_width(does);
        if used + needs > width {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*k, key));
        spans.push(Span::styled(format!(" {does}"), what));
        used += needs;
    }
    Line::from(spans)
}

fn rule(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.colors.faint),
    ))
}

/// `text` cut from the start to `max` columns, as `…/runs/x/log.json`.
fn fit_start(text: &str, max: usize) -> String {
    if display_width(text) <= max || max == 0 {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        kept.push(c);
        used += w;
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}
