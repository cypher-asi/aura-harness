//! The terminal screen: a ratatui backend that keeps the cell grid the
//! terminal UI draws into, and a canvas program that paints it.
//!
//! Painting works the way GPU terminals do: one rectangle per run of
//! background color, one text draw per run of same-styled ASCII, and
//! box, block, braille and status characters drawn as shapes
//! ([`crate::glyphs`]) so borders join and symbols the font lacks still
//! look right. The chat's prose rows are the exception: their text flows
//! in a proportional face (Inter), each run as wide as it measures, with
//! inline code and symbols still a cell per column. Each row's drawing is
//! cached until one of its cells changes, so a streamed word or a ticking
//! timer repaints its row rather than the screen; the block cursor is
//! drawn on top every frame so it can blink.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

use crate::chrome::Chrome;
use crate::{glyphs, theme};
use aura_ui_terminal::ProseRow;
use iced::advanced::text::Paragraph as _;
use iced::alignment::Vertical;
use iced::font::Style;
use iced::widget::canvas::{self, Cache, Frame, Geometry};
use iced::widget::text::{Alignment, LineHeight, Shaping, Wrapping};
use iced::{mouse, Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size as GridSize};
use ratatui::style::{Color as TermColor, Modifier};
use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::sync::{LazyLock, Mutex, PoisonError};
use unicode_width::UnicodeWidthStr;

/// Space kept clear around the grid, in pixels.
pub const PAD: f32 = 6.0;
/// Prose runs whose widths are kept before the measurements start over.
const MAX_MEASURED: usize = 4096;

/// A ratatui backend that records cells instead of writing escape codes.
pub struct GridBackend {
    buffer: Buffer,
    cursor: Position,
    cursor_visible: bool,
    pixels: GridSize,
}

impl GridBackend {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            buffer: Buffer::empty(Rect::new(0, 0, cols, rows)),
            cursor: Position::ORIGIN,
            cursor_visible: false,
            pixels: GridSize::default(),
        }
    }

    /// Change the grid size; the next draw repaints every cell.
    pub fn resize(&mut self, cols: u16, rows: u16, pixels: Size) {
        self.buffer.resize(Rect::new(0, 0, cols, rows));
        self.buffer.reset();
        self.pixels = GridSize::new(
            pixel_dimension(pixels.width),
            pixel_dimension(pixels.height),
        );
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Where the UI wants the text cursor, if it shows one.
    pub fn cursor(&self) -> Option<Position> {
        self.cursor_visible.then_some(self.cursor)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixel_dimension(v: f32) -> u16 {
    v.clamp(0.0, f32::from(u16::MAX)) as u16
}

impl Backend for GridBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if let Some(slot) = self.buffer.cell_mut((x, y)) {
                slot.clone_from(cell);
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.buffer.reset();
        Ok(())
    }

    fn clear_region(&mut self, _region: ClearType) -> io::Result<()> {
        // Only inline viewports clear partial regions; the window is always
        // a full-screen viewport.
        self.buffer.reset();
        Ok(())
    }

    fn size(&self) -> io::Result<GridSize> {
        Ok(self.buffer.area.as_size())
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.buffer.area.as_size(),
            pixels: self.pixels,
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Cell size at the current font size, and how the grid sits in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font_size: f32,
    pub cell: Size,
}

impl Metrics {
    pub fn new(font_size: f32) -> Self {
        Self {
            font_size,
            cell: theme::cell_size(font_size),
        }
    }

    /// Columns and rows that fit in a window of this size.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn grid(&self, window: Size) -> (u16, u16) {
        let fit = |space: f32, cell: f32| {
            ((space - 2.0 * PAD) / cell)
                .floor()
                .clamp(1.0, f32::from(u16::MAX)) as u16
        };
        (
            fit(window.width, self.cell.width),
            fit(window.height, self.cell.height),
        )
    }

    /// Top-left corner of a `cols` × `rows` grid centered in the window,
    /// on a whole pixel.
    pub fn origin(&self, window: Size, cols: u16, rows: u16) -> Point {
        let w = f32::from(cols) * self.cell.width;
        let h = f32::from(rows) * self.cell.height;
        Point::new(
            ((window.width - w) / 2.0).max(0.0).floor(),
            ((window.height - h) / 2.0).max(0.0).floor(),
        )
    }

    /// The cell under a window position, clamped into the grid so drags
    /// past the edge keep selecting.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn cell_at(&self, window: Size, cols: u16, rows: u16, at: Point) -> Position {
        let origin = self.origin(window, cols, rows);
        let index = |offset: f32, cell: f32, count: u16| {
            ((offset / cell).floor().max(0.0) as u16).min(count.saturating_sub(1))
        };
        Position::new(
            index(at.x - origin.x, self.cell.width, cols),
            index(at.y - origin.y, self.cell.height, rows),
        )
    }
}

