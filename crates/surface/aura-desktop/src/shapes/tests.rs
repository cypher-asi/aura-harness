use super::cells::box_edges;
use super::*;
use crate::geom::Point;
use crate::scene::Fill;
use crate::text::TextSystem;
use ratatui::symbols::pixel::OCTANTS;

const WHITE: [u8; 4] = [255; 4];

fn cell(x: f32, y: f32) -> Rectangle {
    Rectangle {
        x,
        y,
        width: 8.4,
        height: 18.0,
    }
}

/// How much of pixel `(x, y)` the quads cover, 0 to 255.
fn coverage(quads: &[Quad], atlas: &Atlas, x: i32, y: i32) -> u8 {
    quads
        .iter()
        .filter(|q| x >= q.x && y >= q.y && x < q.x + q.w as i32 && y < q.y + q.h as i32)
        .map(|q| match q.fill {
            Fill::Solid => q.color[3],
            _ => {
                let (u, v) = (q.u + (x - q.x) as u32, q.v + (y - q.y) as u32);
                atlas.pixels()[((v * atlas.size().0 + u) * 4 + 3) as usize]
            }
        })
        .max()
        .unwrap_or(0)
}

fn drawn(c: char, at: Rectangle) -> (Vec<Quad>, TextSystem) {
    let mut text = TextSystem::new();
    let mut quads = Vec::new();
    assert!(draw(&mut quads, text.atlas_mut(), c, at, WHITE), "{c}");
    (quads, text)
}

#[test]
fn box_corners_point_into_the_box() {
    assert_eq!(box_edges('┌'), Some([0, 1, 1, 0]));
    assert_eq!(box_edges('┘'), Some([1, 0, 0, 1]));
    assert_eq!(box_edges('┼'), Some([1, 1, 1, 1]));
    assert_eq!(box_edges('━'), Some([0, 2, 0, 2]));
    assert_eq!(box_edges('a'), None);
}

#[test]
fn every_shape_the_ui_uses_is_drawn_and_letters_are_left_to_the_font() {
    let mut text = TextSystem::new();
    let mut quads = Vec::new();
    let shapes = "─━│┃┌┐└┘┏┓┗┛├┤┬┴┼╭╮╯╰═║╔╗╚╝▀▄█▌▐░▒▓▖▟⠿⣿🗀🗁☁\
                  ✓✔✗✘✕✖▸►▶◂◄◀▾▼▴▲◦◐◑◒◓◇◆◈■□▪▫▰▱⊘‖⏱⬢☐☒▣";
    for c in shapes.chars() {
        quads.clear();
        assert!(
            draw(&mut quads, text.atlas_mut(), c, cell(16.8, 36.0), WHITE),
            "{c} falls back to the font"
        );
        assert!(!quads.is_empty(), "{c} draws nothing");
    }
    for c in "aZ9 é→".chars() {
        assert!(!draw(
            &mut quads,
            text.atlas_mut(),
            c,
            cell(0.0, 0.0),
            WHITE
        ));
    }
    quads.clear();
    assert!(draw(
        &mut quads,
        text.atlas_mut(),
        '\u{2800}',
        cell(0.0, 0.0),
        WHITE
    ));
    assert!(quads.is_empty(), "blank braille has no dots");
}

#[test]
fn the_cloud_is_an_outline_hollow_inside() {
    // At 8.4 × 18 a grid unit is 0.714 px and the cloud is centered on
    // the right edge of the cell, (25.2, 45).
    let (quads, text) = drawn('☁', cell(16.8, 36.0));
    let at = |x: i32, y: i32| coverage(&quads, text.atlas(), x, y);
    assert_eq!(at(23, 46), 0, "the inside is left empty");
    assert!(at(25, 49).max(at(25, 50)) > 128, "the base is stroked");
    assert!(at(17, 45).max(at(18, 45)) > 128, "the left bump is stroked");
}

#[test]
fn a_light_line_is_one_pixel_through_the_middle_joined_to_its_neighbors() {
    let (quads, text) = drawn('─', cell(8.4, 18.0));
    assert!(quads.iter().all(|q| q.fill == Fill::Solid));
    let row = |y: i32| -> Vec<i32> {
        (0..30)
            .filter(|x| coverage(&quads, text.atlas(), *x, y) > 0)
            .collect()
    };
    // From half a pixel before the cell (7.9) to half a pixel after
    // (17.3): the pixels whose centers are in between.
    assert_eq!(row(27), (8..17).collect::<Vec<_>>());
    assert!(row(26).is_empty() && row(28).is_empty());

    let (corner, text) = drawn('┌', cell(0.0, 0.0));
    assert_eq!(coverage(&corner, text.atlas(), 4, 9), 255, "the bend");
    assert_eq!(coverage(&corner, text.atlas(), 8, 9), 255, "right arm");
    assert_eq!(coverage(&corner, text.atlas(), 4, 17), 255, "down arm");
    assert_eq!(coverage(&corner, text.atlas(), 0, 9), 0, "no left arm");
    assert_eq!(coverage(&corner, text.atlas(), 4, 0), 0, "no up arm");
}

