//! Text and code segment rendering for the chat panel.

use super::markdown::parse_markdown_line;
use super::text::wrap_words;
use crate::components::{CodeBlock, MessageRole};
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

pub(super) const fn message_style<'a>(
    role: MessageRole,
    theme: &Theme,
) -> (&'a str, ratatui::style::Color, ratatui::style::Color) {
    match role {
        MessageRole::User => ("YOU", theme.colors.foreground, theme.colors.primary),
        MessageRole::Assistant => ("AURA", theme.colors.foreground, theme.colors.muted),
        MessageRole::System => ("*", theme.colors.muted, theme.colors.muted),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_text_segment(
    lines: &mut Vec<Line<'_>>,
    text: &str,
    is_first: &mut bool,
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    msg_color: ratatui::style::Color,
    prefix_width: usize,
    first_line_width: usize,
    continuation_width: usize,
    role: MessageRole,
    theme: &Theme,
) {
    for content_line in text.lines() {
        if content_line.is_empty() {
            if *is_first {
                lines.push(line_with_prefix(timestamp, nick, nick_color, &[]));
                *is_first = false;
            } else {
                lines.push(Line::from(""));
            }
            continue;
        }
        let wrap_width = if *is_first {
            first_line_width
        } else {
            continuation_width
        };
        let wrapped = wrap_words(content_line, wrap_width);
        for wrapped_line in wrapped {
            let base_style = Style::default().fg(msg_color);
            let content_spans = if role == MessageRole::Assistant {
                parse_markdown_line(&wrapped_line, base_style, theme)
            } else {
                vec![Span::styled(wrapped_line, base_style)]
            };
            if *is_first {
                let mut spans = vec![
                    Span::styled(
                        format!("[{timestamp}] "),
                        Style::default().fg(theme.colors.muted),
                    ),
                    Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                    Span::raw(" "),
                ];
                spans.extend(content_spans);
                lines.push(Line::from(spans));
                *is_first = false;
            } else {
                let indent = " ".repeat(prefix_width);
                let mut spans = vec![Span::raw(indent)];
                spans.extend(content_spans);
                lines.push(Line::from(spans));
            }
        }
    }
}

pub(super) fn line_with_prefix<'a>(
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    extra: &[Span<'a>],
) -> Line<'a> {
    let muted = ratatui::style::Color::Rgb(136, 136, 136);
    let mut spans = vec![
        Span::styled(format!("[{timestamp}] "), Style::default().fg(muted)),
        Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
    ];
    spans.extend_from_slice(extra);
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_code_segment(
    lines: &mut Vec<Line<'_>>,
    language: &str,
    code: &str,
    is_first: &mut bool,
    timestamp: &str,
    nick: &str,
    nick_color: ratatui::style::Color,
    prefix_width: usize,
    continuation_width: usize,
    theme: &Theme,
) {
    if *is_first {
        lines.push(line_with_prefix(timestamp, nick, nick_color, &[]));
        *is_first = false;
    }
    lines.push(Line::from(""));
    let code_block = CodeBlock::new(language, code);
    let code_lines = code_block.render(theme, continuation_width);
    let indent = " ".repeat(prefix_width);
    for code_line in code_lines {
        let mut line_spans = vec![Span::raw(indent.clone())];
        line_spans.extend(
            code_line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.to_string(), s.style)),
        );
        lines.push(Line::from(line_spans));
    }
    lines.push(Line::from(""));
}
