//! Text as the terminal draws it: control characters made visible, and
//! lines hard-wrapped to the view.

use super::super::text::display_width;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub(super) const TAB: &str = "    ";

/// Tabs as spaces and other control characters as `�`, so the terminal
/// draws exactly what is measured.
pub(super) fn sanitize(line: &str) -> String {
    if !line.chars().any(char::is_control) {
        return line.to_string();
    }
    line.chars()
        .map(|c| match c {
            '\t' => TAB.to_string(),
            c if c.is_control() => '\u{fffd}'.to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Break `line` into rows at most `width` columns wide, character by
/// character, starting each continuation row with `indent`.
pub(super) fn hard_wrap<'a>(line: Line<'a>, width: usize, indent: &str) -> Vec<Line<'a>> {
    if line.width() <= width {
        return vec![line];
    }
    let indent_width = display_width(indent);
    let mut rows: Vec<Line<'a>> = Vec::new();
    let mut current: Vec<Span<'a>> = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width && used > indent_width {
                if !piece.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut piece), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut current)));
                current.push(Span::raw(indent.to_string()));
                used = indent_width;
            }
            piece.push(c);
            used += w;
        }
        if !piece.is_empty() {
            current.push(Span::styled(piece, span.style));
        }
    }
    rows.push(Line::from(current));
    rows
}
