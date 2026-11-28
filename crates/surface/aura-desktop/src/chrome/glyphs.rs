//! The glyphs the title bars draw themselves, as quads: the icons of the
//! tabs drawn as icons, the hamburger and the expand box's corners. Each
//! is laid out from its box's edges so it sits in the middle of it.

use crate::geom::{Color, Point, Rectangle};
use crate::scene::{fill, Quad};

/// A monitor on a stand centered in `rect`: a screen `2 * arm + line`
/// pixels wide framed in strokes `line` pixels thick, a neck under its
/// middle and a base under that. Laid out from the box's edges like
/// [`hamburger`], its width and height take the box's parity, so its
/// margins match across and down.
pub(super) fn computer(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let mut screen = (width * 0.7).round().max(3.0 * line);
    let neck = (arm * 0.3).round().max(line);
    if (rect.height - (screen + neck + line)).rem_euclid(2.0) != 0.0 {
        screen -= 1.0;
    }
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - (screen + neck + line)) / 2.0).floor();
    fill(out, left, top, width, line, ink);
    fill(out, left, top + screen - line, width, line, ink);
    fill(out, left, top, line, screen, ink);
    fill(out, left + width - line, top, line, screen, ink);
    // Stand pieces as wide as asked, or a pixel wider to center them.
    let centered = |wanted: f32| wanted + (width - wanted).rem_euclid(2.0);
    let (stem, base) = (centered(line), centered((width * 0.6).round()));
    let (neck_y, base_y) = (top + screen, top + screen + neck);
    fill(out, left + (width - stem) / 2.0, neck_y, stem, neck, ink);
    fill(out, left + (width - base) / 2.0, base_y, base, line, ink);
}

/// An open-end wrench standing up in the middle of `rect`: two jaws
/// with the gap the nut goes in between them, the head they rise from,
/// and a handle under its middle. As wide as [`computer`]'s screen,
/// `2 * arm + line` pixels, and a third taller; its width and height take
/// the box's parity, so its margins match across and down.
pub(super) fn wrench(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let mut height = (width * 4.0 / 3.0).round();
    if (rect.height - height).rem_euclid(2.0) != 0.0 {
        height -= 1.0;
    }
    // The gap between the jaws and the handle are as wide, or a pixel
    // wider, so both sit in the middle of the head.
    let centered = |wanted: f32| wanted + (width - wanted).rem_euclid(2.0);
    let gap = centered((width / 3.0).round().max(line));
    let handle = centered((width / 3.0).round().max(line));
    let jaw = (height * 0.3).round().max(2.0 * line);
    let head = (height * 0.2).round().max(2.0 * line);
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - height) / 2.0).floor();
    let prong = (width - gap) / 2.0;
    fill(out, left, top, prong, jaw, ink);
    fill(out, left + width - prong, top, prong, jaw, ink);
    fill(out, left, top + jaw, width, head, ink);
    let neck = top + jaw + head;
    let x = left + (width - handle) / 2.0;
    fill(out, x, neck, handle, top + height - neck, ink);
}

/// A plug standing up in the middle of `rect`: two prongs over a body
/// as wide as [`wrench`]'s head, and a cord under its middle. As wide as
/// [`computer`]'s screen, `2 * arm + line` pixels, and a third taller;
/// its width and height take the box's parity, so its margins match.
pub(super) fn plug(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let mut height = (width * 4.0 / 3.0).round();
    if (rect.height - height).rem_euclid(2.0) != 0.0 {
        height -= 1.0;
    }
    let centered = |wanted: f32| wanted + (width - wanted).rem_euclid(2.0);
    let cord = centered(line);
    let prong = line.max(1.0);
    let prongs = (height * 0.3).round().max(2.0 * line);
    let body = (height * 0.4).round().max(3.0 * line);
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - height) / 2.0).floor();
    // The prongs a third of the way in from each side.
    let inset = (width / 4.0).round();
    fill(out, left + inset, top, prong, prongs, ink);
    fill(out, left + width - inset - prong, top, prong, prongs, ink);
    fill(out, left, top + prongs, width, body, ink);
    let neck = top + prongs + body;
    fill(
        out,
        left + (width - cord) / 2.0,
        neck,
        cord,
        top + height - neck,
        ink,
    );
}

