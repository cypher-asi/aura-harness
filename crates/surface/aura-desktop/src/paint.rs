//! The window as quads: the tab bar, the grid's rows, the panels' title
//! bars and the cursor, laid out into one [`Scene`] a frame.
//!
//! Rows are painted the way [`crate::grid`] paints them on an iced
//! canvas, to the pixel: one rectangle per run of background color, one
//! shaped run per stretch of same-styled ASCII, box, block, braille and
//! status characters as shapes, and the chat's prose flowing in the
//! proportional face. What differs is what a frame costs. A row's quads
//! are kept by what the row holds ([`RowCache`]), so a frame shapes only
//! the rows whose content is new, and a row that scrolled is the same
//! quads lower down.
//!
//! Every background, line and shape of the frame comes before any of its
//! text, as iced orders a canvas: text is never covered by the background
//! of the row below it.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

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
use ratatui::layout::Position;
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
    /// The cell the cursor is on, while it shows
    pub cursor: Option<Position>,
    pub metrics: Metrics,
    pub palette: Palette,
    /// The strip above the grid
    pub bar: &'a Bar,
    /// The panels' title bars, over the cells
    pub chrome: &'a Chrome,
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

    /// How many rows the last frame had to paint anew.
    pub const fn built(&self) -> usize {
        self.built
    }

    #[allow(clippy::cast_possible_truncation)]
    fn lay_out(&mut self, view: &View<'_>, text: &mut TextSystem) {
        let (metrics, palette) = (view.metrics, view.palette);
        let bar = tab_bar::height(&metrics);
        let area = Size::new(view.size.width, (view.size.height - bar).max(0.0));
        let origin = metrics.origin(area, view.cols, view.rows);
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
        let mut row_top = top;
        for (index, cells) in lines.enumerate() {
            let prose = prose_at(view.prose, index);
            let row = self
                .rows
                .row(cells, prose, |out| brush.row(out, cells, prose));
            self.frame.extend_lowered(row, row_top);
            row_top += height;
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
            fg = mix(fg, bg, 0.45);
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

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// The prose on screen row `y`, if it holds any.
fn prose_at(prose: &[ProseRow], y: usize) -> Option<&ProseRow> {
    prose.iter().find(|p| usize::from(p.y) == y)
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
    fn row(&mut self, out: &mut Layers, cells: &[Cell], prose: Option<&ProseRow>) {
        let looks: Vec<Look> = cells.iter().map(|c| Look::of(c, self.palette)).collect();
        let row = Row {
            cells,
            looks: &looks,
        };
        let columns = prose.map_or(0..0, |p| {
            let start = usize::from(p.x + p.indent).min(cells.len());
            start..usize::from(p.x + p.width).clamp(start, cells.len())
        });
        for grid in [0..columns.start, columns.end..cells.len()] {
            self.backgrounds(out, &row, grid.clone());
            self.cells(out, &row, grid);
        }
        if let Some(prose) = prose {
            self.prose(out, &row, columns, &prose.code);
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
                // Exactly the grid, under the bar: the origin is (0, 0).
                size: Size::new(168.0, 29.0 + 72.0),
                cells: &self.buffer.content,
                cols: COLS,
                rows: ROWS,
                prose: &[],
                cursor: self.cursor,
                metrics: Metrics::new(14.0),
                palette: self.palette,
                bar: &self.bar,
                chrome: &self.chrome,
            }
        }
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
        assert_eq!((scene.width, scene.height), (168, 101));
        assert_eq!(scene.background, rgba(stage.palette.bg));

        let [back, line, below, h, i] = scene.quads[..] else {
            panic!("a background, a line and two glyphs: {:?}", scene.quads);
        };
        // Columns 2 and 3 of row 1: 16.8 to 33.6 across, 18 down from
        // the bar's 29.
        assert_eq!((back.x, back.y, back.w, back.h), (17, 47, 17, 18));
        assert_eq!((back.fill, back.color), (Fill::Solid, [200, 0, 0, 255]));
        // The line's two arms, up and down from the middle of column 6.
        assert_eq!((line.fill, line.x, line.w), (Fill::Solid, 54, 1));
        assert_eq!((line.y, below.bottom()), (47, 65));
        assert_eq!((below.x, below.w), (54, 1));
        assert_eq!((h.fill, i.fill), (Fill::Mask, Fill::Mask));
        assert!(h.x >= 17 && h.x < i.x && i.right() <= 34);
        assert!(h.y >= 47 && h.bottom() <= 65, "{h:?}");
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
        assert_eq!((block.x, block.y, block.w, block.h), (8, 65, 9, 18));
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
