//! Text and code segment rendering for the chat panel.

use super::markdown::render_markdown;
use super::text::{display_width, wrap_words};
use crate::app::format::{format_cost, format_tokens};
use crate::components::{CodeBlock, MessageRole};
use crate::events::CallUsage;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// `AURA  12:00:00      claude-opus-4-7 · 12.4k tok · $0.042` — the line
/// above a message's body. `meta` (see [`reply_meta`]) sits at the right
/// edge of `width`; its trailing parts are dropped when it doesn't fit.
pub(super) fn message_header<'a>(
    nick: &str,
    nick_color: ratatui::style::Color,
    timestamp: &str,
    meta: &[String],
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let muted = Style::default().fg(theme.colors.muted);
    let detail = format!("  {timestamp}");
    let used = display_width(nick) + display_width(&detail);
    let mut spans = vec![
        Span::styled(
            nick.to_string(),
            Style::default().fg(nick_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(detail, muted),
    ];
    if let Some(right) = fit_meta(meta, width.saturating_sub(used + 2)) {
        let gap = width.saturating_sub(used + display_width(&right));
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(right, muted));
    }
    Line::from(spans)
}

/// `* 07:48:20  policy: write_file allowed by …` — a one-line notice with
/// its header, wrapped under the text when it runs past `width`.
pub(super) fn notice_lines(
    nick: &str,
    nick_color: ratatui::style::Color,
    timestamp: &str,
    text: &str,
    text_color: ratatui::style::Color,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut header = message_header(nick, nick_color, timestamp, &[], width, theme);
    header.spans.push(Span::raw("  "));
    let indent = header.width();
    let style = Style::default().fg(text_color);
    let mut rows = wrap_words(text, width.saturating_sub(indent).max(1)).into_iter();
    header
        .spans
        .push(Span::styled(rows.next().unwrap_or_default(), style));
    let mut lines = vec![header];
    lines.extend(rows.map(|row| {
        Line::from(vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(row, style),
        ])
    }));
    lines
}

/// The longest leading run of `meta` that fits in `room` columns, joined
/// with ` · `; failing that, the first part cut short.
fn fit_meta(meta: &[String], room: usize) -> Option<String> {
    for len in (1..=meta.len()).rev() {
        let text = meta[..len].join(" · ");
        if display_width(&text) <= room {
            return Some(text);
        }
    }
    let first = meta.first()?;
    if room < 2 {
        return None;
    }
    let mut cut = String::new();
    for ch in first.chars() {
        if display_width(&cut) + display_width(ch.encode_utf8(&mut [0; 4])) + 1 > room {
            break;
        }
        cut.push(ch);
    }
    cut.push('…');
    Some(cut)
}

/// Right-hand header parts for a reply, most important first: the model,
/// then the call's tokens and cost when known.
pub(super) fn reply_meta(model: Option<&str>, usage: Option<CallUsage>) -> Vec<String> {
    let mut parts: Vec<String> = model.map(str::to_string).into_iter().collect();
    if let Some(usage) = usage {
        parts.push(format!("{} tok", format_tokens(usage.tokens)));
        parts.extend(usage.cost_usd.map(format_cost));
    }
    parts
}

pub(super) const fn message_style<'a>(
    role: MessageRole,
    theme: &Theme,
) -> (&'a str, ratatui::style::Color, ratatui::style::Color) {
    match role {
        MessageRole::User => ("YOU", theme.colors.success, theme.colors.muted),
        MessageRole::Assistant => ("AURA", theme.colors.muted, theme.colors.text),
        MessageRole::System => ("*", theme.colors.muted, theme.colors.muted),
    }
}

/// Render a text segment. Assistant replies are rendered as markdown;
/// user and system text is shown as written, word-wrapped.
pub(super) fn render_text_segment(
    lines: &mut Vec<Line<'_>>,
    text: &str,
    msg_color: ratatui::style::Color,
    width: usize,
    role: MessageRole,
    theme: &Theme,
) {
    let base_style = Style::default().fg(msg_color);
    if role == MessageRole::Assistant {
        lines.extend(render_markdown(text, width, base_style, theme));
    } else {
        lines.extend(plain_lines(text, width, base_style));
    }
}

fn plain_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for content_line in text.lines() {
        if content_line.is_empty() {
            lines.push(Line::default());
            continue;
        }
        lines.extend(
            wrap_words(content_line, width)
                .into_iter()
                .map(|wrapped| Line::from(Span::styled(wrapped, style))),
        );
    }
    lines
}

pub(super) fn render_code_segment(
    lines: &mut Vec<Line<'_>>,
    language: &str,
    code: &str,
    width: usize,
    theme: &Theme,
) {
    lines.push(Line::from(""));
    let code_block = CodeBlock::new(language, code);
    for code_line in code_block.render(theme, width) {
        lines.push(Line::from(
            code_line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.to_string(), s.style))
                .collect::<Vec<_>>(),
        ));
    }
    lines.push(Line::from(""));
}
