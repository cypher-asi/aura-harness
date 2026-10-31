//! Inline markdown: emphasis, strikethrough, code spans, links, escapes.
//!
//! Delimiters that never close render literally, so half-streamed replies
//! (`**still typ`) show their raw text until the closer arrives.

use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

/// Parse inline markdown in `text` into styled spans layered on `base`.
pub(crate) fn parse_inline(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut parser = Inline {
        pieces: Vec::new(),
        theme,
    };
    parser.parse(text, base);
    parser
        .pieces
        .into_iter()
        .map(|(text, style)| Span::styled(text, style))
        .collect()
}

fn code_style(base: Style, theme: &Theme) -> Style {
    base.fg(theme.colors.primary)
}

fn link_style(base: Style, theme: &Theme) -> Style {
    base.fg(theme.colors.warning)
        .add_modifier(Modifier::UNDERLINED)
}

struct Inline<'t> {
    pieces: Vec<(String, Style)>,
    theme: &'t Theme,
}

impl Inline<'_> {
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.pieces.last_mut() {
            Some((last, last_style)) if *last_style == style => last.push_str(text),
            _ => self.pieces.push((text.to_string(), style)),
        }
    }

    fn parse(&mut self, text: &str, style: Style) {
        let bytes = text.as_bytes();
        let mut literal_start = 0;
        let mut i = 0;
        while i < text.len() {
            let matched = match bytes[i] {
                b'\\' => escaped_char(text, i).map(|c| {
                    self.push(&text[literal_start..i], style);
                    self.push(c, style);
                    i + 2
                }),
                b'`' => code_span(text, i).map(|(code, end)| {
                    self.push(&text[literal_start..i], style);
                    self.push(code, code_style(style, self.theme));
                    end
                }),
                b'*' | b'_' | b'~' => emphasis(text, i).map(|(inner, modifier, end)| {
                    self.push(&text[literal_start..i], style);
                    self.parse(inner, style.add_modifier(modifier));
                    end
                }),
                b'[' | b'!' => link(text, i).map(|link| {
                    self.push(&text[literal_start..i], style);
                    self.push_link(&link, style);
                    link.end
                }),
                b'<' => autolink(text, i).map(|(url, end)| {
                    self.push(&text[literal_start..i], style);
                    self.push(url, link_style(style, self.theme));
                    end
                }),
                _ => None,
            };
            match matched {
                Some(end) => {
                    i = end;
                    literal_start = end;
                }
                // Skip a whole unmatched delimiter run so `**` can't be
                // re-read as two `*` openers.
                None if matches!(bytes[i], b'*' | b'_' | b'~' | b'`') => {
                    i += run_length(bytes, i);
                }
                None => i += text[i..].chars().next().map_or(1, char::len_utf8),
            }
        }
        self.push(&text[literal_start..], style);
    }

    fn push_link(&mut self, link: &Link<'_>, style: Style) {
        let label_style = link_style(style, self.theme);
        if link.is_image {
            self.push("[image] ", style.fg(self.theme.colors.muted));
        }
        if link.label.is_empty() {
            self.push(link.url, label_style);
            return;
        }
        self.parse(link.label, label_style);
        if !link.url.is_empty() && link.url != link.label {
            self.push(
                &format!(" ({})", link.url),
                style.fg(self.theme.colors.muted),
            );
        }
    }
}

fn run_length(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .take_while(|&&b| b == bytes[start])
        .count()
}

fn char_before(text: &str, i: usize) -> Option<char> {
    text[..i].chars().next_back()
}

fn char_at(text: &str, i: usize) -> Option<char> {
    text.get(i..).and_then(|rest| rest.chars().next())
}

/// `\*` and friends: the escaped ASCII punctuation character.
fn escaped_char(text: &str, i: usize) -> Option<&str> {
    let next = *text.as_bytes().get(i + 1)?;
    next.is_ascii_punctuation().then(|| &text[i + 1..i + 2])
}

