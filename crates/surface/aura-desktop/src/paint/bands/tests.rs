use super::*;
use crate::chrome::Chrome;
use crate::grid::Palette;
use crate::paint::View;
use crate::scene::{Fill, Quad};
use crate::tab_bar::{self, Bar};
use crate::text::TextSystem;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as TermColor, Style};

const COLS: u16 = 20;
const ROWS: u16 = 4;
const RED: TermColor = TermColor::Rgb(200, 0, 0);
const BLUE: TermColor = TermColor::Rgb(0, 0, 200);

/// A 14 px grid: rows 18 px, list rows 23.
fn metrics() -> Metrics {
    Metrics::new(14.0)
}

/// A band over the whole grid; the list fits in 3 of its 4 rows.
fn band() -> ListBand {
    ListBand::new(0..COLS, 0..ROWS, 3)
}

#[test]
fn the_pitch_is_whole_pixels_a_third_taller() {
    let metrics = metrics();
    assert!((metrics.cell.height - 18.0).abs() < f32::EPSILON);
    let pitch = metrics.list_pitch();
    assert_eq!((pitch.cell, pitch.tall), (18, 23));
    assert_eq!(pitch.fit(ROWS), 3);
}

#[test]
fn the_pointer_finds_the_row_drawn_under_it() {
    let bands = [band()];
    let at = |y: f32| {
        let plain = Position::new(3, (y / 18.0) as u16);
        cell_in_bands(&bands, metrics(), plain, y).y
    };
    assert_eq!(at(5.0), 0);
    assert_eq!(at(23.0 + 5.0), 1, "the second stretched row");
    assert_eq!(at(2.0 * 23.0 + 22.0), 2);
    assert_eq!(at(3.0 * 23.0 + 1.0), 3, "under the stretched rows");
    let outside = Position::new(3, 1);
    assert_eq!(cell_in_bands(&[], metrics(), outside, 30.0), outside);
}

#[test]
fn the_pointer_on_a_menu_over_a_band_is_on_the_menu() {
    let mut band = band();
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
    band.capture(&buffer);
    buffer.set_string(5, 1, "menu", Style::default());
    band.cover(&buffer);
    let on_menu = Position::new(6, 1);
    assert_eq!(cell_in_bands(&[band], metrics(), on_menu, 20.0), on_menu);
}

/// The scene of `buffer` with `bands`, and where its grid's first row is.
fn paint(buffer: &Buffer, bands: &[ListBand]) -> (Vec<Quad>, i32) {
    let palette = Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber"));
    let (bar, chrome) = (Bar::default(), Chrome::default());
    let metrics = metrics();
    let view = View {
        size: Size::new(168.0, 29.0 + 9.0 + 72.0),
        cells: &buffer.content,
        cols: COLS,
        rows: ROWS,
        prose: &[],
        panes: &[],
        bands,
        cursor: None,
        metrics,
        palette,
        bar: &bar,
        chrome: &chrome,
        slide: None,
        canvas: None,
    };
    let mut text = TextSystem::new();
    let quads = Painter::default().paint_with(&view, &mut text).quads.clone();
    let top = (tab_bar::height(&metrics) + metrics.origin(view.size, COLS).y) as i32;
    (quads, top)
}

/// The solid quads of `color`, as `(y, h)`.
fn fills(quads: &[Quad], (r, g, b): (u8, u8, u8)) -> Vec<(i32, u32)> {
    quads
        .iter()
        .filter(|q| q.fill == Fill::Solid && q.color[..3] == [r, g, b])
        .map(|q| (q.y, q.h))
        .collect()
}

#[test]
fn a_bands_rows_are_painted_a_list_row_tall() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
    buffer.set_string(0, 1, "  row one  ", Style::default().bg(RED));
    let (plain, top) = paint(&buffer, &[]);
    assert_eq!(fills(&plain, (200, 0, 0)), [(top + 18, 18)]);

    let mut band = band();
    band.capture(&buffer);
    band.cover(&buffer);
    let (tall, top) = paint(&buffer, &[band]);
    assert_eq!(fills(&tall, (200, 0, 0)), [(top + 23, 23)], "row 1 at 1 \u{d7} 23");
}

#[test]
fn what_is_drawn_over_a_band_stays_where_the_grid_has_it() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
    buffer.set_string(0, 1, "  row one  ", Style::default().bg(RED));
    let mut band = band();
    band.capture(&buffer);
    // A menu over the list's second row, drawn after the panels.
    buffer.set_string(4, 1, "menu", Style::default().bg(BLUE));
    band.cover(&buffer);
    let (quads, top) = paint(&buffer, &[band]);
    assert_eq!(
        fills(&quads, (0, 0, 200)),
        [(top + 18, 18)],
        "the menu, unstretched"
    );
    assert!(
        fills(&quads, (200, 0, 0))
            .iter()
            .all(|&(y, h)| (y, h) == (top + 23, 23)),
        "the row under it, stretched from the copy"
    );
}
