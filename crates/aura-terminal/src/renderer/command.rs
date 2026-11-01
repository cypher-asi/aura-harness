//! Shell command rendering: highlighted command lines and terminal output.

use super::segments::{message_header, message_style};
use crate::app::shell_input::{tokens, TokenKind};
use crate::components::{MessageRole, ShellRun};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

/// `text` as a command line, colored by token.
pub(super) fn command_spans<'a>(text: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    tokens(text)
        .into_iter()
        .map(|(kind, token)| Span::styled(token, token_style(kind, theme)))
        .collect()
}

fn token_style(kind: TokenKind, theme: &Theme) -> Style {
    let style = Style::default();
    match kind {
        TokenKind::Space => style,
        TokenKind::ForcePrefix => style.fg(theme.colors.warning).add_modifier(Modifier::BOLD),
        TokenKind::Program => style.fg(theme.colors.success).add_modifier(Modifier::BOLD),
        TokenKind::Flag => style.fg(theme.colors.warning),
        TokenKind::Operator => style
            .fg(theme.colors.secondary)
            .add_modifier(Modifier::BOLD),
        TokenKind::Quoted => style
            .fg(theme.colors.foreground)
            .add_modifier(Modifier::ITALIC),
        TokenKind::Arg => style.fg(theme.colors.foreground),
    }
}

/// Chat lines for a shell command: the `YOU` header, the command, then the
/// output below the command as a terminal would show it.
pub(super) fn shell_lines(
    run: &ShellRun,
    timestamp: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (nick, nick_color, _) = message_style(MessageRole::User, theme);
    let mut lines = vec![message_header(nick, nick_color, timestamp, theme)];

    let echo = owned(command_spans(&run.command, theme));
    lines.extend(wrap_spans(echo, width).into_iter().map(Line::from));

    let output = run.output.as_deref().unwrap_or_default();
    let output = output.trim_end_matches('\n');
    if !output.is_empty() {
        let base = Style::default().fg(theme.colors.foreground);
        for line in output.split('\n') {
            lines.extend(
                wrap_spans(crate::ansi::line_spans(line, base), width)
                    .into_iter()
                    .map(Line::from),
            );
        }
    }

    if let Some(code) = run.exit_code.filter(|code| *code != 0) {
        lines.push(Line::from(Span::styled(
            format!("exit {code}"),
            Style::default()
                .fg(theme.colors.error)
                .add_modifier(Modifier::DIM),
        )));
    }
    lines
}

fn owned(spans: Vec<Span<'_>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

/// Break spans into rows at most `width` columns wide, keeping whitespace so
/// indentation and columns survive. Words move to the next row whole unless
/// they are wider than a row; whitespace at a break is dropped.
pub(super) fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![spans];
    }
    let mut rows = Rows {
        rows: vec![Vec::new()],
        used: 0,
        width,
    };
    for token in tokenize(spans) {
        let overflows = rows.used + token.width > width;
        if token.is_space {
            if overflows {
                if rows.used > 0 {
                    rows.break_row();
                }
                continue;
            }
        } else if overflows && rows.used > 0 {
            rows.break_row();
        }
        rows.push(token);
    }
    rows.rows
}

/// A run of either whitespace or non-whitespace text, possibly spanning
/// several styles.
struct Token {
    pieces: Vec<(String, Style)>,
    width: usize,
    is_space: bool,
}

fn tokenize(spans: Vec<Span<'static>>) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for span in spans {
        for c in span.content.chars() {
            let is_space = c.is_whitespace();
            if tokens.last().is_none_or(|last| last.is_space != is_space) {
                tokens.push(Token {
                    pieces: Vec::new(),
                    width: 0,
                    is_space,
                });
            }
            if let Some(token) = tokens.last_mut() {
                match token.pieces.last_mut() {
                    Some((text, style)) if *style == span.style => text.push(c),
                    _ => token.pieces.push((c.to_string(), span.style)),
                }
                token.width += c.width().unwrap_or(0);
            }
        }
    }
    tokens
}

struct Rows {
    rows: Vec<Vec<Span<'static>>>,
    used: usize,
    width: usize,
}

impl Rows {
    fn break_row(&mut self) {
        self.rows.push(Vec::new());
        self.used = 0;
    }

    /// Append `token`, hard-breaking it wherever it overflows the row.
    fn push(&mut self, token: Token) {
        for (text, style) in token.pieces {
            for c in text.chars() {
                let w = c.width().unwrap_or(0);
                if self.used + w > self.width && self.used > 0 {
                    self.break_row();
                }
                let Some(row) = self.rows.last_mut() else {
                    return;
                };
                match row.last_mut() {
                    Some(span) if span.style == style => span.content.to_mut().push(c),
                    _ => row.push(Span::styled(c.to_string(), style)),
                }
                self.used += w;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wrap_spans;
    use ratatui::text::Span;

    fn row_texts(rows: &[Vec<Span<'_>>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn moves_whole_words_to_the_next_row() {
        let rows = wrap_spans(vec![Span::raw("aura: forbidden binary_allowlist")], 20);
        assert_eq!(row_texts(&rows), ["aura: forbidden ", "binary_allowlist"]);
    }

    #[test]
    fn keeps_leading_indentation() {
        let rows = wrap_spans(vec![Span::raw("  ab cd")], 5);
        assert_eq!(row_texts(&rows), ["  ab ", "cd"]);
    }

    #[test]
    fn drops_whitespace_at_a_break() {
        let rows = wrap_spans(vec![Span::raw("ab  "), Span::raw("cd")], 3);
        assert_eq!(row_texts(&rows), ["ab", "cd"]);
    }

    #[test]
    fn splits_words_wider_than_a_row() {
        let rows = wrap_spans(vec![Span::raw("ab cdefgh")], 4);
        assert_eq!(row_texts(&rows), ["ab ", "cdef", "gh"]);
    }

    #[test]
    fn zero_width_does_not_wrap() {
        let rows = wrap_spans(vec![Span::raw("abc")], 0);
        assert_eq!(row_texts(&rows), ["abc"]);
    }
}
