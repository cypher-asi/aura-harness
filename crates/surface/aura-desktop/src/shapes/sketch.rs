//! Shapes with curves or slants, drawn with a [`Pen`] into the atlas.

use super::outline::Outline;
use super::pen::Pen;
use super::{center, light, LineCap, LineJoin};
use crate::geom::{Point, Rectangle};
use std::f32::consts::PI;

pub(super) fn sketch(pen: &mut Pen, c: char, cell: Rectangle) {
    match c {
        '╭' | '╮' | '╯' | '╰' => rounded_corner(pen, c, cell),
        '\u{2800}'..='\u{28FF}' => braille(pen, c, cell),
        '🗀' | '🗁' => folder(pen, c == '🗁', cell),
        '☁' => cloud(pen, cell),
        _ => symbol(pen, c, cell),
    }
}

fn rounded_corner(pen: &mut Pen, c: char, cell: Rectangle) {
    let t = light(cell);
    let (sx, sy) = center(cell, t);
    let (cx, cy) = (sx + t / 2.0, sy + t / 2.0);
    let r = (cell.width.min(cell.height) / 2.0).max(1.0);
    let (left, right) = (cell.x - 0.5, cell.x + cell.width + 0.5);
    let (top, bottom) = (cell.y, cell.y + cell.height);
    let corner = Point::new(cx, cy);
    let (start, end) = match c {
        '╭' => (Point::new(cx, bottom), Point::new(right, cy)),
        '╮' => (Point::new(left, cy), Point::new(cx, bottom)),
        '╯' => (Point::new(left, cy), Point::new(cx, top)),
        _ => (Point::new(cx, top), Point::new(right, cy)),
    };
    let mut path = Outline::new();
    path.move_to(start);
    path.arc_to(corner, end, r);
    path.line_to(end);
    pen.stroke(path, t, LineCap::Butt, LineJoin::Miter);
}

fn braille(pen: &mut Pen, c: char, cell: Rectangle) {
    // Dot n of the pattern is bit n-1; dots 1-3 and 7 form the left column.
    const DOTS: [(f32, f32); 8] = [
        (0.0, 0.0),
        (0.0, 1.0),
        (0.0, 2.0),
        (1.0, 0.0),
        (1.0, 1.0),
        (1.0, 2.0),
        (0.0, 3.0),
        (1.0, 3.0),
    ];
    let bits = c as u32 - 0x2800;
    let radius = (cell.width * 0.12).max(0.8);
    for (bit, (col, row)) in DOTS.into_iter().enumerate() {
        if bits & (1 << bit) != 0 {
            let at = Point::new(
                cell.x + cell.width * (0.28 + 0.44 * col),
                cell.y + cell.height * (0.125 + 0.25 * row),
            );
            pen.fill(Outline::circle(at, radius));
        }
    }
}

/// An outline folder, open or closed, on a 24-unit grid. A folder one cell
/// wide reads as a sliver, so it spans this cell and the next, which must
/// be blank: the projects list's two-cell icon column.
fn folder(pen: &mut Pen, open: bool, cell: Rectangle) {
    let s = (cell.width * 1.7 / 20.0).min(cell.height * 0.66 / 17.0);
    let (cx, cy) = (cell.x + cell.width, cell.y + cell.height / 2.0);
    let q = |x: f32, y: f32| Point::new(cx + (x - 12.0) * s, cy + (y - 11.5) * s);
    let mut b = Outline::new();
    if open {
        b.move_to(q(6.0, 14.0));
        b.arc_to(q(8.07, 10.0), q(22.58, 10.0), 2.0 * s);
        b.arc_to(q(22.58, 10.0), q(20.0, 20.0), 2.0 * s);
        b.arc_to(q(20.0, 20.0), q(2.0, 20.0), 2.0 * s);
    } else {
        b.move_to(q(12.0, 20.0));
    }
    b.arc_to(q(2.0, 20.0), q(2.0, 3.0), 2.0 * s);
    b.arc_to(q(2.0, 3.0), q(9.0, 3.0), 2.0 * s);
    b.arc_to(q(9.0, 3.0), q(11.0, 6.0), s);
    if open {
        b.arc_to(q(11.0, 6.0), q(20.0, 6.0), s);
        b.arc_to(q(20.0, 6.0), q(20.0, 10.0), 2.0 * s);
        b.line_to(q(20.0, 10.0));
    } else {
        b.arc_to(q(11.0, 6.0), q(22.0, 6.0), s);
        b.arc_to(q(22.0, 6.0), q(22.0, 20.0), 2.0 * s);
        b.arc_to(q(22.0, 20.0), q(12.0, 20.0), 2.0 * s);
        b.close();
    }
    pen.stroke(b, (1.8 * s).max(1.0), LineCap::Round, LineJoin::Round);
}

