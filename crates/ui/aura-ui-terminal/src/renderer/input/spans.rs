//! Styled spans for what is typed and for the prompt before it.

use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

/// A chat draft: [`prose_spans`], with every `/plan` word in the theme's
/// accent (bold) so it reads as "this goes in plan mode", like the badge.
pub(in crate::renderer) fn chat_spans<'a>(text: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    let ranges = crate::app::plan_ranges(text);
    if ranges.is_empty() {
        return prose_spans(text, theme);
    }
    let accent = Style::default()
        .fg(theme.colors.success)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut last = 0;
    for range in ranges {
        spans.extend(prose_spans(&text[last..range.start], theme));
        spans.push(Span::styled(&text[range.clone()], accent));
        last = range.end;
    }
    spans.extend(prose_spans(&text[last..], theme));
    spans
}

/// Typed text in the reply color, with `@path` mentions and `[image N]`
/// chips picked out so it is clear what will be attached.
pub(in crate::renderer) fn prose_spans<'a>(text: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    let plain = Style::default().fg(theme.colors.text);
    let chip = Style::default()
        .fg(theme.colors.foreground)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_word_start = i == 0 || text[..i].ends_with(char::is_whitespace);
        let token_len = if at_word_start && rest.starts_with('@') && rest.len() > 1 {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        } else {
            image_chip_len(rest)
        };
        if token_len == 0 {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if start < i {
            spans.push(Span::styled(&text[start..i], plain));
        }
        spans.push(Span::styled(&text[i..i + token_len], chip));
        i += token_len;
        start = i;
    }
    if start < text.len() {
        spans.push(Span::styled(&text[start..], plain));
    }
    spans
}

/// Length of an `[image N]` chip at the start of `text`, or 0.
fn image_chip_len(text: &str) -> usize {
    let Some(rest) = text.strip_prefix("[image ") else {
        return 0;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && rest[digits..].starts_with(']') {
        "[image ".len() + digits + 1
    } else {
        0
    }
}

/// `[user@host dir]$ ` with the user in the success color and `@host` muted;
/// any other prompt is drawn in `color`.
pub(in crate::renderer) fn prompt_spans<'a>(
    prompt: &'a str,
    color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let plain = || vec![Span::styled(prompt, Style::default().fg(color))];
    let Some(body) = prompt.strip_prefix('[') else {
        return plain();
    };
    let Some(at) = body.find('@') else {
        return plain();
    };
    let host_end = body[at..].find(' ').map_or(body.len(), |i| at + i);
    vec![
        Span::styled("[", Style::default().fg(color)),
        Span::styled(&body[..at], Style::default().fg(theme.colors.success)),
        Span::styled(&body[at..host_end], Style::default().fg(theme.colors.muted)),
        Span::styled(&body[host_end..], Style::default().fg(color)),
    ]
}
