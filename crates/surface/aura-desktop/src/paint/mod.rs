//! The window as quads: the tab bar, the grid's rows, the panels' title
//! bars and the cursor, laid out into one [`Scene`] a frame.
//!
//! A row is one rectangle per run of background color, one shaped run
//! per stretch of same-styled ASCII, box, block, braille and status
//! characters as shapes, and the chat's prose flowing in the
//! proportional face. A row is painted in pieces, cut at the panels'
//! borders, and a piece's quads are kept by what it holds ([`RowCache`]),
//! so a frame shapes only the pieces whose content is new, and a piece
//! that scrolled is the same quads lower down.
//!
//! Every background, line and shape of the frame comes before any of its
//! text: text is never covered by the background of the row below it.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

mod brush;

use crate::canvas::Canvas;
use crate::chrome::Chrome;
use crate::geom::{rgba, Point, Size};
use crate::grid::{Metrics, Palette};
use crate::scene::{Layers, RowCache, Scene};
use crate::tab_bar::{self, Bar};
use crate::text::{self, TextSystem};
use aura_ui_terminal::ProseRow;
use brush::{cut, prose_at, prose_columns, Brush, Row};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect};
use std::ops::Range;

/// What one frame shows.
pub struct View<'a> {
    /// The window, in pixels
    pub size: Size,
    /// The grid's cells, row after row
    pub cells: &'a [Cell],
    pub cols: u16,
    pub rows: u16,
    /// Rows of chat prose, set in the proportional face
    pub prose: &'a [ProseRow],
    /// The panels, in cells: rows are painted in pieces cut at their
    /// borders
    pub panes: &'a [Rect],
    /// The cell the cursor is on, while it shows
    pub cursor: Option<Position>,
    pub metrics: Metrics,
    pub palette: Palette,
    /// The strip above the grid
    pub bar: &'a Bar,
    /// The panels' title bars, over the cells
    pub chrome: &'a Chrome,
    /// The chat's body drawn higher than its cells, while it scrolls by
    /// pixels
    pub slide: Option<Slide<'a>>,
    /// The GRAPH tab's canvas, over the blank cells left for it
    pub canvas: Option<Canvas<'a>>,
}

/// The chat's body, part of a row up from where its cells are.
#[derive(Clone, Copy)]
pub struct Slide<'a> {
    /// The body, in cells
    pub body: Rect,
    /// Pixels higher, less than a row
    pub by: i32,
    /// The row below the body's last, which comes into view: its cells,
    /// as wide as the grid, and its prose
    pub next: &'a [Cell],
    pub next_prose: Option<&'a ProseRow>,
}

/// What the kept rows were painted for, besides their cells.
#[derive(Clone, Copy, PartialEq)]
struct Basis {
    metrics: Metrics,
    palette: Palette,
    /// Left edge of the grid
    origin: f32,
    atlas: u64,
}

/// Turns [`View`]s into [`Scene`]s, keeping what the next frame can use.
#[derive(Default)]
pub struct Painter {
    rows: RowCache,
    basis: Option<Basis>,
    frame: Layers,
    scratch: Layers,
    scene: Scene,
    built: usize,
    /// Where the row being painted is cut
    cuts: Vec<usize>,
}

impl Painter {
    /// The scene of `view`, with the process's fonts and atlas.
    pub fn paint(&mut self, view: &View<'_>) -> &Scene {
        text::with(|text| self.paint_with(view, text));
        &self.scene
    }

