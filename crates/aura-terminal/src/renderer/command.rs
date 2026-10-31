//! Shell command rendering: highlighted command lines and terminal output.

use super::segments::{message_style, nick_label};
use crate::app::shell_input::{tokens, TokenKind};
use crate::app::MESSAGE_BODY_INDENT;
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

/// Chat lines for a shell command: `[HH:MM:SS] <YOU>  ls -la`, then the
/// output below the command as a terminal would show it.
pub(super) fn shell_lines(
    run: &ShellRun,
    timestamp: &str,
    content_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = content_width.saturating_sub(MESSAGE_BODY_INDENT);
    let indent = || Span::raw(" ".repeat(MESSAGE_BODY_INDENT));
    let mut lines = Vec::new();

    let (nick, nick_color, _) = message_style(MessageRole::User, theme);
    let echo = owned(command_spans(&run.command, theme));
    for (i, row) in wrap_spans(echo, width).into_iter().enumerate() {
        let lead = if i == 0 {
            vec![
                Span::styled(
                    format!("[{timestamp}] "),
                    Style::default().fg(theme.colors.muted),
                ),
                Span::styled(nick_label(nick), Style::default().fg(nick_color)),
                Span::raw(" "),
            ]
        } else {
            vec![indent()]
        };
        lines.push(Line::from([lead, row].concat()));
    }

    let output = run.output.as_deref().unwrap_or_default();
    let output = output.trim_end_matches('\n');
    if !output.is_empty() {
        let base = Style::default().fg(theme.colors.foreground);
        for line in output.split('\n') {
            for row in wrap_spans(crate::ansi::line_spans(line, base), width) {
                lines.push(Line::from([vec![indent()], row].concat()));
            }
        }
    }

    if let Some(code) = run.exit_code.filter(|code| *code != 0) {
        lines.push(Line::from(vec![
            indent(),
            Span::styled(
                format!("exit {code}"),
                Style::default()
                    .fg(theme.colors.error)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }
    lines
}

fn owned(spans: Vec<Span<'_>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

/// Break spans into rows at most `width` columns wide, splitting anywhere
/// and keeping all whitespace, like a terminal does.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![spans];
    }
    let mut rows = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        let mut chunk = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width && used > 0 {
                push_chunk(&mut rows, &mut chunk, span.style);
                rows.push(Vec::new());
                used = 0;
            }
            chunk.push(c);
            used += w;
        }
        push_chunk(&mut rows, &mut chunk, span.style);
    }
    rows
}

fn push_chunk(rows: &mut [Vec<Span<'static>>], chunk: &mut String, style: Style) {
    if let (false, Some(row)) = (chunk.is_empty(), rows.last_mut()) {
        row.push(Span::styled(std::mem::take(chunk), style));
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
    fn wraps_at_width_keeping_spaces() {
        let rows = wrap_spans(vec![Span::raw("ab  "), Span::raw("cdef")], 3);
        assert_eq!(row_texts(&rows), ["ab ", " cd", "ef"]);
    }

    #[test]
    fn zero_width_does_not_wrap() {
        let rows = wrap_spans(vec![Span::raw("abc")], 0);
        assert_eq!(row_texts(&rows), ["abc"]);
    }
}
