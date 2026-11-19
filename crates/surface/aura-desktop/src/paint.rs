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

use crate::canvas::Canvas;
use crate::chrome::Chrome;
use crate::geom::{rgba, Color, Point, Rectangle, Size};
use crate::grid::{self, Metrics, Palette};
use crate::scene::{fill, Layers, RowCache, Scene};
use crate::shapes;
use crate::tab_bar::{self, Bar};
use crate::text::{self, Align, Face, Family, Label, Shaping, TextSystem};
use crate::theme;
use aura_ui_terminal::ProseRow;
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

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
        if self.basis.is_some_and(|basis| basis.metrics != metrics) {
            // Zoomed: nothing in the atlas is the size of anything drawn
            // from here on.
            text.atlas_mut().clear();
        }
        let basis = Basis {
            metrics,
            palette,
            origin: origin.x,
            atlas: text.atlas().generation(),
        };
        if self.basis != Some(basis) {
            self.rows.clear();
            self.basis = Some(basis);
        }

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
            self.paint_row(&mut brush, &row, view.panes, (body, false));
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
                self.paint_row(&mut brush, &row, &[], (Some(sliding), true));
            }
        }

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

/// How one cell is painted, after modifiers are applied.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    underline: Option<Color>,
    strike: bool,
    hidden: bool,
}

impl Look {
    fn of(cell: &Cell, palette: Palette) -> Self {
        let m = cell.modifier;
        let mut fg = grid::color(cell.fg, palette.fg);
        let mut bg = grid::color(cell.bg, palette.bg);
        if m.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if m.contains(Modifier::DIM) {
            fg = fg.mix(bg, 0.45);
        }
        let underline = m
            .contains(Modifier::UNDERLINED)
            .then(|| grid::color(cell.underline_color, fg));
        Self {
            fg,
            bg,
            bold: m.contains(Modifier::BOLD),
            italic: m.contains(Modifier::ITALIC),
            underline,
            strike: m.contains(Modifier::CROSSED_OUT),
            hidden: m.contains(Modifier::HIDDEN),
        }
    }

    const fn face(&self, family: Family) -> Face {
        Face {
            family,
            bold: self.bold,
            italic: self.italic,
        }
    }
}

/// The prose on screen row `y`, if it holds any.
fn prose_at(prose: &[ProseRow], y: usize) -> Option<&ProseRow> {
    prose.iter().find(|p| usize::from(p.y) == y)
}

/// The columns of a row of `len` cells that `prose` flows in, after its
/// indent.
fn prose_columns(prose: &ProseRow, len: usize) -> Range<usize> {
    let start = usize::from(prose.x + prose.indent).min(len);
    start..usize::from(prose.x + prose.width).clamp(start, len)
}

/// The columns where screen row `y` of `len` cells is cut into pieces,
/// first and last included: each side of the borders of the panels it
/// crosses and the edges of a sliding `body`, except inside the prose
/// `flow`, which is one run.
fn cut(
    cuts: &mut Vec<usize>,
    (panes, body): (&[Rect], Option<Rect>),
    y: u16,
    len: usize,
    flow: Option<&Range<usize>>,
) {
    cuts.clear();
    cuts.push(0);
    for pane in panes.iter().filter(|p| p.y <= y && y < p.bottom()) {
        let (left, right) = (usize::from(pane.x), usize::from(pane.right()));
        cuts.extend([left, left + 1, right.saturating_sub(1), right]);
    }
    if let Some(body) = body {
        cuts.extend([usize::from(body.x), usize::from(body.right())]);
    }
    cuts.push(len);
    cuts.retain(|&c| c <= len && !flow.is_some_and(|f| f.start < c && c < f.end));
    cuts.sort_unstable();
    cuts.dedup();
}

/// One printable ASCII character, which every monospace font has at the
/// cell's advance.
fn is_plain(symbol: &str) -> bool {
    let bytes = symbol.as_bytes();
    bytes.len() == 1 && (0x20..0x7f).contains(&bytes[0])
}

/// One row of the screen as it is painted.
struct Row<'a> {
    cells: &'a [Cell],
    looks: &'a [Look],
}

/// Paints rows, each with its top at 0.
struct Brush<'a> {
    text: &'a mut TextSystem,
    metrics: Metrics,
    palette: Palette,
    /// Left edge of the grid, in pixels
    origin: f32,
}

