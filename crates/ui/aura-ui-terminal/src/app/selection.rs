//! Mouse text selection over the rendered screen.

use std::collections::BTreeMap;

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::UnicodeWidthStr;

use super::{App, PanelFocus, SELECTION_SCROLL_MOST};

/// A drag selection inside one panel. Like a terminal selection it runs
/// from the anchor to the head in reading order, so rows between the two
/// are selected edge to edge.
///
/// Its ends are held as rows of the panel's content, not of the screen,
/// so the selection stays on its text while the panel scrolls; rows that
/// scroll out of view are kept to copy them later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    bounds: Rect,
    /// Content row shown on the top row of `bounds`
    top: usize,
    anchor: Point,
    head: Point,
    /// Each content row's cell symbols as last drawn, by row
    kept: BTreeMap<usize, Vec<String>>,
}

/// A cell of a panel's content: its row and screen column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Point {
    row: usize,
    column: u16,
}

impl Selection {
    /// Start a selection at screen cell `at`, limited to `bounds`, whose
    /// top row shows content row `top`.
    #[must_use]
    pub fn new(bounds: Rect, top: usize, at: Position) -> Self {
        let mut selection = Self {
            bounds,
            top,
            anchor: Point { row: 0, column: 0 },
            head: Point { row: 0, column: 0 },
            kept: BTreeMap::new(),
        };
        selection.anchor = selection.point(at);
        selection.head = selection.anchor;
        selection
    }

    /// The screen cells the selection is limited to.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Move the free end of the selection to screen cell `at` (clamped to
    /// the bounds).
    pub fn extend_to(&mut self, at: Position) {
        self.head = self.point(at);
    }

    /// The panel now shows content row `top` on its top row.
    pub const fn scroll_to(&mut self, top: usize) {
        self.top = top;
    }

    /// Keep the rows `buffer` shows inside the bounds, so they can still
    /// be copied once scrolled out of view.
    pub fn keep(&mut self, buffer: &Buffer) {
        for y in self.bounds.top()..self.bounds.bottom() {
            let symbols = (self.bounds.left()..self.bounds.right())
                .map(|x| buffer.cell((x, y)).map_or(" ", |c| c.symbol()).to_string())
                .collect();
            self.kept
                .insert(self.top + usize::from(y - self.bounds.y), symbols);
        }
    }

    /// Whether the selection covers only the cell it started on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Both ends, anchor first, to tell whether the selection moved.
    #[must_use]
    pub(crate) const fn ends(&self) -> (Point, Point) {
        (self.anchor, self.head)
    }

    fn point(&self, at: Position) -> Point {
        let at = clamp(self.bounds, at);
        Point {
            row: self.top + usize::from(at.y - self.bounds.y),
            column: at.x,
        }
    }

    /// Screen row of content row `row`, when it is in view.
    fn screen_row(&self, row: usize) -> Option<u16> {
        let offset = u16::try_from(row.checked_sub(self.top)?).ok()?;
        (offset < self.bounds.height).then(|| self.bounds.y + offset)
    }

    fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// `(row, start, end)` for each selected content row, end column
    /// exclusive.
    fn rows(&self) -> impl Iterator<Item = (usize, u16, u16)> + '_ {
        let (start, end) = self.ordered();
        (start.row..=end.row).map(move |row| {
            let left = if row == start.row {
                start.column
            } else {
                self.bounds.x
            };
            let right = if row == end.row {
                end.column + 1
            } else {
                self.bounds.right()
            };
            (row, left, right)
        })
    }

    /// Draw the part of the selection in view in reverse video.
    pub fn highlight(&self, buffer: &mut Buffer) {
        if self.is_empty() {
            return;
        }
        for (row, left, right) in self.rows() {
            let Some(y) = self.screen_row(row) else {
                continue;
            };
            for column in left..right {
                if let Some(cell) = buffer.cell_mut((column, y)) {
                    cell.modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }

    /// The selected text, one line per row with trailing blanks removed:
    /// rows in view from `buffer`, the others as they were last kept.
    /// Code block frames are left out and their wrapped lines joined, so
    /// a command copied from one runs as it is.
    #[must_use]
    pub fn text(&self, buffer: &Buffer) -> String {
        let lines: Vec<String> = self
            .rows()
            .map(|(row, left, right)| {
                let screen_row = self.screen_row(row);
                let symbol = |column: u16| match screen_row {
                    Some(y) => buffer.cell((column, y)).map(|c| c.symbol()),
                    None => self
                        .kept
                        .get(&row)?
                        .get(usize::from(column.checked_sub(self.bounds.x)?))
                        .map(String::as_str),
                };
                let mut line = String::new();
                let mut column = left;
                while column < right {
                    let Some(symbol) = symbol(column) else {
                        break;
                    };
                    line.push_str(symbol);
                    // A wide glyph's trailing cells hold filler blanks;
                    // a cell of chat prose may hold several characters.
                    let width = if symbol.chars().count() == 1 {
                        symbol.width().max(1)
                    } else {
                        1
                    };
                    column += u16::try_from(width).unwrap_or(1);
                }
                line.trim_end().to_string()
            })
            .collect();
        crate::components::unframe_code(&lines).join("\n")
    }
}

fn clamp(bounds: Rect, at: Position) -> Position {
    let max_x = bounds.right().saturating_sub(1).max(bounds.x);
    let max_y = bounds.bottom().saturating_sub(1).max(bounds.y);
    Position::new(at.x.clamp(bounds.x, max_x), at.y.clamp(bounds.y, max_y))
}

impl App {
    /// Cells of `panel` that hold text: inside its border, and for the chat
    /// only the rows its body scrolls through, or its message area. The
    /// agents panel on the left is navigation, so its text is never
    /// selected.
    pub(super) fn selectable_area(&self, panel: PanelFocus) -> Option<Rect> {
        let area = match panel {
            PanelFocus::Chat if self.areas.chat_rows.is_some() => return self.areas.chat_rows,
            PanelFocus::Chat if self.detail.view.is_some() => self.areas.chat,
            PanelFocus::Chat => return self.areas.chat_text.or(self.areas.chat),
            PanelFocus::Swarm => return None,
            PanelFocus::Records => self.areas.records,
            PanelFocus::Sandbox => self.areas.sandbox,
        }?;
        Some(Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        })
    }

    /// Text selected with the mouse, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<&Selection> {
        self.select.selection.as_ref()
    }

    /// Drop the mouse selection.
    pub fn clear_selection(&mut self) {
        self.select.clear();
    }

    /// Whether the selection is in the rows the chat panel's body scrolls
    /// through, as drawn in the last frame.
    fn selection_in_chat_rows(&self) -> bool {
        self.select.selection_scrolls
            && self
                .select
                .selection
                .as_ref()
                .is_some_and(|s| self.areas.chat_rows == Some(s.bounds()))
    }

    /// Whether the wheel over `panel` scrolls a selection along with its
    /// text rather than dropping it.
    pub(super) fn wheel_keeps_selection(&self, panel: PanelFocus) -> bool {
        panel == PanelFocus::Chat && self.selection_in_chat_rows()
    }

    /// Fit the selection to the frame just drawn into `buffer`: follow the
    /// chat body's scroll, move the held end to the pointer, and keep the
    /// rows in view for copying once they scroll away.
    pub(crate) fn track_selection(&mut self, buffer: &Buffer) {
        if !self.selection_in_chat_rows() {
            return;
        }
        let Some(selection) = self.select.selection.as_mut() else {
            return;
        };
        selection.scroll_to(self.areas.chat_rows_top);
        if let Some(at) = self.select.selecting {
            selection.extend_to(at);
        }
        selection.keep(buffer);
    }

    /// How the chat should scroll for a selection being dragged past its
    /// top (`true`) or bottom, and by how many rows.
    fn selection_scroll(&self) -> Option<(bool, usize)> {
        let at = self.select.selecting?;
        let bounds = self.select.selection.as_ref()?.bounds();
        if !self.selection_in_chat_rows() {
            return None;
        }
        let (up, past) = if at.y < bounds.top() {
            (true, bounds.top() - at.y)
        } else if at.y >= bounds.bottom() {
            (false, at.y + 1 - bounds.bottom())
        } else {
            return None;
        };
        Some((up, usize::from(past.min(SELECTION_SCROLL_MOST))))
    }

    /// Whether a selection is held past the chat's top or bottom, so the
    /// chat keeps scrolling under it while time passes.
    #[must_use]
    pub fn selection_scrolling(&self) -> bool {
        self.selection_scroll().is_some()
    }

    /// Scroll the chat one step under a selection dragged past its top or
    /// bottom. The selection's held end follows on the next frame.
    pub(crate) fn scroll_selection(&mut self) {
        let Some((up, rows)) = self.selection_scroll() else {
            return;
        };
        match (self.detail.view.is_some(), up) {
            (true, true) => self.scroll_detail_up(rows),
            (true, false) => self.scroll_detail_down(rows),
            (false, true) => self.scroll_panel_up(PanelFocus::Chat, rows),
            (false, false) => self.scroll_panel_down(PanelFocus::Chat, rows),
        }
    }
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
        let mut sel = Selection::new(buf.area, 0, Position::new(6, 0));
        sel.extend_to(Position::new(5, 2));
        assert_eq!(sel.text(&buf), "world\nsecond line\nthird");

        let mut backwards = Selection::new(buf.area, 0, Position::new(5, 2));
        backwards.extend_to(Position::new(6, 0));
        assert_eq!(backwards.text(&buf), sel.text(&buf));
    }

    #[test]
    fn clamps_to_bounds_and_skips_wide_glyph_filler() {
        let buf = buffer(&["| 日本 |", "| ab   |"]);
        let bounds = Rect::new(2, 0, 4, 2);
        let mut sel = Selection::new(bounds, 0, Position::new(0, 0));
        sel.extend_to(Position::new(40, 1));
        assert_eq!(sel.text(&buf), "日本\nab");
    }

    #[test]
    fn highlight_reverses_only_selected_cells() {
        let mut buf = buffer(&["abcd"]);
        let mut sel = Selection::new(buf.area, 0, Position::new(1, 0));
        sel.extend_to(Position::new(2, 0));
        sel.highlight(&mut buf);
        let reversed: Vec<bool> = (0..4)
            .map(|x| buf[(x, 0)].modifier.contains(Modifier::REVERSED))
            .collect();
        assert_eq!(reversed, [false, true, true, false]);
    }

    #[test]
    fn keeps_rows_scrolled_out_of_view() {
        // Content rows 0..4 seen two at a time.
        let content = ["zero", "one", "two", "three"];
        let view = |top: usize| buffer(&content[top..top + 2]);
        let mut sel = Selection::new(Rect::new(0, 0, 5, 2), 0, Position::new(1, 0));
        sel.keep(&view(0));

        sel.scroll_to(1);
        sel.keep(&view(1));
        sel.scroll_to(2);
        let shown = view(2);
        sel.keep(&shown);
        sel.extend_to(Position::new(2, 1));
        assert_eq!(sel.text(&shown), "ero\none\ntwo\nthr");

        let mut highlighted = shown.clone();
        sel.highlight(&mut highlighted);
        let reversed = |y: u16| {
            (0..5)
                .filter(|&x| highlighted[(x, y)].modifier.contains(Modifier::REVERSED))
                .count()
        };
        assert_eq!((reversed(0), reversed(1)), (5, 3));

        // Scrolled back up, the anchor is in view again where it started.
        sel.scroll_to(0);
        assert_eq!(sel.text(&view(0)), "ero\none\ntwo\nthr");
    }
}