/// Colors that ratatui's `Reset` stands for, and the theme colors the
/// window draws with outside the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub fg: Color,
    pub bg: Color,
    /// The theme's primary accent.
    pub accent: Color,
    /// Text that stays in the background.
    pub muted: Color,
}

impl Palette {
    pub fn from_theme(theme: &aura_ui_terminal::Theme) -> Self {
        let fg = color(theme.colors.foreground, Color::from_rgb8(0xe6, 0xe6, 0xe6));
        Self {
            fg,
            bg: color(theme.colors.background, Color::from_rgb8(0x0d, 0x0d, 0x0d)),
            accent: color(theme.colors.primary, fg),
            muted: color(theme.colors.muted, Color::from_rgb8(0x88, 0x88, 0x88)),
        }
    }
}

/// A terminal color as an iced color; `Reset` becomes `default`.
pub fn color(c: TermColor, default: Color) -> Color {
    let index = match c {
        TermColor::Reset => return default,
        TermColor::Rgb(r, g, b) => return Color::from_rgb8(r, g, b),
        TermColor::Indexed(i) => i,
        TermColor::Black => 0,
        TermColor::Red => 1,
        TermColor::Green => 2,
        TermColor::Yellow => 3,
        TermColor::Blue => 4,
        TermColor::Magenta => 5,
        TermColor::Cyan => 6,
        TermColor::Gray => 7,
        TermColor::DarkGray => 8,
        TermColor::LightRed => 9,
        TermColor::LightGreen => 10,
        TermColor::LightYellow => 11,
        TermColor::LightBlue => 12,
        TermColor::LightMagenta => 13,
        TermColor::LightCyan => 14,
        TermColor::White => 15,
    };
    indexed(index)
}

/// The xterm 256-color palette.
fn indexed(i: u8) -> Color {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x00, 0xcd, 0x00),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xcc, 0xcc, 0xcc),
        (0x76, 0x76, 0x76),
        (0xf1, 0x4c, 0x4c),
        (0x00, 0xff, 0x00),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => {
            let (r, g, b) = BASE[usize::from(i)];
            Color::from_rgb8(r, g, b)
        }
        16..=231 => {
            let n = usize::from(i - 16);
            Color::from_rgb8(LEVELS[n / 36], LEVELS[n / 6 % 6], LEVELS[n % 6])
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            Color::from_rgb8(v, v, v)
        }
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
        let mut fg = color(cell.fg, palette.fg);
        let mut bg = color(cell.bg, palette.bg);
        if m.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if m.contains(Modifier::DIM) {
            fg = mix(fg, bg, 0.45);
        }
        let underline = m
            .contains(Modifier::UNDERLINED)
            .then(|| color(cell.underline_color, fg));
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

    fn font(&self) -> Font {
        face((theme::mono(), theme::mono_bold()), self.bold, self.italic)
    }

    fn prose_font(&self) -> Font {
        face((theme::sans(), theme::sans_bold()), self.bold, self.italic)
    }
}

