//! Single-line lists drawn at their taller pitch.
//!
//! The terminal UI lays each such list out in the rows that fit at
//! [`Metrics::list_row`] and records it as a [`ListBand`]. Here a band's
//! drawn rows are painted [`Metrics::list_row`] pixels tall each, from
//! its top, and the blank rows it left under them follow at their own
//! height, cut at the band's bottom. The band's columns include its
//! panel's borders, so they stretch with the rows.
//!
//! What was drawn over a band after the panels (a menu, a notice) is
//! painted where the grid has it, at a cell's height, over everything
//! else, and the stretched rows come from the band's own copy of its
//! cells, so the menu is not stretched under itself. The pointer is
//! mapped back the same way ([`cell_in_bands`]).

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

use super::brush::{Brush, Row};
use super::{Painted, Painter};
use crate::geom::Size;
use crate::grid::Metrics;
use aura_ui_terminal::ListBand;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use std::ops::Range;

/// Add to `cuts` the edges of the bands crossing screen row `y` and of
/// the spans drawn over them, so each piece is inside a band or out of
/// it, and covered or not.
pub(super) fn cut_bands(cuts: &mut Vec<usize>, bands: &[ListBand], y: u16, len: usize) {
    let before = cuts.len();
    for band in bands.iter().filter(|band| band.rows.contains(&y)) {
        cuts.extend([usize::from(band.columns.start), usize::from(band.columns.end)]);
        for span in band.covered(y) {
            cuts.extend([usize::from(span.start), usize::from(span.end)]);
        }
    }
    if cuts.len() > before {
        cuts.retain(|&c| c <= len);
        cuts.sort_unstable();
        cuts.dedup();
    }
}

/// The band the piece `columns` of screen row `y` is inside, if any.
pub(super) fn band_at<'a>(
    bands: &'a [ListBand],
    y: u16,
    columns: &Range<usize>,
) -> Option<&'a ListBand> {
    bands.iter().find(|band| {
        band.rows.contains(&y)
            && usize::from(band.columns.start) <= columns.start
            && columns.end <= usize::from(band.columns.end)
    })
}

/// Row `y`'s cells with the band's columns as the panels drew them.
fn as_drawn(cells: &[Cell], band: &ListBand, y: u16) -> Vec<Cell> {
    let mut row = cells.to_vec();
    let start = usize::from(band.columns.start);
    for (at, cell) in band.row(y).iter().enumerate() {
        if let Some(slot) = row.get_mut(start + at) {
            slot.clone_from(cell);
        }
    }
    row
}

/// The pixel heights a band is painted at: a grid row and a list row.
#[allow(clippy::cast_possible_truncation)]
fn heights(metrics: Metrics) -> (i32, i32) {
    (metrics.cell.height as i32, metrics.list_row() as i32)
}

impl Painter {
    /// Paint the piece `columns` of `row`, inside `band`: stretched when
    /// the list drew in its row, else under the stretched rows, cut at
    /// the band's bottom; then, where something was drawn over the band,
    /// that, unstretched, on top.
    pub(super) fn paint_band_piece(
        &mut self,
        brush: &mut Brush<'_>,
        row: &Painted<'_>,
        band: &ListBand,
        columns: &Range<usize>,
    ) {
        let (height, tall) = heights(brush.metrics);
        let k = i32::from(row.y - band.rows.start);
        let band_top = row.top - k * height;
        let clip = band_top..band_top + i32::from(band.rows.end - band.rows.start) * height;
        let covered = band.covered(row.y).next().is_some();
        let drawn = covered.then(|| as_drawn(row.cells, band, row.y));
        let cells = drawn.as_deref().unwrap_or(row.cells);
        let piece = &cells[columns.clone()];
        let shown = i32::from(band.shown);
        if k < shown {
            let mut stretched = Brush {
                text: &mut *brush.text,
                metrics: Metrics {
                    cell: Size::new(brush.metrics.cell.width, tall as f32),
                    ..brush.metrics
                },
                palette: brush.palette,
                origin: brush.origin,
            };
            let quads = self.tall_rows.row(columns.start, piece, None, |out| {
                let looks = stretched.looks(cells);
                stretched.piece(out, &Row { cells, looks: &looks }, columns.clone(), None);
            });
            self.frame.extend_clipped(quads, band_top + k * tall, clip);
        } else {
            let at = band_top + shown * tall + (k - shown) * height;
            let quads = self.rows.row(columns.start, piece, None, |out| {
                let looks = brush.looks(cells);
                brush.piece(out, &Row { cells, looks: &looks }, columns.clone(), None);
            });
            self.frame.extend_clipped(quads, at, clip);
        }
        let over = u16::try_from(columns.start).is_ok_and(|x| band.is_covered(x, row.y));
        if over {
            let cells = row.cells;
            let quads = self
                .rows
                .row(columns.start, &cells[columns.clone()], None, |out| {
                    let looks = brush.looks(cells);
                    brush.piece(out, &Row { cells, looks: &looks }, columns.clone(), None);
                });
            self.top.extend_lowered(quads, row.top);
        }
    }
}

/// The grid cell drawn at `cell` once `bands` are stretched: `cell` is
/// where the point is on the plain grid, `offset` how far below the top
/// of the grid's first row the point is, in pixels. Outside a band, or
/// on what was drawn over one, it is `cell` itself.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cell_in_bands(bands: &[ListBand], metrics: Metrics, cell: Position, offset: f32) -> Position {
    let Some(band) = bands.iter().find(|band| {
        band.rows.contains(&cell.y) && band.columns.contains(&cell.x) && !band.is_covered(cell.x, cell.y)
    }) else {
        return cell;
    };
    let (height, tall) = (metrics.cell.height, metrics.list_row());
    let into = (offset - f32::from(band.rows.start) * height).max(0.0);
    let stretched = f32::from(band.shown) * tall;
    let row = if into < stretched {
        (into / tall).floor() as u16
    } else {
        band.shown + ((into - stretched) / height).floor() as u16
    };
    let last = band.rows.end.saturating_sub(1);
    Position::new(cell.x, (band.rows.start + row).min(last))
}

#[cfg(test)]
mod tests;
