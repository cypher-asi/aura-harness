//! Word wrapping for styled spans.

use crate::renderer::text::Measure;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// A run of either whitespace or non-whitespace text, possibly spanning
/// several styles (e.g. `**bold**,` is one word in two styles).
struct Token {
    pieces: Vec<(String, Style)>,
    width: f32,
    is_space: bool,
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
    (first_prefix, rest_prefix): (&[Span<'static>], &[Span<'static>]),
    measure: Measure,
) -> Vec<Line<'static>> {
    let mut wrapper = Wrapper {
        lines: Vec::new(),
        current: Vec::new(),
        current_width: 0.0,
        available: 0.0,
        width,
        rest_prefix,
        measure,
    };
    wrapper.start_line(first_prefix);

    let mut pending_space: Option<Token> = None;
    for token in tokenize(spans, measure) {
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
    current: Vec<(String, Style)>,
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
            .map(|span| (span.content.to_string(), span.style))
            .collect();
        self.current_width = 0.0;
        self.available = if self.width == 0 {
            f32::INFINITY
        } else {
            self.width.saturating_sub(prefix_width).max(1) as f32
        };
    }

    fn break_line(&mut self) {
        let pieces = std::mem::take(&mut self.current);
        self.lines.push(to_line(pieces));
        self.start_line(self.rest_prefix);
    }

    fn push_token(&mut self, token: Token) {
        for (text, style) in token.pieces {
            push_piece(&mut self.current, &text, style);
        }
        self.current_width += token.width;
    }

    /// Hard-break a word that is wider than a whole line.
    fn push_split(&mut self, token: Token) {
        for (text, style) in token.pieces {
            for c in text.chars() {
                let mut buf = [0u8; 4];
                let c = c.encode_utf8(&mut buf);
                let char_width = self.measure.width(c, style);
                if self.current_width > 0.0 && self.current_width + char_width > self.available {
                    self.break_line();
                }
                push_piece(&mut self.current, c, style);
                self.current_width += char_width;
            }
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        let pieces = std::mem::take(&mut self.current);
        self.lines.push(to_line(pieces));
        self.lines
    }
}

fn tokenize(spans: &[Span<'static>], measure: Measure) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for span in spans {
        for c in span.content.chars() {
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
                push_piece(&mut token.pieces, c.encode_utf8(&mut buf), span.style);
            }
        }
    }
    for token in &mut tokens {
        token.width = token
            .pieces
            .iter()
            .map(|(text, style)| measure.width(text, *style))
            .sum();
    }
    tokens
}

/// Append `text`, merging it into the last piece when the style matches.
fn push_piece(pieces: &mut Vec<(String, Style)>, text: &str, style: Style) {
    match pieces.last_mut() {
        Some((last, last_style)) if *last_style == style => last.push_str(text),
        _ => pieces.push((text.to_string(), style)),
    }
}

fn to_line(pieces: Vec<(String, Style)>) -> Line<'static> {
    Line::from(
        pieces
            .into_iter()
            .map(|(text, style)| Span::styled(text, style))
            .collect::<Vec<_>>(),
    )
}