    /// The scene of `view`, with the fonts and the atlas of `text`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn paint_with(&mut self, view: &View<'_>, text: &mut TextSystem) -> &Scene {
        self.lay_out(view, text);
        if self.basis.map(|basis| basis.atlas) != Some(text.atlas().generation()) {
            // The atlas filled up and started over while the frame was
            // laid out, so its earlier quads point at other pixels now.
            // Once more, into the emptied atlas.
            self.lay_out(view, text);
        }
        self.scene.begin(
            view.size.width.max(0.0) as u32,
            view.size.height.max(0.0) as u32,
            rgba(view.palette.bg),
        );
        self.scene.quads.extend_from_slice(&self.frame.under);
        self.scene.quads.extend_from_slice(&self.frame.over);
        &self.scene
    }

    /// The scene last painted.
    pub const fn scene(&self) -> &Scene {
        &self.scene
    }

    /// How many pieces of rows the last frame had to paint anew.
    pub const fn built(&self) -> usize {
        self.built
    }

    #[allow(clippy::cast_possible_truncation)]
    fn lay_out(&mut self, view: &View<'_>, text: &mut TextSystem) {
        let (metrics, palette) = (view.metrics, view.palette);
        let bar = tab_bar::height(&metrics);
        let area = Size::new(view.size.width, (view.size.height - bar).max(0.0));
        let origin = metrics.origin(area, view.cols);
        self.rebase(metrics, palette, origin.x, text);

        self.frame.clear();
        view.bar.quads(text, &mut self.frame);

        // Rows start on whole pixels: the origin and the bar are whole,
        // and so is a cell's height.
        let top = (bar + origin.y) as i32;
        let height = metrics.cell.height as i32;
        let mut brush = Brush {
            text,
            metrics,
            palette,
            origin: origin.x,
        };
        self.paint_rows(&mut brush, view, top, height);

        if let Some(canvas) = &view.canvas {
            let grid = Point::new(origin.x, bar + origin.y);
            canvas.quads(brush.text, metrics, grid, &mut self.frame);
        }
        view.chrome.quads(brush.text, &mut self.frame, bar);

        let on_grid = |at: &Position| at.x < view.cols && at.y < view.rows;
        if let Some(at) = view.cursor.filter(on_grid) {
            let index = usize::from(at.y) * usize::from(view.cols) + usize::from(at.x);
            self.scratch.clear();
            brush.cursor(&mut self.scratch, at.x, view.cells.get(index));
            self.frame
                .extend_lowered(&self.scratch, top + i32::from(at.y) * height);
        }
        self.built = self.rows.finish_frame();
    }

    /// Drop the kept rows unless they were painted for the same metrics,
    /// palette, origin and atlas; clear the atlas when zoomed.
    fn rebase(&mut self, metrics: Metrics, palette: Palette, origin: f32, text: &mut TextSystem) {
        if self.basis.is_some_and(|basis| basis.metrics != metrics) {
            // Zoomed: nothing in the atlas is the size of anything drawn
            // from here on.
            text.atlas_mut().clear();
        }
        let basis = Basis {
            metrics,
            palette,
            origin,
            atlas: text.atlas().generation(),
        };
        if self.basis != Some(basis) {
            self.rows.clear();
            self.basis = Some(basis);
        }
    }

    /// The grid's rows from `top`, each `height` pixels, and the row
    /// sliding in under a sliding body.
    fn paint_rows(&mut self, brush: &mut Brush<'_>, view: &View<'_>, top: i32, height: i32) {
        let width = usize::from(view.cols).max(1);
        let lines = view.cells.chunks_exact(width).take(usize::from(view.rows));
        let slide = view.slide.filter(|slide| slide.by > 0);
        let sliding = slide.map(|slide| {
            let body = slide.body;
            let clip = top + i32::from(body.y) * height..top + i32::from(body.bottom()) * height;
            (body, slide.by, clip)
        });
        let mut row_top = top;
        for (index, cells) in lines.enumerate() {
            let y = u16::try_from(index).unwrap_or(u16::MAX);
            let body = sliding
                .clone()
                .filter(|(body, ..)| body.y <= y && y < body.bottom());
            let row = Painted {
                cells,
                prose: prose_at(view.prose, index),
                top: row_top,
                y,
            };
            self.paint_row(brush, &row, view.panes, (body, false));
            row_top += height;
        }
        if let (Some(slide), Some(sliding)) = (slide, sliding) {
            if slide.next.len() == width {
                let row = Painted {
                    cells: slide.next,
                    prose: slide.next_prose,
                    top: top + i32::from(slide.body.bottom()) * height,
                    y: 0,
                };
                self.paint_row(brush, &row, &[], (Some(sliding), true));
            }
        }
    }

    /// Paint `row` piece by piece, cut at the borders of the `panes` that
    /// cross it. The pieces inside a sliding `body` go up by its slide,
    /// cut to its pixel rows; with `only_body`, the others are left out.
    fn paint_row(
        &mut self,
        brush: &mut Brush<'_>,
        row: &Painted<'_>,
        panes: &[Rect],
        (body, only_body): (Option<Sliding>, bool),
    ) {
        let cells = row.cells;
        let flow = row
            .prose
            .map(|p| prose_columns(p, cells.len()))
            .filter(|columns| !columns.is_empty());
        let edges = body.as_ref().map(|(body, ..)| *body);
        cut(
            &mut self.cuts,
            (panes, edges),
            row.y,
            cells.len(),
            flow.as_ref(),
        );
        let mut looks = None;
        for piece in self.cuts.windows(2) {
            let columns = piece[0]..piece[1];
            let inside = body.as_ref().filter(|(body, ..)| {
                usize::from(body.x) <= columns.start && columns.end <= usize::from(body.right())
            });
            if only_body && inside.is_none() {
                continue;
            }
            let prose = row.prose.filter(|_| {
                flow.as_ref()
                    .is_some_and(|f| columns.start <= f.start && f.end <= columns.end)
            });
            let quads = self
                .rows
                .row(columns.start, &cells[columns.clone()], prose, |out| {
                    let looks = looks.get_or_insert_with(|| brush.looks(cells));
                    brush.piece(out, &Row { cells, looks }, columns, prose);
                });
            match inside {
                Some((_, by, clip)) => self.frame.extend_clipped(quads, row.top - by, clip.clone()),
                None => self.frame.extend_lowered(quads, row.top),
            }
        }
    }
}

/// A sliding body: its cells, how many pixels up it is drawn, and the
/// pixel rows it is cut to.
type Sliding = (Rect, i32, Range<i32>);

/// A row of cells to paint.
struct Painted<'a> {
    cells: &'a [Cell],
    prose: Option<&'a ProseRow>,
    /// Its top on the window, in pixels
    top: i32,
    /// Its screen row
    y: u16,
}

#[cfg(test)]
mod tests;