/// The regular or bold font of a family, upright or italic.
fn face((regular, heavy): (Font, Font), bold: bool, italic: bool) -> Font {
    let font = if bold { heavy } else { regular };
    if italic {
        Font {
            style: Style::Italic,
            ..font
        }
    } else {
        font
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

/// The cells last painted, and the cached drawing of them.
#[derive(Default)]
pub struct Screen {
    cells: Vec<Cell>,
    cols: u16,
    rows: u16,
    cursor: Option<Position>,
    /// Rows of chat prose, set in the proportional face
    prose: Vec<ProseRow>,
    /// The background behind the rows
    backdrop: Cache,
    /// Each row's drawing, by row
    lines: Vec<Cache>,
}

impl Screen {
    /// Take the cells of a drawn frame and the UI's prose rows, and
    /// return how many rows changed and will be repainted. The whole
    /// frame, not the backend's copy: a prose cell can hold several
    /// characters, which ratatui's diff takes for a wide glyph and skips
    /// the cells after.
    pub fn update(&mut self, buffer: &Buffer, prose: &[ProseRow]) -> usize {
        let (cols, rows) = (buffer.area.width, buffer.area.height);
        let changed = if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            self.lines = (0..rows).map(|_| Cache::new()).collect();
            self.backdrop.clear();
            self.lines.len()
        } else {
            let width = usize::from(cols);
            let mut changed = 0;
            for (y, line) in self.lines.iter().enumerate() {
                let cells = y * width..(y + 1) * width;
                if self.cells[cells.clone()] != buffer.content[cells]
                    || prose_at(&self.prose, y) != prose_at(prose, y)
                {
                    line.clear();
                    changed += 1;
                }
            }
            changed
        };
        if changed > 0 {
            self.cells.clone_from(&buffer.content);
            prose.clone_into(&mut self.prose);
        }
        changed
    }

    pub fn set_cursor(&mut self, cursor: Option<Position>) {
        self.cursor = cursor;
    }

    /// Drop the cached drawing (the font size or colors changed).
    pub fn invalidate(&self) {
        self.backdrop.clear();
        for line in &self.lines {
            line.clear();
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for row in self.cells.chunks(usize::from(self.cols.max(1))) {
            for cell in row {
                out.push_str(cell.symbol());
            }
            out.push('\n');
        }
        out
    }
}

/// Canvas program painting a [`Screen`].
pub struct Grid<'a> {
    pub screen: &'a Screen,
    /// The panels' title bars, over the cells
    pub chrome: &'a Chrome,
    pub metrics: Metrics,
    pub palette: Palette,
    /// Whether the blinking cursor is in its visible phase.
    pub cursor_on: bool,
}

impl<Message> canvas::Program<Message> for Grid<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let size = bounds.size();
        let (cols, rows) = self.screen.size();
        let origin = self.metrics.origin(size, cols, rows);
        let backdrop = self.screen.backdrop.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, size, self.palette.bg);
        });
        let mut layers = vec![backdrop];
        layers.extend(
            self.screen.lines.iter().enumerate().map(|(y, line)| {
                line.draw(renderer, size, |frame| self.paint_row(frame, origin, y))
            }),
        );
        layers.extend(self.chrome.draw(renderer, size));
        if let Some(at) = self.screen.cursor.filter(|_| self.cursor_on) {
            if at.x < cols && at.y < rows {
                let mut frame = Frame::new(renderer, size);
                self.paint_cursor(&mut frame, origin, at);
                layers.push(frame.into_geometry());
            }
        }
        layers
    }
}

