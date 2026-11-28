//! The CONTEXT panel: what fills the context window of the agent on
//! screen, as one stacked bar and a row per part.
//!
//! ```text
//! ┌ CONTEXT ──────────────────────────────── 25% of 200k ┐
//! │ USED  50.0k of 200k tokens, as the model counted them  │
//! │ ███▓▓▓▓▓▒▒▒▒▒▒▒▒▒▒▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
//! │                                                        │
//! │ PARTS                                                  │
//! │   ■ System prompt      3.0k   1.5%                      │
//! │   ■ Tools              8.0k   4.0%                      │
//! │   ■ Messages          37.0k  18.5%                      │
//! │   ░ Free space         150k  75.0%                      │
//! └────────────────────────────────────────────────────────┘
//! ```
//!
//! Each part has its own colour, the same in the bar and its row; free
//! space is `░` in `faint`. Parts with no tokens are left out of the
//! rows. The meta line is the ring's share, as the footer shows it.

use super::with_notice;
use crate::app::format::format_tokens;
use crate::app::hud::context_usage::{ring, ContextPart, ContextSource, ContextUsage};
use crate::components::{Info, Modal, ModalArea, Value};
use crate::{App, Theme};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    Frame,
};

/// Columns of a part's name, before its tokens.
const LABEL_WIDTH: usize = 16;

/// Draw the panel over the screen.
pub(super) fn render(frame: &mut Frame, app: &App, theme: &Theme) -> ModalArea {
    let screen = frame.area();
    let usage = app.context_usage();
    let mut modal = with_notice(Modal::new("CONTEXT", theme), app, theme);
    if let Some((usage, percent)) = usage
        .as_ref()
        .zip(usage.as_ref().and_then(ContextUsage::percent))
    {
        let window = usage.window.map(format_tokens).unwrap_or_default();
        modal = modal.meta(format!("{} {percent}% of {window}", ring(percent)));
    }
    let width = modal.body_width(screen).max(1);
    let mut info = Info::new(width, theme);
    match &usage {
        None => {
            info.section("USED", None);
            info.text(
                "Nothing yet: the context fills from the agent's first model call.",
                Value::Quiet,
            );
        }
        Some(usage) => fill(&mut info, usage, width, theme),
    }
    if let Some(files) = &app.hud().instructions {
        info.section("PROJECT INSTRUCTIONS", Some("in the system prompt"));
        for file in files {
            info.item(&file.path, Some(&format_tokens(file.bytes as u64 / 4)));
        }
        if files.is_empty() {
            info.none();
        }
    }
    modal.render(frame.buffer_mut(), screen, info.into_lines())
}

/// `usage`'s total, its bar and its parts.
fn fill(info: &mut Info<'_>, usage: &ContextUsage, width: usize, theme: &Theme) {
    let of = usage
        .window
        .map_or_else(String::new, |w| format!(" of {}", format_tokens(w)));
    let source = match usage.source {
        ContextSource::Reported => "as the model counted them",
        ContextSource::Estimated => "estimated, before the model counted any",
    };
    info.section(
        "USED",
        Some(&format!(
            "{}{of} tokens, {source}",
            format_tokens(usage.used)
        )),
    );
    info.indented([bar(usage, width.saturating_sub(4), theme)]);
    info.section(
        "PARTS",
        Some("measured from their text; messages are the rest"),
    );
    let rows: Vec<Line<'static>> = usage
        .parts
        .iter()
        .filter(|(_, tokens)| *tokens > 0)
        .map(|&(part, tokens)| part_row(usage, part, tokens, theme))
        .collect();
    info.indented(rows);
}

/// The stacked bar, `width` cells: each part's share of the window in
/// its colour, free space as `░`.
fn bar(usage: &ContextUsage, width: usize, theme: &Theme) -> Line<'static> {
    let of = usage.window.unwrap_or(usage.used).max(1);
    let mut spans = Vec::new();
    let mut drawn = 0;
    let mut sum = 0;
    for &(part, tokens) in &usage.parts {
        sum += tokens;
        // Cells by running total, so rounding never drifts the end.
        let to = usize::try_from(u128::from(sum) * width as u128 / u128::from(of))
            .unwrap_or(width)
            .min(width);
        let cells = to.saturating_sub(drawn);
        if cells > 0 {
            let (glyph, color) = cell(part, theme);
            spans.push(Span::styled(
                glyph.repeat(cells),
                Style::default().fg(color),
            ));
        }
        drawn = to;
    }
    if drawn < width {
        let (glyph, color) = cell(ContextPart::Free, theme);
        spans.push(Span::styled(
            glyph.repeat(width - drawn),
            Style::default().fg(color),
        ));
    }
    Line::from(spans)
}

/// `■ Messages          37.0k  18.5%`: the part's colour, name, tokens
/// and share of the window.
fn part_row(usage: &ContextUsage, part: ContextPart, tokens: u64, theme: &Theme) -> Line<'static> {
    let (glyph, color) = cell(part, theme);
    let mark = if part == ContextPart::Free {
        glyph
    } else {
        "■"
    };
    let permille = usage.share_permille(tokens);
    Line::from(vec![
        Span::styled(format!("{mark} "), Style::default().fg(color)),
        Span::styled(
            format!("{:<LABEL_WIDTH$}", part.label()),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(
            format!(
                "{:>7}  {:>3}.{}%",
                format_tokens(tokens),
                permille / 10,
                permille % 10
            ),
            Style::default().fg(theme.colors.muted),
        ),
    ])
}

/// The glyph and colour of `part` in the bar.
pub(super) fn cell(part: ContextPart, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    match part {
        ContextPart::System => ("█", colors.secondary),
        ContextPart::Tools => ("█", colors.primary),
        ContextPart::Mcp => ("█", colors.pending),
        ContextPart::Subagents => ("█", colors.warning),
        ContextPart::Skills => ("█", colors.text),
        ContextPart::Messages => ("█", colors.success),
        ContextPart::Free => ("░", colors.faint),
    }
}