/// Three bars centered in `rect`, about `arm` pixels each way from the
/// middle, strokes `line` pixels thick. The glyph is laid out from the
/// box's edges rather than from a rounded middle, which lands half a
/// pixel off and then rounds a whole pixel right and down in a box of
/// odd size; its width takes the box's parity so the margins match.
pub(super) fn hamburger(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let step = (arm * 0.7).round().max(2.0 * line);
    let height = 2.0 * step + line;
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - height) / 2.0).floor();
    for offset in [0.0, step, 2.0 * step] {
        fill(out, left, top + offset, width, line, ink);
    }
}

/// Four corner brackets around `middle`, `reach` pixels out, strokes
/// `line` pixels thick: at the corners and opening inward to expand, or
/// near the middle and opening outward to restore.
pub(super) fn corners(
    out: &mut Vec<Quad>,
    middle: Point,
    size: (f32, f32),
    restore: bool,
    ink: Color,
) {
    let (reach, line) = size;
    let gap = (reach * 0.3).round().max(line);
    let (from, arm) = if restore {
        (gap, reach - gap)
    } else {
        (reach, (reach * 0.7).round())
    };
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        // The bracket's corner, and the way its arms run from it.
        let corner = Point::new(middle.x + sx * from, middle.y + sy * from);
        let (dx, dy): (f32, f32) = if restore { (sx, sy) } else { (-sx, -sy) };
        let span = |at: f32, dir: f32, len: f32| {
            if dir > 0.0 {
                (at, len)
            } else {
                (at - len, len)
            }
        };
        let (x, w) = span(corner.x, dx, arm);
        let (y, h) = span(corner.y, dy, line);
        fill(out, x, y, w, h, ink);
        let (x, w) = span(corner.x, dx, line);
        let (y, h) = span(corner.y, dy, arm);
        fill(out, x, y, w, h, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Size;

    fn ink() -> Color {
        Color::from_rgb8(255, 255, 255)
    }

    /// Pixels between the glyph `draw` puts in a box `side` pixels wide
    /// and tall and the box, on the left, right, top and bottom.
    fn margins_of(side: i16, draw: impl Fn(&mut Vec<Quad>, Rectangle)) -> [i64; 4] {
        let size = f32::from(side);
        let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(size, size));
        let mut out = Vec::new();
        draw(&mut out, rect);
        let left = out.iter().map(|q| i64::from(q.x)).min().unwrap();
        let right = out
            .iter()
            .map(|q| i64::from(q.x) + i64::from(q.w))
            .max()
            .unwrap();
        let top = out.iter().map(|q| i64::from(q.y)).min().unwrap();
        let bottom = out
            .iter()
            .map(|q| i64::from(q.y) + i64::from(q.h))
            .max()
            .unwrap();
        let side = i64::from(side);
        [left - 10, 10 + side - right, top - 20, 20 + side - bottom]
    }

    fn margins(side: i16) -> [i64; 4] {
        margins_of(side, |out, rect| {
            hamburger(out, rect, (4.0, 1.0), ink());
            assert_eq!(out.len(), 3, "three bars");
        })
    }

    #[test]
    fn the_computer_sits_in_the_middle_of_its_box_across_and_down() {
        for side in [22, 23, 24, 25] {
            for arm in [4.0, 5.0] {
                let [left, right, top, bottom] = margins_of(side, |out, rect| {
                    computer(out, rect, (arm, 1.0), ink());
                    assert_eq!(out.len(), 6, "the screen's four sides, a neck, a base");
                });
                assert_eq!(left, right, "centered across a box of {side}");
                assert_eq!(top, bottom, "centered down a box of {side}");
                assert!(left > 0 && top > 0, "inside a box of {side}");
            }
        }
    }

    #[test]
    fn the_computers_stand_is_centered_under_its_screen() {
        for side in [23_u8, 24] {
            let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(f32::from(side), 23.0));
            let mut out = Vec::new();
            computer(&mut out, rect, (5.0, 1.0), ink());
            let middle = |q: &Quad| 2 * i64::from(q.x) + i64::from(q.w);
            let screen = middle(&out[0]);
            for piece in &out[4..] {
                assert_eq!(middle(piece), screen, "{piece:?} under {:?}", out[0]);
            }
        }
    }

    #[test]
    fn the_wrench_sits_in_the_middle_of_its_box_across_and_down() {
        for side in [22, 23, 24, 25] {
            for arm in [4.0, 5.0] {
                let [left, right, top, bottom] = margins_of(side, |out, rect| {
                    wrench(out, rect, (arm, 1.0), ink());
                    assert_eq!(out.len(), 4, "two jaws, the head, the handle");
                });
                assert_eq!(left, right, "centered across a box of {side}");
                assert_eq!(top, bottom, "centered down a box of {side}");
                assert!(left > 0 && top > 0, "inside a box of {side}");
            }
        }
    }

    #[test]
    fn the_plug_sits_in_the_middle_of_its_box_across_and_down() {
        for side in [22, 23, 24, 25] {
            for arm in [4.0, 5.0] {
                let [left, right, top, bottom] = margins_of(side, |out, rect| {
                    plug(out, rect, (arm, 1.0), ink());
                    assert_eq!(out.len(), 4, "two prongs, the body, the cord");
                });
                assert_eq!(left, right, "centered across a box of {side}");
                assert_eq!(top, bottom, "centered down a box of {side}");
                assert!(left > 0 && top > 0, "inside a box of {side}");
            }
        }
    }

    #[test]
    fn the_plugs_prongs_stand_apart_over_its_cord() {
        for side in [23_u8, 24] {
            let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(f32::from(side), 23.0));
            let mut out = Vec::new();
            plug(&mut out, rect, (5.0, 1.0), ink());
            let [prong, other, body, cord] = [out[0], out[1], out[2], out[3]];
            assert_eq!((prong.w, prong.h), (other.w, other.h), "prongs alike");
            assert!(other.x > prong.right(), "a gap between the prongs");
            let middle = |q: &Quad| 2 * i64::from(q.x) + i64::from(q.w);
            assert_eq!(middle(&cord), middle(&body), "cord under the body's middle");
            assert_eq!(
                i64::from(prong.x) - i64::from(body.x),
                i64::from(body.right()) - i64::from(other.right()),
                "prongs as far in from each side"
            );
            assert!(
                prong.y < body.y && cord.y > body.y,
                "prongs on top, cord below"
            );
        }
    }

    #[test]
    fn the_wrenchs_jaws_open_over_its_handle() {
        for side in [23_u8, 24] {
            let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(f32::from(side), 23.0));
            let mut out = Vec::new();
            wrench(&mut out, rect, (5.0, 1.0), ink());
            let [jaw, other, head, handle] = [out[0], out[1], out[2], out[3]];
            assert_eq!(jaw.w, other.w, "jaws alike");
            assert_eq!(jaw.h, other.h, "jaws alike");
            assert!(other.x > jaw.right(), "a gap between the jaws");
            let middle = |q: &Quad| 2 * i64::from(q.x) + i64::from(q.w);
            assert_eq!(
                middle(&handle),
                middle(&head),
                "handle under the head's middle"
            );
            let gap = other.x - jaw.right();
            assert_eq!(
                i64::from(gap),
                i64::from(handle.w),
                "the gap is the handle's width"
            );
            assert!(handle.w < head.w, "the handle is narrower than the head");
            assert_eq!(head.y, jaw.bottom());
            assert_eq!(handle.y, head.bottom());
        }
    }

    #[test]
    fn the_hamburger_sits_in_the_middle_of_an_odd_box() {
        let [left, right, top, bottom] = margins(23);
        assert_eq!(left, right, "centered across");
        assert_eq!(top, bottom, "centered down");
    }

    #[test]
    fn the_hamburger_sits_in_the_middle_of_an_even_box_across() {
        let [left, right, top, bottom] = margins(24);
        assert_eq!(left, right, "centered across");
        // Three equal bars with equal gaps have an odd height, so an even
        // box leaves one pixel more on one side.
        assert!(top.abs_diff(bottom) <= 1, "{top} {bottom}");
    }
}