impl Brush<'_> {
    fn looks(&self, cells: &[Cell]) -> Vec<Look> {
        cells.iter().map(|c| Look::of(c, self.palette)).collect()
    }

    /// The `columns` of `row`, with the `prose` that flows inside them.
    fn piece(
        &mut self,
        out: &mut Layers,
        row: &Row<'_>,
        columns: Range<usize>,
        prose: Option<&ProseRow>,
    ) {
        let flow = prose.map_or(columns.end..columns.end, |p| {
            prose_columns(p, row.cells.len())
        });
        for grid in [columns.start..flow.start, flow.end..columns.end] {
            self.backgrounds(out, row, grid.clone());
            self.cells(out, row, grid);
        }
        if let Some(prose) = prose {
            self.prose(out, row, flow, &prose.code);
        }
    }

    /// A prose row's `columns` after its indent, laid out in flow: runs
    /// of text in the proportional face, as wide as they measure; inline
    /// `code` a cell per column, in the monospace face.
    fn prose(
        &mut self,
        out: &mut Layers,
        row: &Row<'_>,
        columns: Range<usize>,
        code: &[Range<u16>],
    ) {
        let (cells, looks) = (row.cells, row.looks);
        let cell = self.metrics.cell;
        let is_code =
            |i: usize| u16::try_from(i).is_ok_and(|i| code.iter().any(|r| r.contains(&i)));
        let mut x = self.x_at(columns.start);
        let mut col = columns.start;
        while col < columns.end {
            let look = looks[col];
            if cells[col].skip {
                col += 1;
                continue;
            }
            let code = is_code(col);
            let end = (col..columns.end)
                .find(|&i| looks[i] != look || cells[i].skip || is_code(i) != code)
                .unwrap_or(columns.end);
            let run: String = cells[col..end].iter().map(Cell::symbol).collect();
            let (face, size, width) = if code {
                let size = self.metrics.font_size;
                (
                    look.face(Family::Mono),
                    size,
                    (end - col) as f32 * cell.width,
                )
            } else {
                let (face, size) = (
                    look.face(Family::Sans),
                    theme::prose_size(self.metrics.font_size),
                );
                (face, size, self.text.run_width(&run, face, size))
            };
            self.background(out, look.bg, x.round(), (x + width).round());
            if !look.hidden {
                self.run(out, &run, x, &look, (face, size, Shaping::Advanced));
            }
            self.decorate(out, &look, x, x + width);
            x += width;
            col = end;
        }
    }

    /// A block over the cell in column `column`, the character `under`
    /// it cut out of the block in the cell's background color.
    fn cursor(&mut self, out: &mut Layers, column: u16, under: Option<&Cell>) {
        let cell = self.metrics.cell;
        let x = (self.origin + f32::from(column) * cell.width).round();
        let next = (self.origin + f32::from(column + 1) * cell.width).round();
        fill(
            &mut out.under,
            x,
            0.0,
            next - x,
            cell.height,
            self.palette.fg,
        );
        let Some(under) = under else {
            return;
        };
        let look = Look::of(under, self.palette);
        let cut = Look {
            fg: look.bg,
            bg: self.palette.fg,
            underline: None,
            strike: false,
            ..look
        };
        if !under.skip {
            self.symbol(out, under.symbol(), x, &cut);
        }
    }

    /// Backgrounds of `columns`, one rectangle per run of the same color.
    fn backgrounds(&self, out: &mut Layers, row: &Row<'_>, columns: Range<usize>) {
        let edge = |col: usize| self.x_at(col).round();
        let mut start = columns.start;
        while start < columns.end {
            let bg = row.looks[start].bg;
            let end = (start..columns.end)
                .find(|&i| row.looks[i].bg != bg)
                .unwrap_or(columns.end);
            self.background(out, bg, edge(start), edge(end));
            start = end;
        }
    }

    /// Foreground of `columns`: runs of plain ASCII share one shaped run;
    /// anything else is a built-in shape or a single centered glyph.
    fn cells(&mut self, out: &mut Layers, row: &Row<'_>, columns: Range<usize>) {
        let (cells, looks) = (row.cells, row.looks);
        let mut col = columns.start;
        while col < columns.end {
            let look = looks[col];
            let symbol = cells[col].symbol();
            if cells[col].skip || look.hidden {
                col += 1;
                continue;
            }
            if is_plain(symbol) {
                let end = (col..columns.end)
                    .find(|&i| looks[i] != look || cells[i].skip || !is_plain(cells[i].symbol()))
                    .unwrap_or(columns.end);
                let run: String = cells[col..end].iter().map(Cell::symbol).collect();
                let x = self.x_at(col);
                let face = (
                    look.face(Family::Mono),
                    self.metrics.font_size,
                    Shaping::Basic,
                );
                self.run(out, &run, x, &look, face);
                self.decorate(out, &look, x, self.x_at(end));
                col = end;
                continue;
            }
            self.symbol(out, symbol, self.x_at(col), &look);
            col += 1;
        }
    }

    /// One symbol that is not plain ASCII, in the cells from `x`.
    fn symbol(&mut self, out: &mut Layers, symbol: &str, x: f32, look: &Look) {
        let cell = self.metrics.cell;
        let width = symbol.width().max(1);
        let area = Rectangle::new(
            Point::new(x, 0.0),
            Size::new(cell.width * width as f32, cell.height),
        );
        if !look.hidden {
            let mut chars = symbol.chars();
            let built_in = match (chars.next(), chars.next()) {
                (Some(c), None) if width == 1 => {
                    let atlas = self.text.atlas_mut();
                    shapes::draw(&mut out.under, atlas, c, area, rgba(look.fg))
                }
                _ => false,
            };
            if !built_in && !symbol.trim().is_empty() {
                let face = (
                    look.face(Family::Mono),
                    self.metrics.font_size,
                    Shaping::Advanced,
                );
                self.write(out, symbol, area.center_x(), look, face, Align::Center);
            }
        }
        self.decorate(out, look, area.x, area.x + area.width);
    }

    fn x_at(&self, col: usize) -> f32 {
        self.origin + col as f32 * self.metrics.cell.width
    }

    fn background(&self, out: &mut Layers, bg: Color, x0: f32, x1: f32) {
        if bg != self.palette.bg && x1 > x0 {
            fill(
                &mut out.under,
                x0,
                0.0,
                x1 - x0,
                self.metrics.cell.height,
                bg,
            );
        }
    }

    /// A run of text from `x`, unless it is only spaces.
    fn run(
        &mut self,
        out: &mut Layers,
        run: &str,
        x: f32,
        look: &Look,
        face: (Face, f32, Shaping),
    ) {
        let text = run.trim_end();
        if !text.is_empty() {
            self.write(out, text, x, look, face, Align::Left);
        }
    }

    fn write(
        &mut self,
        out: &mut Layers,
        content: &str,
        x: f32,
        look: &Look,
        (face, size, shaping): (Face, f32, Shaping),
        align: Align,
    ) {
        let label = Label {
            text: content,
            face,
            size,
            line_height: self.metrics.cell.height,
            shaping,
            align,
            middle: false,
            color: rgba(look.fg),
            on_row: true,
        };
        self.text.write(&mut out.over, &label, Point::new(x, 0.0));
    }

    fn decorate(&self, out: &mut Layers, look: &Look, x0: f32, x1: f32) {
        let cell = self.metrics.cell;
        let t = (self.metrics.font_size / 14.0).round().max(1.0);
        if let Some(color) = look.underline {
            fill(&mut out.under, x0, cell.height - 2.0 * t, x1 - x0, t, color);
        }
        if look.strike {
            let mid = (cell.height / 2.0).round();
            fill(&mut out.under, x0, mid, x1 - x0, t, look.fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster;
    use crate::scene::{Fill, Quad};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color as TermColor, Style};

    const COLS: u16 = 20;
    const ROWS: u16 = 4;

    struct Stage {
        buffer: Buffer,
        cursor: Option<Position>,
        bar: Bar,
        chrome: Chrome,
        palette: Palette,
    }

    impl Stage {
        fn new() -> Self {
            Self {
                buffer: Buffer::empty(Rect::new(0, 0, COLS, ROWS)),
                cursor: None,
                bar: Bar::default(),
                chrome: Chrome::default(),
                palette: Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber")),
            }
        }

        fn view(&self) -> View<'_> {
            View {
                // Exactly the grid, half a row under the bar: the origin is
                // (0, 9).
                size: Size::new(168.0, 29.0 + 9.0 + 72.0),
                cells: &self.buffer.content,
                cols: COLS,
                rows: ROWS,
                prose: &[],
                panes: &[],
                cursor: self.cursor,
                metrics: Metrics::new(14.0),
                palette: self.palette,
                bar: &self.bar,
                chrome: &self.chrome,
                slide: None,
                canvas: None,
            }
        }
    }

    #[test]
    fn zooming_starts_the_atlas_over() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        stage
            .buffer
            .set_string(0, 0, "zoom \u{256d}\u{2500}\u{256e}", Style::default());
        let mut painter = Painter::default();
        painter.paint_with(&stage.view(), &mut text);
        let small = text.atlas().generation();
        painter.paint_with(&stage.view(), &mut text);
        assert_eq!(text.atlas().generation(), small, "the same size again");

        let view = View {
            metrics: Metrics::new(20.0),
            ..stage.view()
        };
        let scene = painter.paint_with(&view, &mut text).clone();
        assert_ne!(
            text.atlas().generation(),
            small,
            "the old size's glyphs went"
        );
        let mut zoomed = Vec::new();
        raster::paint(&scene, text.atlas(), &mut zoomed);

        // What a painter that never saw the old size paints.
        let mut fresh = TextSystem::new();
        let mut direct = Vec::new();
        let scene = Painter::default().paint_with(&view, &mut fresh).clone();
        raster::paint(&scene, fresh.atlas(), &mut direct);
        assert!(zoomed == direct, "the picture after zooming");
    }

    #[test]
    fn a_row_is_its_backgrounds_then_its_text() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        let red = Style::default().bg(TermColor::Rgb(200, 0, 0));
        stage.buffer.set_string(2, 1, "hi", red);
        stage.buffer.set_string(6, 1, "│", Style::default());
        let mut painter = Painter::default();
        let scene = painter.paint_with(&stage.view(), &mut text);
        assert_eq!((scene.width, scene.height), (168, 110));
        assert_eq!(scene.background, rgba(stage.palette.bg));

        let [back, line, below, h, i] = scene.quads[..] else {
            panic!("a background, a line and two glyphs: {:?}", scene.quads);
        };
        // Columns 2 and 3 of row 1: 16.8 to 33.6 across, 18 down from
        // the grid's top at 38.
        assert_eq!((back.x, back.y, back.w, back.h), (17, 56, 17, 18));
        assert_eq!((back.fill, back.color), (Fill::Solid, [200, 0, 0, 255]));
        // The line's two arms, up and down from the middle of column 6.
        assert_eq!((line.fill, line.x, line.w), (Fill::Solid, 54, 1));
        assert_eq!((line.y, below.bottom()), (56, 74));
        assert_eq!((below.x, below.w), (54, 1));
        assert_eq!((h.fill, i.fill), (Fill::Mask, Fill::Mask));
        assert!(h.x >= 17 && h.x < i.x && i.right() <= 34);
        assert!(h.y >= 56 && h.bottom() <= 74, "{h:?}");
        assert_eq!(h.color, rgba(stage.palette.fg));
    }

    #[test]
    fn rows_that_scroll_or_come_back_are_not_painted_again() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        stage.buffer.set_string(0, 0, "first", Style::default());
        stage.buffer.set_string(0, 1, "second", Style::default());
        let mut painter = Painter::default();
        let before = painter.paint_with(&stage.view(), &mut text).clone();
        assert_eq!(painter.built(), 3, "two rows of text and the blank one");

        // Scrolled down a row.
        stage.buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
        stage.buffer.set_string(0, 1, "first", Style::default());
        stage.buffer.set_string(0, 2, "second", Style::default());
        let after = painter.paint_with(&stage.view(), &mut text).clone();
        assert_eq!(painter.built(), 0);
        let lowered: Vec<Quad> = before.quads.iter().map(|q| q.lowered(18)).collect();
        assert_eq!(after.quads, lowered);

        // Another font size is another picture.
        let mut view = stage.view();
        view.metrics = Metrics::new(16.0);
        painter.paint_with(&view, &mut text);
        assert_eq!(painter.built(), 3);
    }

    #[test]
    fn a_panel_that_scrolls_leaves_the_one_beside_it_painted() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        let panes = [Rect::new(0, 0, 8, ROWS), Rect::new(8, 0, 12, ROWS)];
        let write = |stage: &mut Stage, left: [&str; 2]| {
            stage.buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
            for y in 0..ROWS {
                stage.buffer.set_string(0, y, "│", Style::default());
                stage.buffer.set_string(7, y, "││", Style::default());
                stage
                    .buffer
                    .set_string(10, y, format!("right {y}"), Style::default());
            }
            for (y, line) in (0..).zip(left) {
                stage.buffer.set_string(2, y, line, Style::default());
            }
        };
        let mut painter = Painter::default();
        write(&mut stage, ["one", "two"]);
        painter.paint_with(
            &View {
                panes: &panes,
                ..stage.view()
            },
            &mut text,
        );

        write(&mut stage, ["two", "three"]);
        let scene = painter
            .paint_with(
                &View {
                    panes: &panes,
                    ..stage.view()
                },
                &mut text,
            )
            .clone();
        assert_eq!(painter.built(), 1, "only the left panel's new line");

        let mut pieces = Vec::new();
        raster::paint(&scene, text.atlas(), &mut pieces);
        let mut whole = Vec::new();
        let scene = Painter::default()
            .paint_with(&stage.view(), &mut text)
            .clone();
        raster::paint(&scene, text.atlas(), &mut whole);
        assert!(pieces == whole, "the same picture as whole rows");
    }

    #[test]
    fn rows_are_cut_at_borders_but_never_inside_prose() {
        let panes = [Rect::new(0, 0, 10, 4), Rect::new(10, 2, 10, 2)];
        let mut cuts = Vec::new();
        cut(&mut cuts, (&panes, None), 0, 20, None);
        assert_eq!(cuts, [0, 1, 9, 10, 20]);
        cut(&mut cuts, (&panes, None), 3, 20, None);
        assert_eq!(cuts, [0, 1, 9, 10, 11, 19, 20]);
        cut(&mut cuts, (&panes, None), 3, 20, Some(&(2..12)));
        assert_eq!(cuts, [0, 1, 19, 20], "prose across the border");
    }

    #[test]
    fn the_cursor_is_a_block_with_its_character_cut_out() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        stage.buffer.set_string(0, 2, "ab", Style::default());
        let mut painter = Painter::default();
        let without = painter.paint_with(&stage.view(), &mut text).quads.len();
        stage.cursor = Some(Position::new(1, 2));
        let scene = painter.paint_with(&stage.view(), &mut text);
        assert_eq!(scene.quads.len(), without + 2);

        let block = scene
            .quads
            .iter()
            .find(|q| q.fill == Fill::Solid)
            .expect("the block");
        assert_eq!((block.x, block.y, block.w, block.h), (8, 74, 9, 18));
        assert_eq!(block.color, rgba(stage.palette.fg));
        let cut = scene.quads.last().expect("the cut-out");
        assert_eq!((cut.fill, cut.color), (Fill::Mask, rgba(stage.palette.bg)));
        assert!(cut.x >= 8 && cut.right() <= 17, "{cut:?}");

        // Off the grid, there is no cursor to draw.
        stage.cursor = Some(Position::new(COLS, 0));
        assert_eq!(
            painter.paint_with(&stage.view(), &mut text).quads.len(),
            without
        );
    }

    #[test]
    fn a_frame_survives_the_atlas_starting_over() {
        let mut text = TextSystem::new();
        let mut stage = Stage::new();
        stage
            .buffer
            .set_string(0, 0, "╭─ atlas ✓", Style::default());
        let mut painter = Painter::default();
        let mut first = Vec::new();
        raster::paint(
            painter.paint_with(&stage.view(), &mut text),
            text.atlas(),
            &mut first,
        );

        text.atlas_mut().clear();
        let mut second = Vec::new();
        raster::paint(
            painter.paint_with(&stage.view(), &mut text),
            text.atlas(),
            &mut second,
        );
        assert!(painter.built() > 0, "the kept rows pointed at old pixels");
        assert_eq!(first, second);
        let bg = rgba(stage.palette.bg);
        let inked = first.chunks_exact(4).filter(|p| p[..3] != bg[..3]).count();
        assert!(inked > 40, "{inked} pixels drawn");
    }
}
