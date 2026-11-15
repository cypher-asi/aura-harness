//! The chat's prose as drawn in the last frame, so a front end that is not
//! bound to a cell grid can set it in a proportional font.

use super::App;
use ratatui::buffer::{Buffer, Cell};
use std::ops::Range;

/// One screen row of the user's or the agent's flowing text: not code, a
/// table, tool output or chrome, whose columns have to line up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseRow {
    /// Screen row
    pub y: u16,
    /// First column of the text
    pub x: u16,
    /// Columns the text covers from `x`
    pub width: u16,
    /// Columns of inline code, which stay monospace
    pub code: Vec<Range<u16>>,
    /// What the row's cells held when the chat was drawn
    symbols: String,
}

impl ProseRow {
    pub(crate) fn new(
        buffer: &Buffer,
        (x, y): (u16, u16),
        width: u16,
        code: Vec<Range<u16>>,
    ) -> Self {
        Self {
            y,
            x,
            width,
            code,
            symbols: symbols(buffer, x, y, width),
        }
    }

    /// Whether nothing drawn after the chat changed the row's text.
    fn uncovered(&self, buffer: &Buffer) -> bool {
        symbols(buffer, self.x, self.y, self.width) == self.symbols
    }
}

fn symbols(buffer: &Buffer, x: u16, y: u16, width: u16) -> String {
    (x..x.saturating_add(width))
        .filter_map(|x| buffer.cell((x, y)))
        .map(Cell::symbol)
        .collect()
}

impl App {
    /// The chat's prose rows in the last frame, top to bottom.
    #[must_use]
    pub fn prose_rows(&self) -> &[ProseRow] {
        &self.prose_rows
    }

    pub(crate) fn set_prose_rows(&mut self, rows: Vec<ProseRow>) {
        self.prose_rows = rows;
    }

    /// Forget the prose rows an overlay or menu was drawn over.
    pub(crate) fn drop_covered_prose(&mut self, buffer: &Buffer) {
        self.prose_rows.retain(|row| row.uncovered(buffer));
    }
}