/// An outline cloud on a 24-unit grid, stroked like the folders and hollow
/// inside: a big round bump on the left, a smaller one on the right and a
/// flat base. One cell wide it is a speck, so like the folders it spans
/// this cell and the next, which must be blank: the terminal UI's cloud
/// mark leaves it so.
fn cloud(pen: &mut Pen, cell: Rectangle) {
    let s = (cell.width * 1.7 / 20.0).min(cell.height * 0.66 / 14.0);
    let (cx, cy) = (cell.x + cell.width, cell.y + cell.height / 2.0);
    let q = |x: f32, y: f32| Point::new(cx + (x - 12.0) * s, cy + (y - 12.0) * s);
    // The big bump, around (9, 12), runs from the base up over the top to
    // where it meets the small bump's top at y = 10.
    let meets = 2.0 * PI - (2.0_f32 / 7.0).asin();
    let mut cloud = Outline::new();
    cloud.move_to(q(17.5, 19.0));
    cloud.line_to(q(9.0, 19.0));
    cloud.arc_on(q(9.0, 12.0), 7.0 * s, PI / 2.0, meets);
    cloud.line_to(q(17.5, 10.0));
    cloud.arc_on(q(17.5, 14.5), 4.5 * s, -PI / 2.0, PI / 2.0);
    cloud.close();
    pen.stroke(cloud, (1.8 * s).max(1.0), LineCap::Round, LineJoin::Round);
}

/// Where a symbol's points are: offsets from the middle of the cell, in
/// cell widths.
#[derive(Clone, Copy)]
struct Grid {
    cx: f32,
    cy: f32,
    w: f32,
}

impl Grid {
    #[inline]
    fn p(self, dx: f32, dy: f32) -> Point {
        Point::new(self.cx + dx * self.w, self.cy + dy * self.w)
    }

    /// The square `s` from the middle each way.
    fn square(self, s: f32) -> Outline {
        Outline::polygon(&[self.p(-s, -s), self.p(s, -s), self.p(s, s), self.p(-s, s)])
    }

    /// The diamond `s` from the middle each way.
    fn diamond(self, s: f32) -> Outline {
        Outline::polygon(&[
            self.p(0.0, -s),
            self.p(s, 0.0),
            self.p(0.0, s),
            self.p(-s, 0.0),
        ])
    }

    /// The cross whose arms run `s` from the middle each way.
    fn cross(self, s: f32) -> Outline {
        let mut cross = Outline::new();
        cross.move_to(self.p(-s, -s));
        cross.line_to(self.p(s, s));
        cross.move_to(self.p(s, -s));
        cross.line_to(self.p(-s, s));
        cross
    }
}

fn stroke(pen: &mut Pen, path: Outline, width: f32) {
    pen.stroke(path, width, LineCap::Round, LineJoin::Round);
}

fn symbol(pen: &mut Pen, c: char, cell: Rectangle) {
    let w = cell.width;
    let g = Grid {
        cx: cell.x + w / 2.0,
        cy: cell.y + cell.height / 2.0,
        w,
    };
    let thin = (w * 0.11).max(1.0);
    match c {
        '✓' | '✔' | '✗' | '✘' | '✕' | '✖' => mark(pen, c, g, thin),
        '▸' | '►' | '▶' | '◂' | '◄' | '◀' | '▾' | '▼' | '▴' | '▲' => {
            triangle(pen, c, g);
        }
        '◦' | '◐' | '◑' | '◒' | '◓' | '⊘' | '⏱' => round(pen, c, g, thin),
        '○' | '◔' | '◕' | '●' => ring(pen, c, g, thin),
        '◇' | '◆' | '◈' | '■' | '□' | '▪' | '▫' => diamond_or_square(pen, c, g, thin),
        '▰' | '▱' => progress(pen, c, cell, g, thin),
        '☐' | '☒' | '▣' => to_do(pen, c, g, thin),
        '⬢' => hexagon(pen, g),
        _ => {}
    }
}

/// Checks and crosses.
fn mark(pen: &mut Pen, c: char, g: Grid, thin: f32) {
    if matches!(c, '✓' | '✔') {
        let mut check = Outline::new();
        check.move_to(g.p(-0.36, 0.02));
        check.line_to(g.p(-0.1, 0.28));
        check.line_to(g.p(0.38, -0.3));
        stroke(pen, check, if c == '✔' { thin * 1.8 } else { thin * 1.4 });
        return;
    }
    let width = if matches!(c, '✘' | '✖') {
        thin * 1.8
    } else {
        thin * 1.4
    };
    stroke(pen, g.cross(0.3), width);
}

/// Filled triangles pointing right, left, down or up.
fn triangle(pen: &mut Pen, c: char, g: Grid) {
    let p = |dx: f32, dy: f32| g.p(dx, dy);
    let corners = match c {
        '▸' | '►' => [p(-0.22, -0.3), p(0.3, 0.0), p(-0.22, 0.3)],
        '▶' => [p(-0.32, -0.42), p(0.4, 0.0), p(-0.32, 0.42)],
        '◂' | '◄' => [p(0.22, -0.3), p(-0.3, 0.0), p(0.22, 0.3)],
        '◀' => [p(0.32, -0.42), p(-0.4, 0.0), p(0.32, 0.42)],
        '▾' => [p(-0.3, -0.18), p(0.3, -0.18), p(0.0, 0.3)],
        '▼' => [p(-0.42, -0.3), p(0.42, -0.3), p(0.0, 0.42)],
        '▴' => [p(-0.3, 0.18), p(0.3, 0.18), p(0.0, -0.3)],
        _ => [p(-0.42, 0.3), p(0.42, 0.3), p(0.0, -0.42)],
    };
    pen.fill(Outline::polygon(&corners));
}

