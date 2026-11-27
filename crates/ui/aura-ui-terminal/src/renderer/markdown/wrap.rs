//! Word wrapping for styled spans.

use super::inline::Linked;
use crate::renderer::text::{display_width, Measure};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::ops::Range;

/// A piece of text in one style, and the link it is part of, if any.
type Piece = (String, Style, Option<usize>);

/// A run of either whitespace or non-whitespace text, possibly spanning
/// several styles (e.g. `**bold**,` is one word in two styles).
struct Token {
    pieces: Vec<Piece>,
    width: f32,
    is_space: bool,
}

/// Where a link fell in wrapped lines: one per line it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkPiece {
    /// Which of the wrapped lines
    pub(crate) line: usize,
    /// Display columns, the line's prefix counted
    pub(crate) columns: Range<u16>,
    /// Which link, as numbered in the [`Linked`] wrapped
    pub(crate) link: usize,
}

/// Wrap `spans` into lines at most `width` display columns wide, text
/// as wide as `measure` makes it.
///
/// The first line starts with `first_prefix` and every later line with
/// `rest_prefix`; prefixes stay on the grid and count against `width`.
/// Words only split when they are wider than a whole line. A `width` of 0
/// means unbounded.
pub(crate) fn wrap_spans(
    spans: &[Span<'static>],
    width: usize,
    prefixes: (&[Span<'static>], &[Span<'static>]),
    measure: Measure,
) -> Vec<Line<'static>> {
    let pieces = spans
        .iter()
        .map(|span| (span.content.to_string(), span.style, None))
        .collect();
    wrap_pieces(pieces, width, prefixes, measure).0
}

/// [`wrap_spans`] for `linked` spans, with where each link fell.
pub(crate) fn wrap_linked(
    linked: Linked,
    width: usize,
    prefixes: (&[Span<'static>], &[Span<'static>]),
    measure: Measure,
) -> (Vec<Line<'static>>, Vec<LinkPiece>) {
    let pieces = linked
        .spans
        .into_iter()
        .zip(linked.links)
        .map(|(span, link)| (span.content.into_owned(), span.style, link))
        .collect();
    wrap_pieces(pieces, width, prefixes, measure)
}

fn wrap_pieces(
    pieces: Vec<Piece>,
    width: usize,
    (first_prefix, rest_prefix): (&[Span<'static>], &[Span<'static>]),
    measure: Measure,
) -> (Vec<Line<'static>>, Vec<LinkPiece>) {
    let mut wrapper = Wrapper {
        lines: Vec::new(),
        links: Vec::new(),
        current: Vec::new(),
        current_width: 0.0,
        available: 0.0,
        width,
        rest_prefix,
        measure,
    };
    wrapper.start_line(first_prefix);

    let mut pending_space: Option<Token> = None;
    for token in tokenize(pieces, measure) {
        if token.is_space {
            if wrapper.current_width > 0.0 {
                pending_space = Some(token);
            }
            continue;
        }
        let space_width = pending_space.as_ref().map_or(0.0, |space| space.width);
        if wrapper.current_width > 0.0
            && wrapper.current_width + space_width + token.width > wrapper.available
        {
            wrapper.break_line();
            pending_space = None;
        } else if let Some(space) = pending_space.take() {
            wrapper.push_token(space);
        }
        if token.width > wrapper.available {
            wrapper.push_split(token);
        } else {
            wrapper.push_token(token);
        }
    }
    wrapper.finish()
}

struct Wrapper<'p> {
    lines: Vec<Line<'static>>,
    links: Vec<LinkPiece>,
    current: Vec<Piece>,
    current_width: f32,
    available: f32,
    width: usize,
    rest_prefix: &'p [Span<'static>],
    measure: Measure,
}

impl Wrapper<'_> {
    #[allow(clippy::cast_precision_loss)]
    fn start_line(&mut self, prefix: &[Span<'static>]) {
        let prefix_width: usize = prefix.iter().map(Span::width).sum();
        self.current = prefix
            .iter()
            .map(|span| (span.content.to_string(), span.style, None))
            .collect();
        self.current_width = 0.0;
        self.available = if self.width == 0 {
            f32::INFINITY
        } else {
            self.width.saturating_sub(prefix_width).max(1) as f32
        };
    }

    fn break_line(&mut self) {
        self.end_line();
        self.start_line(self.rest_prefix);
    }

    fn push_token(&mut self, token: Token) {
        for (text, style, link) in token.pieces {
            push_piece(&mut self.current, &text, style, link);
        }
        self.current_width += token.width;
    }

    /// Hard-break a word that is wider than a whole line.
    fn push_split(&mut self, token: Token) {
        for (text, style, link) in token.pieces {
            for c in text.chars() {
                let mut buf = [0u8; 4];
                let c = c.encode_utf8(&mut buf);
                let char_width = self.measure.width(c, style);
                if self.current_width > 0.0 && self.current_width + char_width > self.available {
                    self.break_line();
                }
                push_piece(&mut self.current, c, style, link);
                self.current_width += char_width;
            }
        }
    }

    /// Put the line so far after the others, noting where its links are.
    fn end_line(&mut self) {
        let pieces = std::mem::take(&mut self.current);
        let line = self.lines.len();
        let mut column = 0u16;
        for (text, _, link) in &pieces {
            let width = u16::try_from(display_width(text)).unwrap_or(u16::MAX);
            let end = column.saturating_add(width);
            if let Some(link) = *link {
                match self.links.last_mut() {
                    Some(last)
                        if last.line == line && last.link == link && last.columns.end == column =>
                    {
                        last.columns.end = end;
                    }
                    _ => self.links.push(LinkPiece {
                        line,
                        columns: column..end,
                        link,
                    }),
                }
            }
            column = end;
        }
        self.lines.push(to_line(pieces));
    }

    fn finish(mut self) -> (Vec<Line<'static>>, Vec<LinkPiece>) {
        self.end_line();
        (self.lines, self.links)
    }
}

fn tokenize(pieces: Vec<Piece>, measure: Measure) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for (text, style, link) in pieces {
        for c in text.chars() {
            let is_space = c.is_whitespace();
            let starts_token = tokens.last().is_none_or(|last| last.is_space != is_space);
            if starts_token {
                tokens.push(Token {
                    pieces: Vec::new(),
                    width: 0.0,
                    is_space,
                });
            }
            if let Some(token) = tokens.last_mut() {
                let mut buf = [0u8; 4];
                push_piece(&mut token.pieces, c.encode_utf8(&mut buf), style, link);
            }
        }
    }
    for token in &mut tokens {
        token.width = token
            .pieces
            .iter()
            .map(|(text, style, _)| measure.width(text, *style))
            .sum();
    }
    tokens
}

/// Append `text`, merging it into the last piece when the style and the
/// link match.
fn push_piece(pieces: &mut Vec<Piece>, text: &str, style: Style, link: Option<usize>) {
    match pieces.last_mut() {
        Some((last, last_style, last_link)) if *last_style == style && *last_link == link => {
            last.push_str(text);
        }
        _ => pieces.push((text.to_string(), style, link)),
    }
}

/// The line of `pieces`, adjacent pieces of one style in one span as a
/// link's edge no longer matters once it is noted.
fn to_line(pieces: Vec<Piece>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(pieces.len());
    for (text, style, _) in pieces {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(&text),
            _ => spans.push(Span::styled(text, style)),
        }
    }
    Line::from(spans)
}
