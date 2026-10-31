//! Text and code segment rendering for the chat panel.

use super::markdown::render_markdown;
use super::text::wrap_words;
use crate::components::{CodeBlock, MessageRole};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// `AURA  12:00:00` — the line above a message's body.
pub(super) fn message_header<'a>(
    nick: &str,
    nick_color: ratatui::style::Color,
    timestamp: &str,
    theme: &Theme,
) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            nick.to_string(),
            Style::default().fg(nick_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {timestamp}"),
            Style::default().fg(theme.colors.muted),
        ),
    ])
}

pub(super) const fn message_style<'a>(
    role: MessageRole,
    theme: &Theme,
) -> (&'a str, ratatui::style::Color, ratatui::style::Color) {
    match role {
        MessageRole::User => ("YOU", theme.colors.success, theme.colors.muted),
        MessageRole::Assistant => ("AURA", theme.colors.muted, theme.colors.foreground),
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
