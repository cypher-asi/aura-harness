//! The terminal screen: a ratatui backend that keeps the cell grid the
//! terminal UI draws into, where that grid sits in the window, and the
//! colors its cells name. [`crate::paint`] draws it.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

use crate::geom::{Color, Point, Size};
use crate::theme;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size as GridSize};
use ratatui::style::Color as TermColor;
use std::convert::Infallible;

/// Space kept clear around the grid, in pixels of a plain display.
pub const PAD: f32 = 6.0;
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
    // Drawing into memory cannot fail.
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Infallible>
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

    fn hide_cursor(&mut self) -> Result<(), Infallible> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Infallible> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Infallible> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Infallible> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Infallible> {
        self.buffer.reset();
        Ok(())
    }

    fn clear_region(&mut self, _region: ClearType) -> Result<(), Infallible> {
        // Only inline viewports clear partial regions; the window is always
        // a full-screen viewport.
        self.buffer.reset();
        Ok(())
    }

    fn size(&self) -> Result<GridSize, Infallible> {
        Ok(self.buffer.area.as_size())
    }

    fn window_size(&mut self) -> Result<WindowSize, Infallible> {
        Ok(WindowSize {
            columns_rows: self.buffer.area.as_size(),
            pixels: self.pixels,
        })
    }

    fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}

/// Cell size at the current font size, and how the grid sits in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font_size: f32,
    pub cell: Size,
    /// Space kept clear beside and below the grid and above the tab bar
    pub pad: f32,
}

impl Metrics {
    pub fn new(font_size: f32) -> Self {
        Self::on_display(font_size, 1.0)
    }

    /// Sizes at `font_size` in the pixels of a display that has `scale`
    /// of them to a plain display's one.
    pub fn on_display(font_size: f32, scale: f32) -> Self {
        let font_size = font_size * scale;
        Self {
            font_size,
            cell: theme::cell_size(font_size),
            pad: (PAD * scale).round(),
        }
    }

    /// Columns and rows that fit in a window of this size.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn grid(&self, window: Size) -> (u16, u16) {
        let fit =
            |space: f32, cell: f32| (space / cell).floor().clamp(1.0, f32::from(u16::MAX)) as u16;
        (
            fit(window.width - 2.0 * self.pad, self.cell.width),
            fit(window.height - self.top() - self.pad, self.cell.height),
        )
    }

    /// Space above the grid: half a row, so the middle of the first row
    /// is as far from the tab bar as from the panels' top borders, drawn
    /// through the middle of the second.
    fn top(&self) -> f32 {
        (self.cell.height / 2.0).round()
    }

    /// Height of a single-line list's row, in pixels: 1.3 rows.
    pub fn list_row(&self) -> f32 {
        theme::list_row_height(self.cell.height)
    }

    /// How tall cells and list rows are, for the terminal UI to lay its
    /// lists out by.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn list_pitch(&self) -> aura_ui_terminal::ListPitch {
        let pixels = |height: f32| height.clamp(1.0, f32::from(u16::MAX)) as u16;
        aura_ui_terminal::ListPitch {
            cell: pixels(self.cell.height),
            tall: pixels(self.list_row()),
        }
    }

    /// Top-left corner of a `cols` × `rows` grid centered across the
    /// window and [`Self::top`] down it, on a whole pixel.
    pub fn origin(&self, window: Size, cols: u16) -> Point {
        let w = f32::from(cols) * self.cell.width;
        Point::new(((window.width - w) / 2.0).max(0.0).floor(), self.top())
    }

    /// The cell under a window position, clamped into the grid so drags
    /// past the edge keep selecting.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn cell_at(&self, window: Size, cols: u16, rows: u16, at: Point) -> Position {
        let origin = self.origin(window, cols);
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
    /// The theme's status accent: what is live or working.
    pub accent: Color,
    /// Text that stays in the background.
    pub muted: Color,
    /// Outlines and boxes at rest.
    pub faint: Color,
    /// The fill of the selected tab.
    pub selection: Color,
}

impl Palette {
    pub fn from_theme(theme: &aura_ui_terminal::Theme) -> Self {
        let fg = color(theme.colors.foreground, Color::from_rgb8(0xe6, 0xe6, 0xe6));
        let muted = color(theme.colors.muted, Color::from_rgb8(0x88, 0x88, 0x88));
        Self {
            fg,
            bg: color(theme.colors.background, Color::from_rgb8(0x0d, 0x0d, 0x0d)),
            accent: color(theme.colors.success, fg),
            muted,
            faint: color(theme.colors.faint, muted),
            selection: color(theme.colors.selection, muted),
        }
    }
}

/// A terminal color as the window draws it; `Reset` becomes `default`.
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
        assert_eq!((cols, rows), (119, 39));
        let origin = m.origin(window, cols);
        assert!(origin.x >= PAD);
        assert!((origin.y - 9.0).abs() < f32::EPSILON, "half a row down");
        let below = window.height - origin.y - f32::from(rows) * m.cell.height;
        assert!(below >= PAD, "{below}");
        let inside = Point::new(origin.x + 8.4 * 3.5, origin.y + 18.0 * 2.5);
        assert_eq!(m.cell_at(window, cols, rows, inside), Position::new(3, 2));
        assert_eq!(
            m.cell_at(window, cols, rows, Point::default()),
            Position::new(0, 0)
        );
        assert_eq!(
            m.cell_at(window, cols, rows, Point::new(5000.0, 5000.0)),
            Position::new(cols - 1, rows - 1)
        );
    }

    #[test]
    fn colors_map_like_xterm() {
        let d = Color::from_rgb8(0, 0, 0);
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
}