#[test]
fn every_octant_is_drawn_as_its_pattern() {
    for (bits, &c) in OCTANTS.iter().enumerate() {
        let mut text = TextSystem::new();
        let mut quads = Vec::new();
        assert!(
            c == ' ' || draw(&mut quads, text.atlas_mut(), c, cell(0.0, 0.0), WHITE),
            "{c} falls back to the font"
        );
        // A pixel inside each eighth of the 8.4×18 cell.
        for i in 0..8 {
            let (x, y) = (if i % 2 == 0 { 2 } else { 6 }, 2 + 4 * (i / 2) + i / 4);
            let on = bits & (1 << i) != 0;
            assert_eq!(
                coverage(&quads, text.atlas(), x, y) > 0,
                on,
                "{c} ({bits:08b}) eighth {i}"
            );
        }
    }
}

#[test]
fn shades_are_the_color_made_fainter() {
    let (quads, _) = drawn('▒', cell(0.0, 0.0));
    assert_eq!(quads.len(), 1);
    assert_eq!(quads[0].color, [255, 255, 255, 128]);
    assert_eq!((quads[0].w, quads[0].h), (8, 18));
    let (full, _) = drawn('█', cell(0.0, 0.0));
    assert_eq!((full[0].x, full[0].w, full[0].h), (0, 9, 18));
}

#[test]
fn a_rounded_corner_meets_the_lines_beside_and_below_it() {
    let (quads, text) = drawn('╭', cell(0.0, 0.0));
    assert_eq!(quads.len(), 1);
    assert_eq!(quads[0].fill, Fill::Mask);
    let at = |x: i32, y: i32| coverage(&quads, text.atlas(), x, y);
    assert!(at(8, 9) > 128, "where `─` continues: {}", at(8, 9));
    assert_eq!(at(4, 17), 255, "where `│` continues");
    assert_eq!(at(4, 9), 0, "the corner itself is cut");
    assert!(at(5, 10) > 0, "the curve passes inside it");
    assert_eq!(at(1, 9), 0);
    assert_eq!(at(4, 2), 0);
}

#[test]
fn a_symbol_is_drawn_once_for_each_place_in_a_pixel() {
    let mut text = TextSystem::new();
    let mut quads = Vec::new();
    for x in [0.0, 42.0, 84.0, 8.4] {
        assert!(draw(
            &mut quads,
            text.atlas_mut(),
            '✓',
            cell(x, 54.0),
            WHITE
        ));
    }
    let [first, fifth, tenth, second] = quads[..] else {
        panic!("one quad each: {quads:?}");
    };
    // Columns 0, 5 and 10 start on whole pixels; column 1 doesn't.
    assert_eq!((first.u, first.v), (fifth.u, fifth.v));
    assert_eq!((first.u, first.v), (tenth.u, tenth.v));
    assert_eq!((fifth.x - first.x, tenth.x - first.x), (42, 84));
    assert_ne!((first.u, first.v), (second.u, second.v));
    assert!(first.y >= 54 && first.y + first.h as i32 <= 72, "{first:?}");
    assert!(first.x >= 0 && first.x + first.w as i32 <= 9, "{first:?}");

    let mut ink = 0_u32;
    for y in first.y..first.y + first.h as i32 {
        for x in first.x..first.x + first.w as i32 {
            ink += u32::from(coverage(&quads[..1], text.atlas(), x, y));
        }
    }
    assert!(ink > 255 * 6, "a check mark's worth of ink: {ink}");
}

#[test]
fn arcs_turn_corners_on_the_circle_touching_both_legs() {
    let mut outline = Outline::new();
    outline.move_to(Point::new(0.0, 10.0));
    outline.arc_to(Point::new(0.0, 0.0), Point::new(10.0, 0.0), 4.0);
    let end = outline.at.expect("an arc");
    assert!((end.x - 4.0).abs() < 1e-3 && end.y.abs() < 1e-3, "{end:?}");
    let path = outline.path.finish().expect("a path");
    // Around (4, 4): the bounds stop at the legs.
    let bounds = path.bounds();
    assert!(bounds.left().abs() < 1e-3 && bounds.top().abs() < 1e-3);
    assert!((bounds.bottom() - 10.0).abs() < 1e-3);
}