impl Grid<'_> {
    /// Row `index` of the screen.
    fn paint_row(&self, frame: &mut Frame, origin: Point, index: usize) {
        let width = usize::from(self.screen.cols);
        let Some(cells) = self.screen.cells.get(index * width..(index + 1) * width) else {
            return;
        };
        let looks: Vec<Look> = cells.iter().map(|c| Look::of(c, self.palette)).collect();
        let row = Row {
            cells,
            looks: &looks,
            y: origin.y + index as f32 * self.metrics.cell.height,
            origin: origin.x,
        };
        let prose = prose_at(&self.screen.prose, index);
        let columns = prose.map_or(0..0, |p| {
            let start = usize::from(p.x + p.indent).min(cells.len());
            start..usize::from(p.x + p.width).clamp(start, cells.len())
        });
        for grid in [0..columns.start, columns.end..cells.len()] {
            self.paint_backgrounds(frame, &row, grid.clone());
            self.paint_cells(frame, &row, grid);
        }
        if let Some(prose) = prose {
            self.paint_prose(frame, &row, columns, &prose.code);
        }
    }

    /// A prose row's `columns` after its indent, laid out in flow: runs
    /// of text in the proportional face, as wide as they measure; inline
    /// `code` a cell per column, in the monospace face.
    fn paint_prose(
        &self,
        frame: &mut Frame,
        row: &Row<'_>,
        columns: Range<usize>,
        code: &[Range<u16>],
    ) {
        let (cells, looks, y) = (row.cells, row.looks, row.y);
        let cell = self.metrics.cell;
        let is_code =
            |i: usize| u16::try_from(i).is_ok_and(|i| code.iter().any(|r| r.contains(&i)));
        let mut x = self.x_at(row, columns.start);
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
            let (font, size, width) = if code {
                let size = self.metrics.font_size;
                (look.font(), size, (end - col) as f32 * cell.width)
            } else {
                let (font, size) = (look.prose_font(), theme::prose_size(self.metrics.font_size));
                (font, size, run_width(&run, font, size))
            };
            self.fill_background(frame, look.bg, x.round(), (x + width).round(), y);
            if !look.hidden {
                self.run_text(
                    frame,
                    &run,
                    Point::new(x, y),
                    &look,
                    (font, size, Shaping::Advanced),
                );
            }
            self.decorate(frame, &look, x, x + width, y);
            x += width;
            col = end;
        }
    }

    /// A block over the cell at `at`, its character cut out of the block
    /// in the cell's background color.
    fn paint_cursor(&self, frame: &mut Frame, origin: Point, at: Position) {
        let cell = self.metrics.cell;
        let x = (origin.x + f32::from(at.x) * cell.width).round();
        let next = (origin.x + f32::from(at.x + 1) * cell.width).round();
        let y = origin.y + f32::from(at.y) * cell.height;
        frame.fill_rectangle(
            Point::new(x, y),
            Size::new(next - x, cell.height),
            self.palette.fg,
        );
        let index = usize::from(at.y) * usize::from(self.screen.cols) + usize::from(at.x);
        let Some(under) = self.screen.cells.get(index) else {
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
            self.paint_symbol(frame, under.symbol(), (x, y), &cut);
        }
    }

    /// Backgrounds of `columns`, one rectangle per run of the same color.
    fn paint_backgrounds(&self, frame: &mut Frame, row: &Row<'_>, columns: Range<usize>) {
        let edge = |col: usize| self.x_at(row, col).round();
        let mut start = columns.start;
        while start < columns.end {
            let bg = row.looks[start].bg;
            let end = (start..columns.end)
                .find(|&i| row.looks[i].bg != bg)
                .unwrap_or(columns.end);
            self.fill_background(frame, bg, edge(start), edge(end), row.y);
            start = end;
        }
    }

    /// Foreground of `columns`: runs of plain ASCII share one text draw;
    /// anything else is a built-in shape or a single centered glyph.
    fn paint_cells(&self, frame: &mut Frame, row: &Row<'_>, columns: Range<usize>) {
        let (cells, looks, y) = (row.cells, row.looks, row.y);
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
                let at = Point::new(self.x_at(row, col), y);
                let size = self.metrics.font_size;
                self.run_text(frame, &run, at, &look, (look.font(), size, Shaping::Basic));
                self.decorate(frame, &look, at.x, self.x_at(row, end), y);
                col = end;
                continue;
            }
            self.paint_symbol(frame, symbol, (self.x_at(row, col), y), &look);
            col += 1;
        }
    }

    /// One symbol that is not plain ASCII, in the cells from `at`;
    /// returns how wide it is.
    fn paint_symbol(&self, frame: &mut Frame, symbol: &str, at: (f32, f32), look: &Look) -> f32 {
        let cell = self.metrics.cell;
        let width = symbol.width().max(1);
        let area = Rectangle::new(
            Point::new(at.0, at.1),
            Size::new(cell.width * width as f32, cell.height),
        );
        if !look.hidden {
            let mut chars = symbol.chars();
            let built_in = match (chars.next(), chars.next()) {
                (Some(c), None) if width == 1 => glyphs::draw(frame, c, area, look.fg),
                _ => false,
            };
            if !built_in && !symbol.trim().is_empty() {
                self.text(
                    frame,
                    symbol.to_owned(),
                    Point::new(area.center_x(), at.1),
                    look,
                    (look.font(), self.metrics.font_size, Shaping::Advanced),
                    true,
                );
            }
        }
        self.decorate(frame, look, area.x, area.x + area.width, at.1);
        area.width
    }

    fn x_at(&self, row: &Row<'_>, col: usize) -> f32 {
        row.origin + col as f32 * self.metrics.cell.width
    }

    fn fill_background(&self, frame: &mut Frame, bg: Color, x0: f32, x1: f32, y: f32) {
        if bg != self.palette.bg && x1 > x0 {
            frame.fill_rectangle(
                Point::new(x0, y),
                Size::new(x1 - x0, self.metrics.cell.height),
                bg,
            );
        }
    }

    /// A run of text at `at`, unless it is only spaces.
    fn run_text(
        &self,
        frame: &mut Frame,
        run: &str,
        at: Point,
        look: &Look,
        face: (Font, f32, Shaping),
    ) {
        let text = run.trim_end();
        if !text.is_empty() {
            self.text(frame, text.to_owned(), at, look, face, false);
        }
    }

    fn text(
        &self,
        frame: &mut Frame,
        content: String,
        at: Point,
        look: &Look,
        (font, size, shaping): (Font, f32, Shaping),
        centered: bool,
    ) {
        frame.fill_text(canvas::Text {
            content,
            position: at,
            color: look.fg,
            size: Pixels(size),
            line_height: LineHeight::Absolute(Pixels(self.metrics.cell.height)),
            font,
            align_x: if centered {
                Alignment::Center
            } else {
                Alignment::Left
            },
            align_y: Vertical::Top,
            shaping,
            ..canvas::Text::default()
        });
    }

    fn decorate(&self, frame: &mut Frame, look: &Look, x0: f32, x1: f32, y: f32) {
        let cell = self.metrics.cell;
        let t = (self.metrics.font_size / 14.0).round().max(1.0);
        if let Some(color) = look.underline {
            frame.fill_rectangle(
                Point::new(x0, y + cell.height - 2.0 * t),
                Size::new(x1 - x0, t),
                color,
            );
        }
        if look.strike {
            let mid = (y + cell.height / 2.0).round();
            frame.fill_rectangle(Point::new(x0, mid), Size::new(x1 - x0, t), look.fg);
        }
    }
}

