//! Taller rows for single-line lists, for a front end that can draw rows
//! taller than its cells.
//!
//! The cell grid has whole rows, so a terminal draws every list row one
//! cell tall. A front end that paints the grid itself (`aura-desktop`)
//! sets a [`ListPitch`]: how tall a cell is and how tall a list row
//! should be, in its pixels. Each single-line list then asks for its rows
//! area through [`App::list_rows`], which cuts the area to the rows that
//! fit at the taller pitch. The list draws, scrolls and hit-tests those
//! rows as it always does; it simply has fewer of them. The area is
//! recorded as a [`ListBand`] for the front end, which paints the band's
//! rows `tall` pixels each and maps the pointer back through them.
//!
//! Invariants:
//! - With the default pitch (cell and row the same height), `list_rows`
//!   returns its area unchanged and records nothing, so a terminal draws
//!   exactly as before.
//! - A band's rows below [`ListBand::shown`] are left blank by the list
//!   (apart from its borders); the front end paints the first of them in
//!   the pixels the taller rows leave at the band's bottom.
//! - A band keeps a copy of its cells as the panels drew them. Whatever is
//!   drawn over them later (a menu, a notice, a dialog) is recorded as
//!   covered, and a front end paints covered cells where the grid has
//!   them, unstretched, on top.

use super::App;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use std::ops::Range;

/// How tall a cell and a list row are, in a front end's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPitch {
    /// A cell's height
    pub cell: u16,
    /// A list row's height, at least `cell`
    pub tall: u16,
}

impl Default for ListPitch {
    fn default() -> Self {
        Self { cell: 1, tall: 1 }
    }
}

impl ListPitch {
    /// Whether list rows are taller than cells.
    #[must_use]
    pub const fn is_tall(self) -> bool {
        self.tall > self.cell && self.cell > 0
    }

    /// Rows of `rows` cells that fit at this pitch: at least one for a
    /// non-empty area.
    #[must_use]
    pub fn fit(self, rows: u16) -> u16 {
        if !self.is_tall() || rows == 0 {
            return rows;
        }
        let fit = u32::from(rows) * u32::from(self.cell) / u32::from(self.tall);
        u16::try_from(fit).unwrap_or(rows).clamp(1, rows)
    }
}

/// A list drawn at the taller pitch this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListBand {
    /// The grid columns the band covers: its panel's, borders included,
    /// so they stretch with the rows
    pub columns: Range<u16>,
    /// The grid rows the band covers, as the list was given them
    pub rows: Range<u16>,
    /// How many of those rows the list drew in, from the top
    pub shown: u16,
    /// The band's cells as the panels drew them, row after row
    cells: Vec<Cell>,
    /// Spans drawn over the band later, by grid row
    covered: Vec<(u16, Range<u16>)>,
}

impl ListBand {
    /// A band over grid `columns` and `rows`, the list drawn in the first
    /// `shown` of them, with no copy of its cells yet ([`Self::capture`]).
    #[must_use]
    pub const fn new(columns: Range<u16>, rows: Range<u16>, shown: u16) -> Self {
        Self {
            columns,
            rows,
            shown,
            cells: Vec::new(),
            covered: Vec::new(),
        }
    }

    /// The band's row `y` (a grid row) as the panels drew it, across
    /// [`Self::columns`]; empty before the copy is taken.
    #[must_use]
    pub fn row(&self, y: u16) -> &[Cell] {
        let width = usize::from(self.columns.end - self.columns.start);
        let Some(index) = y.checked_sub(self.rows.start).map(usize::from) else {
            return &[];
        };
        self.cells
            .get(index * width..(index + 1) * width)
            .unwrap_or_default()
    }

    /// The columns of grid row `y` drawn over the band after the panels.
    pub fn covered(&self, y: u16) -> impl Iterator<Item = Range<u16>> + '_ {
        self.covered
            .iter()
            .filter(move |(row, _)| *row == y)
            .map(|(_, columns)| columns.clone())
    }

    /// Whether grid cell (`x`, `y`) was drawn over after the panels.
    #[must_use]
    pub fn is_covered(&self, x: u16, y: u16) -> bool {
        self.covered(y).any(|columns| columns.contains(&x))
    }

    /// Copy the band's cells from `buffer`, as the panels drew them.
    pub fn capture(&mut self, buffer: &Buffer) {
        self.cells.clear();
        for y in self.rows.clone() {
            for x in self.columns.clone() {
                self.cells
                    .push(buffer.cell((x, y)).cloned().unwrap_or_default());
            }
        }
    }

    /// Record the runs of the band's cells that `buffer` no longer holds
    /// as they were copied.
    pub fn cover(&mut self, buffer: &Buffer) {
        self.covered.clear();
        let width = usize::from(self.columns.end - self.columns.start);
        for (index, y) in self.rows.clone().enumerate() {
            let mut run: Option<u16> = None;
            for (at, x) in self.columns.clone().enumerate() {
                let copied = self.cells.get(index * width + at);
                let changed = buffer.cell((x, y)) != copied;
                match (changed, run) {
                    (true, None) => run = Some(x),
                    (false, Some(start)) => {
                        self.covered.push((y, start..x));
                        run = None;
                    }
                    _ => {}
                }
            }
            if let Some(start) = run {
                self.covered.push((y, start..self.columns.end));
            }
        }
    }
}

impl App {
    /// Draw single-line lists at `pitch` from the next frame on.
    pub fn set_list_pitch(&mut self, pitch: ListPitch) {
        if self.panels.list_bands.pitch != pitch {
            self.panels.list_bands.pitch = pitch;
            self.request_redraw();
        }
    }

    /// The lists drawn at the taller pitch in the last frame.
    #[must_use]
    pub fn list_bands(&self) -> &[ListBand] {
        &self.panels.list_bands.bands
    }

    /// The rows a single-line list in `panel` draws in, of the `rows` it
    /// was given: as many as fit at the list pitch, from the top. At a
    /// taller pitch the area is recorded as a band.
    pub(crate) fn list_rows(&mut self, panel: Rect, rows: Rect) -> Rect {
        let pitch = self.panels.list_bands.pitch;
        if !pitch.is_tall() || rows.height == 0 {
            return rows;
        }
        let shown = pitch.fit(rows.height);
        let left = panel.x.min(rows.x);
        let right = panel.right().max(rows.right());
        self.panels
            .list_bands
            .bands
            .push(ListBand::new(left..right, rows.y..rows.bottom(), shown));
        Rect {
            height: shown,
            ..rows
        }
    }

    /// Forget last frame's bands, before the panels draw.
    pub(crate) fn clear_list_bands(&mut self) {
        self.panels.list_bands.bands.clear();
    }

    /// Copy each band's cells, right after the panels drew them.
    pub(crate) fn capture_list_bands(&mut self, buffer: &Buffer) {
        for band in &mut self.panels.list_bands.bands {
            band.capture(buffer);
        }
    }

    /// Record what was drawn over each band after the panels.
    pub(crate) fn cover_list_bands(&mut self, buffer: &Buffer) {
        for band in &mut self.panels.list_bands.bands {
            band.cover(buffer);
        }
    }
}

/// The list pitch and this frame's bands.
#[derive(Debug, Default)]
pub(crate) struct ListBands {
    pitch: ListPitch,
    bands: Vec<ListBand>,
}

#[cfg(test)]
mod tests;