/// A code span opened by the backtick run at `i`: its content and the
/// index just past the closing run.
fn code_span(text: &str, i: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let ticks = run_length(bytes, i);
    let content_start = i + ticks;
    let mut j = content_start;
    while j < bytes.len() {
        if bytes[j] == b'`' {
            let run = run_length(bytes, j);
            if run == ticks {
                let code = &text[content_start..j];
                let code = match code.strip_prefix(' ').and_then(|c| c.strip_suffix(' ')) {
                    Some(inner) if !inner.trim().is_empty() => inner,
                    _ => code,
                };
                return Some((code, j + run));
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// Emphasis opened by the delimiter run at `i`: the inner text, the
/// modifier it applies, and the index just past the closing run.
fn emphasis(text: &str, i: usize) -> Option<(&str, Modifier, usize)> {
    let bytes = text.as_bytes();
    let delimiter = bytes[i];
    let run = run_length(bytes, i);
    let modifier = match (delimiter, run) {
        (b'~', 2) => Modifier::CROSSED_OUT,
        (b'*' | b'_', 1) => Modifier::ITALIC,
        (b'*' | b'_', 2) => Modifier::BOLD,
        (b'*' | b'_', 3) => Modifier::BOLD | Modifier::ITALIC,
        _ => return None,
    };
    let intraword_ok = delimiter != b'_';
    if char_at(text, i + run).is_none_or(char::is_whitespace) {
        return None;
    }
    if !intraword_ok && char_before(text, i).is_some_and(char::is_alphanumeric) {
        return None;
    }

    let content_start = i + run;
    let mut j = content_start;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if escaped_char(text, j).is_some() => j += 2,
            b'`' => match code_span(text, j) {
                Some((_, end)) => j = end,
                None => j += run_length(bytes, j),
            },
            b if b == delimiter => {
                let closing = run_length(bytes, j);
                let closes = closing == run
                    && j > content_start
                    && !char_before(text, j).is_some_and(char::is_whitespace)
                    && (intraword_ok || !char_at(text, j + run).is_some_and(char::is_alphanumeric));
                if closes {
                    return Some((&text[content_start..j], modifier, j + run));
                }
                j += closing;
            }
            _ => j += 1,
        }
    }
    None
}

struct Link<'a> {
    label: &'a str,
    url: &'a str,
    is_image: bool,
    end: usize,
}

/// `[label](url "title")` or `![alt](url)` starting at `i`.
fn link(text: &str, i: usize) -> Option<Link<'_>> {
    let bytes = text.as_bytes();
    let is_image = bytes[i] == b'!';
    let open = if is_image { i + 1 } else { i };
    if bytes.get(open) != Some(&b'[') {
        return None;
    }
    let label_end = matching(bytes, open, b'[', b']')?;
    if bytes.get(label_end + 1) != Some(&b'(') {
        return None;
    }
    let url_end = matching(bytes, label_end + 1, b'(', b')')?;
    let target = text[label_end + 2..url_end].trim();
    let url = target.split_whitespace().next().unwrap_or("");
    let url = url
        .strip_prefix('<')
        .and_then(|u| u.strip_suffix('>'))
        .unwrap_or(url);
    Some(Link {
        label: &text[open + 1..label_end],
        url,
        is_image,
        end: url_end + 1,
    })
}

/// Index of the bracket closing the one at `open`, honoring nesting and
/// backslash escapes.
fn matching(bytes: &[u8], open: usize, open_byte: u8, close_byte: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = open;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 1,
            b if b == open_byte => depth += 1,
            b if b == close_byte => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// `<https://example.com>`: the URL and the index past `>`.
fn autolink(text: &str, i: usize) -> Option<(&str, usize)> {
    let rest = &text[i + 1..];
    let close = rest.find('>')?;
    let url = &rest[..close];
    let is_url = ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
        && !url.contains(char::is_whitespace);
    is_url.then_some((url, i + 1 + close + 1))
}
