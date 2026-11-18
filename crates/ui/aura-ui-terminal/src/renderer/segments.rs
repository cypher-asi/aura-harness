//! Text and code segment rendering for the chat panel.

use super::markdown::{render_markdown_flow, wrap_spans};
use super::text::{display_width, wrap_words, Measure};
use crate::app::format::{format_cost, format_tokens};
use crate::components::{CodeBlock, MessageRole};
use crate::events::CallUsage;
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// `[claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 · 27.9k tok · $0.040]`
/// — the dim line closing a turn. `meta` (see [`reply_meta`]) loses its
/// trailing parts when it doesn't fit in `width`.
pub(super) fn reply_footer(meta: &[String], width: usize, theme: &Theme) -> Option<Line<'static>> {
    let text = fit_meta(meta, width.saturating_sub(2))?;
    Some(Line::from(Span::styled(
        format!("[{text}]"),
        Style::default().fg(theme.colors.muted),
    )))
}

/// `> fix the login bug` or `* policy: write_file allowed by …`: `prefix`
/// before the first row of `text`, later rows indented under it, the way
/// a terminal echoes what was typed after its prompt.
pub(super) fn prefixed_lines(
    (prefix, prefix_color): (&str, ratatui::style::Color),
    text: &str,
    text_color: ratatui::style::Color,
    width: usize,
    measure: Measure,
) -> Vec<Line<'static>> {
    let indent = display_width(prefix);
    let style = Style::default().fg(text_color);
    let mut lines = plain_lines(text, width.saturating_sub(indent).max(1), style, measure);
    if lines.is_empty() {
        lines.push(Line::default());
    }
    for (i, line) in lines.iter_mut().enumerate() {
        let lead = if i == 0 {
            Span::styled(prefix.to_string(), Style::default().fg(prefix_color))
        } else {
            Span::raw(" ".repeat(indent))
        };
        line.spans.insert(0, lead);
    }
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

/// Parts of a reply's footer, most important first: the turn's models,
/// then the tokens and cost of its calls when known.
pub(super) fn reply_meta(model: Option<&str>, usage: Option<CallUsage>) -> Vec<String> {
    let mut parts: Vec<String> = model.map(str::to_string).into_iter().collect();
    if let Some(usage) = usage {
        parts.push(format!("{} tok", format_tokens(usage.tokens)));
        parts.extend(usage.cost_usd.map(format_cost));
    }
    parts
}

/// The prompt before a message's first row, its color, and the color of
/// the message's text. The agent's replies have no prompt.
pub(super) const fn message_style<'a>(
    role: MessageRole,
    theme: &Theme,
) -> (&'a str, ratatui::style::Color, ratatui::style::Color) {
    match role {
        MessageRole::User => (USER_PROMPT, theme.colors.muted, theme.colors.foreground),
        MessageRole::Assistant => ("", theme.colors.muted, theme.colors.text),
        MessageRole::System => (NOTICE_PROMPT, theme.colors.muted, theme.colors.muted),
    }
}

/// Before a prompt the user sent, as in the input row.
pub(super) const USER_PROMPT: &str = "→ ";
/// Before a system message.
pub(super) const NOTICE_PROMPT: &str = "* ";

/// Render a text segment. Assistant replies are rendered as markdown;
/// user and system text is shown as written, word-wrapped after its
/// prompt (see [`message_style`]) when `first` is the message's first.
///
/// Prose, the user's or the agent's flowing text, is as wide as `measure`
/// makes it. Returns, for each added line, whether it is prose: `Some` of
/// the columns its marker, quote bar or indent takes. Tables, rules and
/// system messages are not.
pub(super) fn render_text_segment(
    lines: &mut Vec<Line<'_>>,
    text: &str,
    (role, first): (MessageRole, bool),
    width: usize,
    theme: &Theme,
    measure: Measure,
) -> Vec<Option<u16>> {
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    if role == MessageRole::Assistant {
        let base_style = Style::default().fg(msg_color);
        let (markdown, flow) = render_markdown_flow(text, width, base_style, theme, measure);
        lines.extend(markdown);
        return flow;
    }
    let (measure, flow) = if role == MessageRole::User {
        (
            measure,
            Some(u16::try_from(display_width(prompt)).unwrap_or(0)),
        )
    } else {
        (Measure::GRID, None)
    };
    let indent = " ".repeat(display_width(prompt));
    let lead = if first { prompt } else { indent.as_str() };
    let plain = prefixed_lines((lead, prompt_color), text, msg_color, width, measure);
    let prose = vec![flow; plain.len()];
    lines.extend(plain);
    prose
}

fn plain_lines(text: &str, width: usize, style: Style, measure: Measure) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for content_line in text.lines() {
        if content_line.is_empty() {
            lines.push(Line::default());
        } else if measure.is_grid() {
            lines.extend(
                wrap_words(content_line, width)
                    .into_iter()
                    .map(|wrapped| Line::from(Span::styled(wrapped, style))),
            );
        } else {
            let spans = [Span::styled(content_line.to_owned(), style)];
            lines.extend(wrap_spans(&spans, width, (&[], &[]), measure));
        }
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
