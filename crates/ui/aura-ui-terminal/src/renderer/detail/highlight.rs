//! Colours of a detail view's lines: source languages, `grep` and `log`
//! output, and vim's search matches and cursor.

use super::wrap::TAB;
use crate::app::vim::Vim;
use crate::components::highlight_code_line;
use crate::components::syntax::{highlight_line, syntax_for_path};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

/// Whether `language` is one of the view-only formats below rather than
/// a source language.
pub(super) fn is_pseudo_language(language: &str) -> bool {
    matches!(language, "grep" | "log")
}

/// Highlight one line of a detail view. Besides source languages, `grep`
/// colors `path:line:` before highlighting the match by the path's
/// extension, and `log` colors errors and warnings in command output.
pub(super) fn highlight_view_line(language: &str, line: &str, theme: &Theme) -> Vec<Span<'static>> {
    match language {
        "grep" => grep_line(line, theme),
        "log" => vec![log_line(line, theme)],
        _ => highlight_code_line(language, line, theme),
    }
}

/// `path:line:text`, as search tools print matches.
fn grep_line(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    let split = line.find(':').and_then(|colon| {
        let rest = &line[colon + 1..];
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        (digits > 0 && rest[digits..].starts_with(':')).then_some((colon, colon + 1 + digits))
    });
    let Some((colon, end)) = split else {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(theme.colors.secondary),
        )];
    };
    let path = &line[..colon];
    let syntax = syntax_for_path(std::path::Path::new(path), "");
    let muted = Style::default().fg(theme.colors.muted);
    let mut spans = vec![
        Span::styled(
            path.to_string(),
            Style::default().fg(theme.colors.secondary),
        ),
        Span::styled(":".to_string(), muted),
        Span::styled(line[colon + 1..end].to_string(), muted),
        Span::styled(":".to_string(), muted),
    ];
    spans.extend(highlight_line(syntax, &line[end + 1..], theme));
    spans
}

/// Command output: errors and warnings stand out, `-->` locations and
/// the `stderr:` divider are subdued.
fn log_line(line: &str, theme: &Theme) -> Span<'static> {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    let color =
        if lower.starts_with("error") || lower.contains("panicked") || lower.contains("failed") {
            theme.colors.error
        } else if lower.starts_with("warning") {
            theme.colors.warning
        } else if trimmed.starts_with("-->")
            || trimmed.starts_with("stderr:")
            || trimmed.starts_with('|')
        {
            theme.colors.secondary
        } else if lower.starts_with("test ") && lower.ends_with(" ok") || lower.contains("finished")
        {
            theme.colors.success
        } else {
            theme.colors.foreground
        };
    Span::styled(line.to_string(), Style::default().fg(color))
}

/// Restyle the highlighted `spans` of `raw` to show search matches and,
/// on the cursor line, the cursor cell.
pub(super) fn mark<'a>(
    mut spans: Vec<Span<'a>>,
    raw: &str,
    vim: &Vim,
    on_cursor: bool,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let found = Style::default()
        .bg(theme.colors.selection)
        .fg(theme.colors.on_selection);
    let mut ranges: Vec<(usize, usize, Style)> = vim
        .matches(raw)
        .into_iter()
        .map(|(start, end)| (shown_index(raw, start), shown_index(raw, end), found))
        .collect();
    if on_cursor {
        let cursor = Style::default().add_modifier(Modifier::REVERSED);
        let start = shown_index(raw, vim.col());
        let end = shown_index(raw, vim.col() + 1);
        if end == start {
            spans.push(Span::raw(" "));
        }
        ranges.push((start, end.max(start + 1), cursor));
    }
    if ranges.is_empty() {
        return spans;
    }
    let mut out = Vec::new();
    let mut index = 0;
    for span in spans {
        let mut piece = String::new();
        let mut piece_style = span.style;
        for c in span.content.chars() {
            let style = ranges
                .iter()
                .rev()
                .find(|(start, end, _)| (*start..*end).contains(&index))
                .map_or(span.style, |(_, _, extra)| span.style.patch(*extra));
            if style != piece_style && !piece.is_empty() {
                out.push(Span::styled(std::mem::take(&mut piece), piece_style));
            }
            piece_style = style;
            piece.push(c);
            index += 1;
        }
        if !piece.is_empty() {
            out.push(Span::styled(piece, piece_style));
        }
    }
    out
}

/// Index in the sanitized line of character `col` of `raw`.
fn shown_index(raw: &str, col: usize) -> usize {
    raw.chars()
        .take(col)
        .map(|c| if c == '\t' { TAB.len() } else { 1 })
        .sum()
}
