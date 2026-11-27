//! Pieces of a dialog's lines, and the frame the menus share with dialogs.

use super::super::list_row::{cell_style, CellState};
use crate::themes::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};
use unicode_width::UnicodeWidthStr;

/// What leads the selected row of a list.
const MARKER: &str = "› ";

/// What leads the other rows, as wide as [`MARKER`].
const NO_MARKER: &str = "  ";

/// The caret in a field being typed into.
const CARET: &str = "█";

/// An answer a dialog takes by key, `[Y] Allow once`, in `color` with the
/// key bold.
#[must_use]
pub fn choice_line(key: char, label: &str, color: Color) -> Line<'static> {
    let style = Style::default().fg(color);
    Line::from(vec![
        Span::styled(
            format!("[{}]", key.to_ascii_uppercase()),
            style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), style),
    ])
}

/// A row of a list, `width` columns: `spans` after `› ` on the
/// selection's fill when `selected`, which takes over their colors, or
/// after two blanks.
#[must_use]
pub fn list_item<'a>(
    spans: Vec<Span<'a>>,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    if !selected {
        let row: Vec<Span<'a>> = std::iter::once(Span::raw(NO_MARKER)).chain(spans).collect();
        return Line::from(row);
    }
    let cell = cell_style(theme, CellState::Selected, Style::default());
    let fill = cell.bg.map_or(cell.text, |bg| cell.text.bg(bg));
    let mut used = MARKER.width();
    let mut row = vec![Span::styled(MARKER, fill)];
    for span in spans {
        used += span.width();
        row.push(Span::styled(span.content, span.style.patch(fill)));
    }
    row.push(Span::styled(" ".repeat(width.saturating_sub(used)), fill));
    Line::from(row)
}

/// A line being typed into, `width` columns: `lead`, such as a label or an
/// indent, then `text` in `style` with a caret at byte `cursor`. Text left
/// of the caret is dropped from the left while the line is too wide, so
/// the caret stays in view.
#[must_use]
pub fn caret_line<'a>(
    lead: Vec<Span<'a>>,
    text: &str,
    cursor: usize,
    width: usize,
    style: Style,
    theme: &Theme,
) -> Line<'a> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (mut before, after) = text.split_at(cursor);
    let lead_width: usize = lead.iter().map(Span::width).sum();
    let room = width.saturating_sub(lead_width + CARET.width());
    while before.width() > room {
        let mut chars = before.chars();
        chars.next();
        before = chars.as_str();
    }
    let mut line = lead;
    line.push(Span::styled(before.to_string(), style));
    line.push(Span::styled(
        CARET,
        Style::default().fg(theme.colors.primary),
    ));
    line.push(Span::styled(after.to_string(), style));
    Line::from(line)
}

/// The frame of a menu opened beside what it acts on, such as the `/` and
/// `@` menus: a dialog's border and title, and `hint`, if any, at the
/// right end of the bottom border.
#[must_use]
pub fn popup_block<'a>(title: Option<&str>, hint: Option<&str>, theme: &Theme) -> Block<'a> {
    let colors = &theme.colors;
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors.muted));
    if let Some(title) = title {
        block = block.title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(hint) = hint {
        let hint = Span::styled(format!(" {hint} "), Style::default().fg(colors.muted));
        block = block.title_bottom(Line::from(hint).right_aligned());
    }
    block
}
