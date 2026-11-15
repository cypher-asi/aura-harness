//! The terminal screen: a ratatui backend that keeps the cell grid the
//! terminal UI draws into, and a canvas program that paints it.
//!
//! Painting works the way GPU terminals do: one rectangle per run of
//! background color, one text draw per run of same-styled ASCII, and
//! box, block, braille and status characters drawn as shapes
//! ([`crate::glyphs`]) so borders join and symbols the font lacks still
//! look right. The painted grid is cached until a cell changes; the
//! cursor is drawn on top every frame so it can blink.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

use crate::{glyphs, theme};
use iced::alignment::Vertical;
use iced::font::Style;
use iced::widget::canvas::{self, Cache, Frame, Geometry};
use iced::widget::text::{Alignment, LineHeight, Shaping};
use iced::{mouse, Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size as GridSize};
use ratatui::style::{Color as TermColor, Modifier};
use std::io;
use unicode_width::UnicodeWidthStr;

/// Space kept clear around the grid, in pixels.
const PAD: f32 = 6.0;
/// Width of the bar cursor, in pixels.
const CURSOR_WIDTH: f32 = 2.0;

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

/// Colors that ratatui's `Reset` stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub fg: Color,
    pub bg: Color,
}

impl Palette {
    pub fn from_theme(theme: &aura_terminal::Theme) -> Self {
        Self {
            fg: color(theme.colors.foreground, Color::from_rgb8(0xe6, 0xe6, 0xe6)),
            bg: color(theme.colors.background, Color::from_rgb8(0x0d, 0x0d, 0x0d)),
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
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xcc, 0xcc, 0xcc),
        (0x76, 0x76, 0x76),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
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
        let font = if self.bold {
            theme::mono_bold()
        } else {
            theme::mono()
        };
        if self.italic {
            Font {
                style: Style::Italic,
                ..font
            }
        } else {
            font
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

/// The cells last painted, and the cached drawing of them.
#[derive(Default)]
pub struct Screen {
    cells: Vec<Cell>,
    cols: u16,
    rows: u16,
    cursor: Option<Position>,
    cache: Cache,
}

impl Screen {
    /// Take the backend's cells; repaints only if any changed.
    pub fn update(&mut self, backend: &GridBackend) {
        let buffer = backend.buffer();
        let (cols, rows) = (buffer.area.width, buffer.area.height);
        if cols != self.cols || rows != self.rows || self.cells != buffer.content {
            self.cells.clone_from(&buffer.content);
            self.cols = cols;
            self.rows = rows;
            self.cache.clear();
        }
        self.cursor = backend.cursor();
    }

    /// Drop the cached drawing (the font size or colors changed).
    pub fn invalidate(&self) {
        self.cache.clear();
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
        let cells = self.screen.cache.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, size, self.palette.bg);
            self.paint(frame, origin);
        });
        let mut layers = vec![cells];
        if let Some(at) = self.screen.cursor.filter(|_| self.cursor_on) {
            if at.x < cols && at.y < rows {
                let mut frame = Frame::new(renderer, size);
                let cell = self.metrics.cell;
                let x = (origin.x + f32::from(at.x) * cell.width).round();
                let y = origin.y + f32::from(at.y) * cell.height;
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(CURSOR_WIDTH, cell.height),
                    self.palette.fg,
                );
                layers.push(frame.into_geometry());
            }
        }
        layers
    }
}

impl Grid<'_> {
    fn paint(&self, frame: &mut Frame, origin: Point) {
        let (cols, rows) = self.screen.size();
        if cols == 0 {
            return;
        }
        let cell = self.metrics.cell;
        let x_at = |col: usize| origin.x + col as f32 * cell.width;
        let edge = |col: usize| x_at(col).round();
        for (row, cells) in self
            .screen
            .cells
            .chunks(usize::from(cols))
            .take(usize::from(rows))
            .enumerate()
        {
            let y = origin.y + row as f32 * cell.height;
            let looks: Vec<Look> = cells.iter().map(|c| Look::of(c, self.palette)).collect();

            // Backgrounds, one rectangle per run of the same color.
            let mut start = 0;
            while start < cells.len() {
                let bg = looks[start].bg;
                let end = (start..cells.len())
                    .find(|&i| looks[i].bg != bg)
                    .unwrap_or(cells.len());
                if bg != self.palette.bg {
                    let (x0, x1) = (edge(start), edge(end));
                    frame.fill_rectangle(Point::new(x0, y), Size::new(x1 - x0, cell.height), bg);
                }
                start = end;
            }

            // Foreground: runs of plain ASCII share one text draw; anything
            // else is a built-in shape or a single centered glyph.
            let mut col = 0;
            while col < cells.len() {
                let look = looks[col];
                let symbol = cells[col].symbol();
                if cells[col].skip || look.hidden {
                    col += 1;
                    continue;
                }
                if is_plain(symbol) {
                    let end = (col..cells.len())
                        .find(|&i| {
                            looks[i] != look || cells[i].skip || !is_plain(cells[i].symbol())
                        })
                        .unwrap_or(cells.len());
                    let run: String = cells[col..end].iter().map(Cell::symbol).collect();
                    if !run.trim_end().is_empty() {
                        self.text(
                            frame,
                            run.trim_end().to_owned(),
                            Point::new(x_at(col), y),
                            &look,
                            Shaping::Basic,
                            false,
                        );
                    }
                    self.decorate(frame, &look, x_at(col), x_at(end), y);
                    col = end;
                    continue;
                }
                let width = symbol.width().max(1);
                let area = Rectangle::new(
                    Point::new(x_at(col), y),
                    Size::new(cell.width * width as f32, cell.height),
                );
                let mut chars = symbol.chars();
                let built_in = match (chars.next(), chars.next()) {
                    (Some(c), None) if width == 1 => glyphs::draw(frame, c, area, look.fg),
                    _ => false,
                };
                if !built_in && !symbol.trim().is_empty() {
                    let center = Point::new(area.center_x(), y);
                    self.text(
                        frame,
                        symbol.to_owned(),
                        center,
                        &look,
                        Shaping::Advanced,
                        true,
                    );
                }
                self.decorate(frame, &look, area.x, area.x + area.width, y);
                col += 1;
            }
        }
    }

    fn text(
        &self,
        frame: &mut Frame,
        content: String,
        at: Point,
        look: &Look,
        shaping: Shaping,
        centered: bool,
    ) {
        frame.fill_text(canvas::Text {
            content,
            position: at,
            color: look.fg,
            size: Pixels(self.metrics.font_size),
            line_height: LineHeight::Absolute(Pixels(self.metrics.cell.height)),
            font: look.font(),
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
        };
        let mut cell = Cell::default();
        cell.modifier = Modifier::REVERSED;
        let look = Look::of(&cell, palette);
        assert_eq!((look.fg, look.bg), (Color::BLACK, Color::WHITE));
    }
}