/// Circles: the bullet, the half-filled ones, "none" and the stopwatch.
fn round(pen: &mut Pen, c: char, g: Grid, thin: f32) {
    let w = g.w;
    match c {
        '◦' => stroke(pen, Outline::circle(g.p(0.0, 0.0), w * 0.17), thin),
        '◐' | '◑' | '◒' | '◓' => {
            let r = w * 0.38;
            stroke(pen, Outline::circle(g.p(0.0, 0.0), r), thin);
            // The filled half: left, right, bottom or top.
            let start = match c {
                '◐' => PI / 2.0,
                '◑' => -PI / 2.0,
                '◒' => 0.0,
                _ => PI,
            };
            let mut half = Outline::new();
            half.arc(g.p(0.0, 0.0), r, start, start + PI);
            half.close();
            pen.fill(half);
        }
        '⊘' => {
            stroke(pen, Outline::circle(g.p(0.0, 0.0), w * 0.38), thin);
            stroke(pen, Outline::line(g.p(-0.27, 0.27), g.p(0.27, -0.27)), thin);
        }
        _ => {
            let face = g.p(0.0, 0.06);
            stroke(pen, Outline::circle(face, w * 0.36), thin);
            stroke(
                pen,
                Outline::line(g.p(0.0, -0.42), g.p(0.0, -0.36)),
                thin * 1.4,
            );
            stroke(pen, Outline::line(face, g.p(0.14, -0.1)), thin);
        }
    }
}

/// The context ring: a circle filled clockwise from the top by a
/// quarter, three quarters, or wholly, as `◑` is by half.
fn ring(pen: &mut Pen, c: char, g: Grid, thin: f32) {
    let r = g.w * 0.38;
    let center = g.p(0.0, 0.0);
    stroke(pen, Outline::circle(center, r), thin);
    let turn = match c {
        '◔' => 0.25,
        '◕' => 0.75,
        '●' => 1.0,
        _ => return,
    };
    if c == '●' {
        pen.fill(Outline::circle(center, r));
        return;
    }
    let top = -PI / 2.0;
    let mut slice = Outline::new();
    slice.move_to(center);
    slice.arc_on(center, r, top, top + turn * 2.0 * PI);
    slice.close();
    pen.fill(slice);
}

/// Diamonds and squares, filled or outlined.
fn diamond_or_square(pen: &mut Pen, c: char, g: Grid, thin: f32) {
    if matches!(c, '◇' | '◆' | '◈') {
        let diamond = g.diamond(0.4);
        if c == '◆' {
            pen.fill(diamond);
        } else {
            stroke(pen, diamond, thin);
        }
        if c == '◈' {
            pen.fill(g.diamond(0.17));
        }
        return;
    }
    let s = if matches!(c, '■' | '□') {
        0.36
    } else {
        0.22
    };
    let square = g.square(s);
    if matches!(c, '■' | '▪') {
        pen.fill(square);
    } else {
        stroke(pen, square, thin);
    }
}

/// A slanted bar of a progress meter, filled or outlined.
fn progress(pen: &mut Pen, c: char, cell: Rectangle, g: Grid, thin: f32) {
    let w = g.w;
    let (dx, h) = (w * 0.12, (cell.height * 0.24).round());
    let (x0, x1) = (cell.x + 1.0, cell.x + w - 1.0);
    let (y0, y1) = (g.cy - h, g.cy + h);
    let shape = Outline::polygon(&[
        Point::new(x0 + dx, y0),
        Point::new(x1, y0),
        Point::new(x1 - dx, y1),
        Point::new(x0, y1),
    ]);
    if c == '▰' {
        pen.fill(shape);
    } else {
        stroke(pen, shape, thin);
    }
}

/// To-do boxes: empty, crossed or filled.
fn to_do(pen: &mut Pen, c: char, g: Grid, thin: f32) {
    // Wider than the cell's other squares: the to-do boxes are read
    // at a glance, and the space after them leaves room to overhang.
    stroke(pen, g.square(0.5), thin);
    if c == '☒' {
        stroke(pen, g.cross(0.28), thin);
    } else if c == '▣' {
        pen.fill(g.square(0.26));
    }
}

/// A filled hexagon, point up.
fn hexagon(pen: &mut Pen, g: Grid) {
    let r = 0.42;
    let corners: Vec<Point> = (0..6u8)
        .map(|i| {
            let angle = PI / 3.0 * f32::from(i) - PI / 2.0;
            g.p(r * angle.cos(), r * angle.sin())
        })
        .collect();
    pen.fill(Outline::polygon(&corners));
}