/// The prose on screen row `y`, if it holds any.
fn prose_at(prose: &[ProseRow], y: usize) -> Option<&ProseRow> {
    prose.iter().find(|p| usize::from(p.y) == y)
}

/// One row of the screen as it is painted.
struct Row<'a> {
    cells: &'a [Cell],
    looks: &'a [Look],
    /// Top of the row, in pixels
    y: f32,
    /// Left edge of the grid, in pixels
    origin: f32,
}

/// How many cells `text` takes in the proportional face: what the
/// terminal UI wraps chat prose to.
pub fn prose_cells(text: &str, modifier: Modifier) -> f32 {
    /// Size widths are measured at, large enough that rounding is lost
    const SIZE: f32 = 100.0;
    let font = face(
        (theme::sans(), theme::sans_bold()),
        modifier.contains(Modifier::BOLD),
        modifier.contains(Modifier::ITALIC),
    );
    run_width(text, font, theme::prose_size(SIZE)) / theme::cell_size(SIZE).width
}

/// How wide a run of `text` is in `font` at `size`, trailing spaces
/// included.
fn run_width(text: &str, font: Font, size: f32) -> f32 {
    /// Widths by text, font and font size
    type Widths = HashMap<(String, Font, u32), f32>;
    static WIDTHS: LazyLock<Mutex<Widths>> = LazyLock::new(Mutex::default);
    let mut widths = WIDTHS.lock().unwrap_or_else(PoisonError::into_inner);
    if widths.len() > MAX_MEASURED {
        widths.clear();
    }
    let mut width = |text: &str| {
        *widths
            .entry((text.to_owned(), font, size.to_bits()))
            .or_insert_with(|| text_width(text, font, size))
    };
    // A closing bar keeps trailing spaces in the measured line.
    width(&format!("{text}|")) - width("|")
}

/// How wide `text` is laid out on one line in `font` at `size`.
fn text_width(text: &str, font: Font, size: f32) -> f32 {
    iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::text::Text {
        content: text,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: LineHeight::default(),
        font,
        align_x: Alignment::Left,
        align_y: Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::None,
    })
    .min_width()
}

