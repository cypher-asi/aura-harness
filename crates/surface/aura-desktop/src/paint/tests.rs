use super::*;
use crate::raster;
use crate::scene::{Fill, Quad};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
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

/// `xx ` in the prose face, `xx` of inline code, ` xx` in the prose
/// face, laid out as a prose row; the code selected with `reversed`.
fn prose_with_code(text: &mut TextSystem, reversed: bool) -> Layers {
    let stage = Stage::new();
    let mut code = Style::default().bg(TermColor::Rgb(38, 38, 38));
    if reversed {
        code = code.add_modifier(Modifier::REVERSED);
    }
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, 1));
    buffer.set_string(0, 0, "xx ", Style::default());
    buffer.set_string(3, 0, "xx", code);
    buffer.set_string(5, 0, " xx", Style::default());
    let mut brush = Brush {
        text,
        metrics: Metrics::new(14.0),
        palette: stage.palette,
        origin: 0.0,
    };
    let cells = &buffer.content;
    let looks = brush.looks(cells);
    let mut out = Layers::default();
    brush.prose(
        &mut out,
        &Row {
            cells,
            looks: &looks,
        },
        0..8,
        std::slice::from_ref(&(3..5)),
    );
    out
}

#[test]
fn inline_code_sits_on_the_texts_baseline_on_a_background_inside_the_row() {
    let mut text = TextSystem::new();
    let out = prose_with_code(&mut text, false);
    let row = 18;
    let [chip] = out.under[..] else {
        panic!("only the code's background: {:?}", out.under);
    };
    assert!(chip.y > 0 && chip.bottom() <= row, "{chip:?}");
    assert!(chip.h < 16, "shorter than the row: {chip:?}");

    // Every x, monospace or not, ends on the same baseline.
    assert_eq!(out.over.len(), 6, "{:?}", out.over);
    let baseline = out.over[0].bottom();
    assert!(
        out.over.iter().all(|q| q.bottom() == baseline),
        "{:?}",
        out.over
    );
    let code: Vec<&Quad> = out
        .over
        .iter()
        .filter(|q| chip.x <= q.x && q.right() <= chip.right())
        .collect();
    assert_eq!(code.len(), 2, "the code's glyphs are on it");
    for glyph in code {
        assert!(
            chip.y < glyph.y && glyph.bottom() < chip.bottom(),
            "{glyph:?}"
        );
    }
}

#[test]
fn selected_inline_code_is_highlighted_across_the_row() {
    let mut text = TextSystem::new();
    let out = prose_with_code(&mut text, true);
    let [highlight] = out.under[..] else {
        panic!("only the selection: {:?}", out.under);
    };
    assert_eq!((highlight.y, highlight.h), (0, 18));
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
