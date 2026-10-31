//! Mouse text selection over the rendered screen.

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::UnicodeWidthStr;

/// A drag selection inside one panel, in screen cells. Like a terminal
/// selection it runs from the anchor to the head in reading order, so rows
/// between the two are selected edge to edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    bounds: Rect,
    anchor: Position,
    head: Position,
}

impl Selection {
    /// Start a selection at `at`, limited to `bounds`.
    #[must_use]
    pub fn new(bounds: Rect, at: Position) -> Self {
        let at = clamp(bounds, at);
        Self {
            bounds,
            anchor: at,
            head: at,
        }
    }

    /// Move the free end of the selection to `at` (clamped to the bounds).
    pub fn extend_to(&mut self, at: Position) {
        self.head = clamp(self.bounds, at);
    }

    /// Whether the selection covers only the cell it started on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn ordered(&self) -> (Position, Position) {
        if (self.anchor.y, self.anchor.x) <= (self.head.y, self.head.x) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// `(row, start, end)` for each selected row, end column exclusive.
    fn rows(&self) -> impl Iterator<Item = (u16, u16, u16)> + '_ {
        let (start, end) = self.ordered();
        (start.y..=end.y).map(move |row| {
            let left = if row == start.y {
                start.x
            } else {
                self.bounds.x
            };
            let right = if row == end.y {
                end.x + 1
            } else {
                self.bounds.right()
            };
            (row, left, right)
        })
    }

    /// Draw the selection in reverse video.
    pub fn highlight(&self, buffer: &mut Buffer) {
        if self.is_empty() {
            return;
        }
        for (row, left, right) in self.rows() {
            for column in left..right {
                if let Some(cell) = buffer.cell_mut((column, row)) {
                    cell.modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }

    /// The selected text in `buffer`, one line per row with trailing
    /// blanks removed.
    #[must_use]
    pub fn text(&self, buffer: &Buffer) -> String {
        let lines: Vec<String> = self
            .rows()
            .map(|(row, left, right)| {
                let mut line = String::new();
                let mut column = left;
                while column < right {
                    let Some(cell) = buffer.cell((column, row)) else {
                        break;
                    };
                    let symbol = cell.symbol();
                    line.push_str(symbol);
                    // A wide glyph's trailing cells hold filler blanks.
                    column += u16::try_from(symbol.width().max(1)).unwrap_or(1);
                }
                line.trim_end().to_string()
            })
            .collect();
        lines.join("\n")
    }
}

fn clamp(bounds: Rect, at: Position) -> Position {
    let max_x = bounds.right().saturating_sub(1).max(bounds.x);
    let max_y = bounds.bottom().saturating_sub(1).max(bounds.y);
    Position::new(at.x.clamp(bounds.x, max_x), at.y.clamp(bounds.y, max_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn buffer(lines: &[&str]) -> Buffer {
        let width = lines.iter().map(|l| l.width()).max().unwrap_or(0);
        let mut buffer = Buffer::empty(Rect::new(
            0,
            0,
            u16::try_from(width).unwrap(),
            u16::try_from(lines.len()).unwrap(),
        ));
        for (y, line) in lines.iter().enumerate() {
            buffer.set_string(0, u16::try_from(y).unwrap(), line, Style::default());
        }
        buffer
    }

    #[test]
    fn selects_in_reading_order_across_rows() {
        let buf = buffer(&["hello world", "second line", "third"]);
        let mut sel = Selection::new(buf.area, Position::new(6, 0));
        sel.extend_to(Position::new(5, 2));
        assert_eq!(sel.text(&buf), "world\nsecond line\nthird");

        let mut backwards = Selection::new(buf.area, Position::new(5, 2));
        backwards.extend_to(Position::new(6, 0));
        assert_eq!(backwards.text(&buf), sel.text(&buf));
    }

    #[test]
    fn clamps_to_bounds_and_skips_wide_glyph_filler() {
        let buf = buffer(&["| 日本 |", "| ab   |"]);
        let bounds = Rect::new(2, 0, 4, 2);
        let mut sel = Selection::new(bounds, Position::new(0, 0));
        sel.extend_to(Position::new(40, 1));
        assert_eq!(sel.text(&buf), "日本\nab");
    }

    #[test]
    fn highlight_reverses_only_selected_cells() {
        let mut buf = buffer(&["abcd"]);
        let mut sel = Selection::new(buf.area, Position::new(1, 0));
        sel.extend_to(Position::new(2, 0));
        sel.highlight(&mut buf);
        let reversed: Vec<bool> = (0..4)
            .map(|x| buf[(x, 0)].modifier.contains(Modifier::REVERSED))
            .collect();
        assert_eq!(reversed, [false, true, true, false]);
    }
}
