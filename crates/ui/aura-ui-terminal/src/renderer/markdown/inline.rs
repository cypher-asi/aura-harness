//! Inline markdown: emphasis, strikethrough, code spans, links, escapes.
//!
//! Delimiters that never close render literally, so half-streamed replies
//! (`**still typ`) show their raw text until the closer arrives.
//!
//! Links are drawn in the link color, and the ones a click may open (see
//! [`is_openable_url`]) are kept with their URL, so the chat can find
//! them under the pointer. A web address written out in prose is a link
//! too, without the sentence's punctuation after it (see [`bare_url`]).

use super::urls::bare_url;
use crate::app::is_openable_url;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};
use std::sync::Arc;

/// Inline markdown parsed into spans, with the links among them.
#[derive(Debug, Default)]
pub(crate) struct Linked {
    pub(crate) spans: Vec<Span<'static>>,
    /// For each span, which of `urls` it links to, if any
    pub(crate) links: Vec<Option<usize>>,
    /// Where each link goes
    pub(crate) urls: Vec<Arc<str>>,
}

impl Linked {
    /// Put `more` after these spans, its links numbered after these.
    pub(crate) fn append(&mut self, more: Self) {
        let offset = self.urls.len();
        self.spans.extend(more.spans);
        self.links
            .extend(more.links.into_iter().map(|link| link.map(|i| i + offset)));
        self.urls.extend(more.urls);
    }

    /// Spans that link nowhere.
    pub(crate) fn plain(spans: Vec<Span<'static>>) -> Self {
        Self {
            links: vec![None; spans.len()],
            spans,
            urls: Vec::new(),
        }
    }
}

/// Parse inline markdown in `text` into styled spans layered on `base`.
pub(crate) fn parse_inline(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    parse_linked(text, base, theme).spans
}

/// [`parse_inline`], keeping which spans are links a click may open.
pub(crate) fn parse_linked(text: &str, base: Style, theme: &Theme) -> Linked {
    let mut parser = Inline {
        pieces: Vec::new(),
        urls: Vec::new(),
        link: None,
        theme,
    };
    parser.parse(text, base);
    let (spans, links) = parser
        .pieces
        .into_iter()
        .map(|(text, style, link)| (Span::styled(text, style), link))
        .unzip();
    Linked {
        spans,
        links,
        urls: parser.urls,
    }
}

/// Inline code is gray on its shaded cell, a step below the body text, so
/// it reads apart from the prose around it.
fn code_style(base: Style, theme: &Theme) -> Style {
    base.fg(theme.colors.secondary).bg(theme.colors.surface)
}

/// Bold body text is drawn brighter than the body so it reads as emphasis;
/// bold inside links, code or colored headings keeps its color.
fn emphasis_style(base: Style, modifier: Modifier, theme: &Theme) -> Style {
    let style = base.add_modifier(modifier);
    if modifier.contains(Modifier::BOLD) && base.fg == Some(theme.colors.text) {
        style.fg(theme.colors.foreground)
    } else {
        style
    }
}

/// Links are gray (`muted`), like the other clickable text, and not
/// underlined: the chat underlines the one the pointer is on, so the
/// underline says a click opens it. A label's ` (url)` is part of the
/// same link and shares its gray.
fn link_style(base: Style, theme: &Theme) -> Style {
    base.fg(theme.colors.muted)
}

struct Inline<'t> {
    pieces: Vec<(String, Style, Option<usize>)>,
    urls: Vec<Arc<str>>,
    /// The link what is pushed now is part of
    link: Option<usize>,
    theme: &'t Theme,
}

impl Inline<'_> {
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        let link = self.link;
        match self.pieces.last_mut() {
            Some((last, last_style, last_link)) if *last_style == style && *last_link == link => {
                last.push_str(text);
            }
            _ => self.pieces.push((text.to_string(), style, link)),
        }
    }

    /// A new link to `url`, when a click may open it and it is not inside
    /// another link.
    fn new_link(&mut self, url: &str) -> Option<usize> {
        if self.link.is_some() || !is_openable_url(url) {
            return None;
        }
        self.urls.push(Arc::from(url));
        Some(self.urls.len() - 1)
    }

    /// Push what `push` does as part of `link`, if there is one.
    fn push_in(&mut self, link: Option<usize>, push: impl FnOnce(&mut Self)) {
        let outer = self.link;
        self.link = link.or(outer);
        push(self);
        self.link = outer;
    }

    fn parse(&mut self, text: &str, style: Style) {
        let bytes = text.as_bytes();
        let mut literal_start = 0;
        let mut i = 0;
        while i < text.len() {
            match self.inline_at(text, (i, literal_start), style) {
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

    /// Push the inline element starting at `i`, after the literal text
    /// from `literal_start`, and return where it ends; `None` when none
    /// starts there.
    fn inline_at(
        &mut self,
        text: &str,
        (i, literal_start): (usize, usize),
        style: Style,
    ) -> Option<usize> {
        let (end, element) = match text.as_bytes()[i] {
            b'\\' => escaped_char(text, i).map(|c| (i + 2, Element::Literal(c)))?,
            b'`' => code_span(text, i).map(|(code, end)| (end, Element::Code(code)))?,
            b'*' | b'_' | b'~' => emphasis(text, i)
                .map(|(inner, modifier, end)| (end, Element::Emphasis(inner, modifier)))?,
            b'[' | b'!' => link(text, i).map(|link| (link.end, Element::Link(link)))?,
            b'<' => autolink(text, i).map(|(url, end)| (end, Element::Url(url)))?,
            b'h' | b'H' if self.link.is_none() => {
                bare_url(text, i).map(|(url, end)| (end, Element::Url(url)))?
            }
            _ => return None,
        };
        self.push(&text[literal_start..i], style);
        match element {
            Element::Literal(c) => self.push(c, style),
            Element::Code(code) => self.push(code, code_style(style, self.theme)),
            Element::Emphasis(inner, modifier) => {
                self.parse(inner, emphasis_style(style, modifier, self.theme));
            }
            Element::Link(link) => self.push_link(&link, style),
            Element::Url(url) => {
                let link = self.new_link(url);
                let style = link_style(style, self.theme);
                self.push_in(link, |parser| parser.push(url, style));
            }
        }
        Some(end)
    }

    /// The label in the link color, then a muted ` (url)` when the URL
    /// is not the label. Both are the one link; the parentheses are not.
    fn push_link(&mut self, link: &Link<'_>, style: Style) {
        let label_style = link_style(style, self.theme);
        if link.is_image {
            self.push("[image] ", style.fg(self.theme.colors.muted));
        }
        let target = self.new_link(link.url);
        if link.label.is_empty() {
            self.push_in(target, |parser| parser.push(link.url, label_style));
            return;
        }
        self.push_in(target, |parser| parser.parse(link.label, label_style));
        if !link.url.is_empty() && link.url != link.label {
            let muted = style.fg(self.theme.colors.muted);
            self.push(" (", muted);
            self.push_in(target, |parser| parser.push(link.url, muted));
            self.push(")", muted);
        }
    }
}

/// An inline element found in the text.
enum Element<'a> {
    /// An escaped character, drawn as written
    Literal(&'a str),
    Code(&'a str),
    Emphasis(&'a str, Modifier),
    Link(Link<'a>),
    /// A URL that is its own label
    Url(&'a str),
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