/// One printable ASCII character, which every monospace font has at the
/// cell's advance.
fn is_plain(symbol: &str) -> bool {
    let bytes = symbol.as_bytes();
    bytes.len() == 1 && (0x20..0x7f).contains(&bytes[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style as TermStyle;

    #[test]
    fn backend_records_cells_and_cursor() {
        let mut terminal = ratatui::Terminal::new(GridBackend::new(10, 2)).unwrap();
        terminal
            .draw(|f| {
                f.buffer_mut().set_string(1, 0, "hi", TermStyle::default());
                f.set_cursor_position((3, 1));
            })
            .unwrap();
        let backend = terminal.backend();
        assert_eq!(backend.buffer()[(1, 0)].symbol(), "h");
        assert_eq!(backend.buffer()[(2, 0)].symbol(), "i");
        assert_eq!(backend.cursor(), Some(Position::new(3, 1)));

        terminal.draw(|_| {}).unwrap();
        assert_eq!(terminal.backend().cursor(), None);
    }

    #[test]
    fn only_rows_whose_cells_changed_are_repainted() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 4));
        let mut screen = Screen::default();
        assert_eq!(
            screen.update(&buffer, &[]),
            4,
            "a new size paints every row"
        );
        assert_eq!(screen.update(&buffer, &[]), 0);

        buffer.set_string(2, 1, "hi", TermStyle::default());
        assert_eq!(screen.update(&buffer, &[]), 1);
        assert_eq!(screen.update(&buffer, &[]), 0);

        buffer.set_string(0, 0, "x", TermStyle::default());
        buffer.set_string(0, 3, "y", TermStyle::default());
        assert_eq!(screen.update(&buffer, &[]), 2);
        assert_eq!(screen.text().lines().nth(3), Some("y         "));

        buffer.resize(Rect::new(0, 0, 12, 4));
        assert_eq!(screen.update(&buffer, &[]), 4);
    }

    #[test]
    fn terminal_follows_backend_resize() {
        let mut terminal = ratatui::Terminal::new(GridBackend::new(10, 2)).unwrap();
        terminal.backend_mut().resize(20, 5, Size::new(168.0, 90.0));
        let frame = terminal
            .draw(|f| assert_eq!(f.area(), Rect::new(0, 0, 20, 5)))
            .unwrap();
        assert_eq!(frame.area, Rect::new(0, 0, 20, 5));
    }

    #[test]
    fn grid_fits_the_window_and_maps_points_to_cells() {
        let m = Metrics::new(14.0);
        assert!((m.cell.width - 8.4).abs() < 1e-4);
        assert!((m.cell.height - 18.0).abs() < f32::EPSILON);
        let window = Size::new(1012.0, 732.0);
        let (cols, rows) = m.grid(window);
        assert_eq!((cols, rows), (119, 40));
        let origin = m.origin(window, cols, rows);
        assert!(origin.x >= PAD && origin.y >= PAD);
        let inside = Point::new(origin.x + 8.4 * 3.5, origin.y + 18.0 * 2.5);
        assert_eq!(m.cell_at(window, cols, rows, inside), Position::new(3, 2));
        assert_eq!(
            m.cell_at(window, cols, rows, Point::ORIGIN),
            Position::new(0, 0)
        );
        assert_eq!(
            m.cell_at(window, cols, rows, Point::new(5000.0, 5000.0)),
            Position::new(cols - 1, rows - 1)
        );
    }

    #[test]
    fn colors_map_like_xterm() {
        let d = Color::BLACK;
        assert_eq!(color(TermColor::Reset, d), d);
        assert_eq!(
            color(TermColor::Rgb(1, 244, 203), d),
            Color::from_rgb8(1, 244, 203)
        );
        assert_eq!(color(TermColor::Indexed(16), d), Color::from_rgb8(0, 0, 0));
        assert_eq!(
            color(TermColor::Indexed(231), d),
            Color::from_rgb8(255, 255, 255)
        );
        assert_eq!(color(TermColor::Indexed(232), d), Color::from_rgb8(8, 8, 8));
        assert_eq!(color(TermColor::White, d), Color::from_rgb8(255, 255, 255));
    }

    #[test]
    fn reversed_cells_swap_colors() {
        let palette = Palette {
            fg: Color::WHITE,
            bg: Color::BLACK,
            accent: Color::WHITE,
            muted: Color::WHITE,
        };
        let mut cell = Cell::default();
        cell.modifier = Modifier::REVERSED;
        let look = Look::of(&cell, palette);
        assert_eq!((look.fg, look.bg), (Color::BLACK, Color::WHITE));
    }
}
