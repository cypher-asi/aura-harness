use super::*;

const RED: [u8; 4] = [255, 0, 0, 255];

#[test]
fn a_quad_is_cut_to_the_clip_and_its_atlas_pixels_move_with_it() {
    let solid = Quad {
        x: 10,
        y: 20,
        w: 30,
        h: 10,
        color: RED,
        fill: Fill::Solid,
        u: 0,
        v: 0,
    };
    let part = clipped(&solid, [15, 0, 100, 25]).expect("overlaps");
    assert_eq!((part.x, part.y, part.w, part.h), (15, 20, 25, 5));
    assert_eq!((part.u, part.v), (0, 0));
    assert!(
        clipped(&solid, [40, 0, 100, 100]).is_none(),
        "touching only"
    );

    let glyph = Quad {
        fill: Fill::Mask,
        u: 100,
        v: 200,
        ..solid
    };
    let part = clipped(&glyph, [12, 23, 30, 100]).expect("overlaps");
    assert_eq!((part.x, part.y, part.w, part.h), (12, 23, 18, 7));
    assert_eq!((part.u, part.v), (102, 203));
}

#[test]
fn text_that_does_not_fit_is_cut_short() {
    assert_eq!(fit("sonnet", 6), "sonnet");
    assert_eq!(fit("task coder · write greet()", 10), "task code…");
    assert_eq!(fit("日本語", 4), "日…");
    assert_eq!(fit("abc", 0), "");
}
