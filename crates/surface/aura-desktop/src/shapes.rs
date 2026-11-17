//! Characters drawn as shapes instead of font glyphs, as quads: box
//! drawing, block elements, braille and the status symbols the UI uses.
//!
//! The same shapes as [`crate::glyphs`], which draws them on an iced
//! canvas. Lines and blocks are rectangles, so they become solid quads.
//! Anything with a curve or a slant is drawn once, with tiny-skia, into
//! the atlas and stamped from there like a glyph: once per cell size and
//! per place a cell starts within a pixel, since both change which
//! pixels a curve covers.

// Pixel math on cell sizes and small counts.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use crate::geom::{Point, Rectangle};
use crate::scene::{Fill, Quad};
use crate::text::{Atlas, Bitmap, Pixels, ShapeKey};
use std::f32::consts::PI;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub use tiny_skia::{LineCap, LineJoin};

/// Draw `c` into `cell`; `false` when it isn't one of the built-in shapes
/// and should come from the font.
pub fn draw(
    out: &mut Vec<Quad>,
    atlas: &mut Atlas,
    c: char,
    cell: Rectangle,
    color: [u8; 4],
) -> bool {
    if let Some(edges) = box_edges(c) {
        lines(out, cell, edges, color);
        return true;
    }
    match c {
        '═' | '║' | '╔' | '╗' | '╚' | '╝' => double(out, c, cell, color),
        '\u{2580}'..='\u{259F}' => block(out, c, cell, color),
        '‖' => bars(out, cell, color),
        _ if is_sketched(c) => stamp_char(out, atlas, c, cell, color),
        _ => return false,
    }
    true
}

/// Stroke width of a light line at this cell size.
fn light(cell: Rectangle) -> f32 {
    (cell.width / 8.0).round().max(1.0)
}

/// Weights of the four arms (up, right, down, left): 0 none, 1 light, 2 heavy.
fn box_edges(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' | '┄' | '┈' | '╌' => [0, 1, 0, 1],
        '━' | '┅' | '┉' | '╍' => [0, 2, 0, 2],
        '│' | '┆' | '┊' | '╎' => [1, 0, 1, 0],
        '┃' | '┇' | '┋' | '╏' => [2, 0, 2, 0],
        '┌' => [0, 1, 1, 0],
        '┍' => [0, 2, 1, 0],
        '┎' => [0, 1, 2, 0],
        '┏' => [0, 2, 2, 0],
        '┐' => [0, 0, 1, 1],
        '┑' => [0, 0, 1, 2],
        '┒' => [0, 0, 2, 1],
        '┓' => [0, 0, 2, 2],
        '└' => [1, 1, 0, 0],
        '┕' => [1, 2, 0, 0],
        '┖' => [2, 1, 0, 0],
        '┗' => [2, 2, 0, 0],
        '┘' => [1, 0, 0, 1],
        '┙' => [1, 0, 0, 2],
        '┚' => [2, 0, 0, 1],
        '┛' => [2, 0, 0, 2],
        '├' => [1, 1, 1, 0],
        '┝' => [1, 2, 1, 0],
        '┠' => [2, 1, 2, 0],
        '┣' => [2, 2, 2, 0],
        '┤' => [1, 0, 1, 1],
        '┥' => [1, 0, 1, 2],
        '┨' => [2, 0, 2, 1],
        '┫' => [2, 0, 2, 2],
        '┬' => [0, 1, 1, 1],
        '┯' => [0, 2, 1, 2],
        '┰' => [0, 1, 2, 1],
        '┳' => [0, 2, 2, 2],
        '┴' => [1, 1, 0, 1],
        '┷' => [1, 2, 0, 2],
        '┸' => [2, 1, 0, 1],
        '┻' => [2, 2, 0, 2],
        '┼' => [1, 1, 1, 1],
        '┿' => [1, 2, 1, 2],
        '╂' => [2, 1, 2, 1],
        '╋' => [2, 2, 2, 2],
        '╴' => [0, 0, 0, 1],
        '╵' => [1, 0, 0, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 1, 0],
        '╸' => [0, 0, 0, 2],
        '╹' => [2, 0, 0, 0],
        '╺' => [0, 2, 0, 0],
        '╻' => [0, 0, 2, 0],
        '╼' => [0, 2, 0, 1],
        '╽' => [1, 0, 2, 0],
        '╾' => [0, 1, 0, 2],
        '╿' => [2, 0, 1, 0],
        _ => return None,
    })
}

/// Center of the cell on the pixel grid, where strokes of width `t` start.
fn center(cell: Rectangle, t: f32) -> (f32, f32) {
    let x = (cell.x + cell.width / 2.0 - t / 2.0).round();
    let y = (cell.y + cell.height / 2.0 - t / 2.0).round();
    (x, y)
}

fn rect(out: &mut Vec<Quad>, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
    out.extend(Quad::solid(x0, y0, x1, y1, color));
}

fn lines(out: &mut Vec<Quad>, cell: Rectangle, [up, right, down, left]: [u8; 4], color: [u8; 4]) {
    let t = light(cell);
    let width = |w: u8| f32::from(w) * t;
    // Arms reach half a pixel past the cell so neighbors join without a
    // seam, and run to the far side of the crossing stroke.
    let (x0, x1) = (cell.x - 0.5, cell.x + cell.width + 0.5);
    let (y0, y1) = (cell.y, cell.y + cell.height);
    let tv = width(up.max(down)).max(t);
    let th = width(left.max(right)).max(t);
    let (vx, _) = center(cell, tv);
    let (_, hy) = center(cell, th);
    for (weight, top) in [(up, true), (down, false)] {
        if weight > 0 {
            let w = width(weight);
            let (sx, _) = center(cell, w);
            let (a, b) = if top { (y0, hy + th) } else { (hy, y1) };
            rect(out, sx, a, sx + w, b, color);
        }
    }
    for (weight, west) in [(left, true), (right, false)] {
        if weight > 0 {
            let w = width(weight);
            let (_, sy) = center(cell, w);
            let (a, b) = if west { (x0, vx + tv) } else { (vx, x1) };
            rect(out, a, sy, b, sy + w, color);
        }
    }
}

fn double(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
    let t = light(cell);
    let gap = (cell.width / 5.0).round().max(t + 1.0);
    let (cx, cy) = center(cell, t);
    let (left, right) = (cell.x - 0.5, cell.x + cell.width + 0.5);
    let (top, bottom) = (cell.y, cell.y + cell.height);
    let (outer_x, inner_x) = (cx - gap, cx + gap);
    let (outer_y, inner_y) = (cy - gap, cy + gap);
    let mut h = |x0: f32, x1: f32, y: f32| rect(out, x0, y, x1, y + t, color);
    match c {
        '═' => {
            h(left, right, outer_y);
            h(left, right, inner_y);
        }
        '║' => {
            rect(out, outer_x, top, outer_x + t, bottom, color);
            rect(out, inner_x, top, inner_x + t, bottom, color);
        }
        '╔' => {
            h(outer_x, right, outer_y);
            h(inner_x, right, inner_y);
            rect(out, outer_x, outer_y, outer_x + t, bottom, color);
            rect(out, inner_x, inner_y, inner_x + t, bottom, color);
        }
        '╗' => {
            h(left, inner_x + t, outer_y);
            h(left, outer_x + t, inner_y);
            rect(out, inner_x, outer_y, inner_x + t, bottom, color);
            rect(out, outer_x, inner_y, outer_x + t, bottom, color);
        }
        '╚' => {
            h(outer_x, right, inner_y);
            h(inner_x, right, outer_y);
            rect(out, outer_x, top, outer_x + t, inner_y + t, color);
            rect(out, inner_x, top, inner_x + t, outer_y + t, color);
        }
        _ => {
            h(left, inner_x + t, inner_y);
            h(left, outer_x + t, outer_y);
            rect(out, inner_x, top, inner_x + t, inner_y + t, color);
            rect(out, outer_x, top, outer_x + t, outer_y + t, color);
        }
    }
}

fn block(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
    let Rectangle {
        x,
        y,
        width: w,
        height: h,
    } = cell;
    // Full-width blocks overlap their neighbors by half a pixel so a run
    // of them reads as one bar.
    let (l, r) = (x - 0.5, x + w + 0.5);
    let eighth_h = |n: u32| (h * n as f32 / 8.0).round();
    let eighth_w = |n: u32| w * n as f32 / 8.0;
    match c {
        '▀' => rect(out, l, y, r, y + eighth_h(4), color),
        '▁'..='█' => {
            let n = c as u32 - 0x2580;
            rect(out, l, y + h - eighth_h(n), r, y + h, color);
        }
        '▉'..='▏' => {
            let n = 0x2590 - c as u32;
            rect(out, x, y, x + eighth_w(n), y + h, color);
        }
        '▐' => rect(out, x + w / 2.0, y, r, y + h, color),
        '░' | '▒' | '▓' => {
            let a = match c {
                '░' => 0.25,
                '▒' => 0.5,
                _ => 0.75,
            };
            let [red, green, blue, alpha] = color;
            let alpha = (f32::from(alpha) * a).round() as u8;
            rect(out, x, y, x + w, y + h, [red, green, blue, alpha]);
        }
        '▔' => rect(out, l, y, r, y + eighth_h(1), color),
        '▕' => rect(out, x + w - eighth_w(1), y, x + w, y + h, color),
        _ => {
            // Quadrants: upper-left, upper-right, lower-left, lower-right.
            let quads: [bool; 4] = match c {
                '▖' => [false, false, true, false],
                '▗' => [false, false, false, true],
                '▘' => [true, false, false, false],
                '▙' => [true, false, true, true],
                '▚' => [true, false, false, true],
                '▛' => [true, true, true, false],
                '▜' => [true, true, false, true],
                '▝' => [false, true, false, false],
                '▞' => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let (mx, my) = (x + w / 2.0, y + eighth_h(4));
            let spans = [
                (l, mx, y, my),
                (mx, r, y, my),
                (l, mx, my, y + h),
                (mx, r, my, y + h),
            ];
            for (on, (x0, x1, y0, y1)) in quads.into_iter().zip(spans) {
                if on {
                    rect(out, x0, y0, x1, y1, color);
                }
            }
        }
    }
}

/// `‖`: two upright bars.
fn bars(out: &mut Vec<Quad>, cell: Rectangle, color: [u8; 4]) {
    let t = light(cell);
    let cx = cell.x + cell.width / 2.0;
    for dx in [-0.18, 0.18] {
        let x = (cx + dx * cell.width).round();
        rect(
            out,
            x,
            cell.y + 1.0,
            x + t,
            cell.y + cell.height - 1.0,
            color,
        );
    }
}

/// A path being built: tiny-skia's builder, with the arcs iced's has.
pub struct Outline {
    path: PathBuilder,
}

impl Outline {
    pub fn new() -> Self {
        Self {
            path: PathBuilder::new(),
        }
    }

    pub fn move_to(&mut self, to: Point) {
        self.path.move_to(to.x, to.y);
    }

    pub fn line_to(&mut self, to: Point) {
        self.path.line_to(to.x, to.y);
    }

    pub fn close(&mut self) {
        self.path.close();
    }

    /// A line from `from` to `to`.
    pub fn line(from: Point, to: Point) -> Self {
        let mut outline = Self::new();
        outline.move_to(from);
        outline.line_to(to);
        outline
    }

    /// The closed shape through `points`.
    pub fn polygon(points: &[Point]) -> Self {
        let mut outline = Self::new();
        if let Some((first, rest)) = points.split_first() {
            outline.move_to(*first);
            for point in rest {
                outline.line_to(*point);
            }
            outline.close();
        }
        outline
    }

    pub fn circle(center: Point, radius: f32) -> Self {
        let mut outline = Self::new();
        outline.path.push_circle(center.x, center.y, radius);
        outline
    }

    /// A line towards `a` that turns towards `b` on a circle of `radius`
    /// touching both legs, ending where the circle meets the second.
    pub fn arc_to(&mut self, a: Point, b: Point, radius: f32) {
        let Some(start) = self.path.last_point() else {
            return;
        };
        let start = Point::new(start.x, start.y);
        if start == a || a == b || radius == 0.0 {
            return self.line_to(a);
        }
        let double_area = start.x * (a.y - b.y) + a.x * (b.y - start.y) + b.x * (start.y - a.y);
        if double_area == 0.0 {
            return self.line_to(a);
        }
        let unit = |from: Point, to: Point| {
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            let length = dx.hypot(dy);
            (dx / length, dy / length)
        };
        let to_start = unit(a, start);
        let to_end = unit(a, b);
        let half = (to_start.0 * to_end.0 + to_start.1 * to_end.1).acos() / 2.0;
        let along = radius / half.tan();
        let arc_start = Point::new(a.x + to_start.0 * along, a.y + to_start.1 * along);
        let arc_end = Point::new(a.x + to_end.0 * along, a.y + to_end.1 * along);
        // The circle's center is on the line halving the corner.
        let (bx, by) = (to_start.0 + to_end.0, to_start.1 + to_end.1);
        let reach = radius / half.sin() / bx.hypot(by);
        let center = Point::new(a.x + bx * reach, a.y + by * reach);
        let angle = |at: Point| (at.y - center.y).atan2(at.x - center.x);
        let from = angle(arc_start);
        // The short way round.
        let mut sweep = angle(arc_end) - from;
        if sweep > PI {
            sweep -= 2.0 * PI;
        } else if sweep < -PI {
            sweep += 2.0 * PI;
        }
        self.line_to(arc_start);
        self.sweep(center, radius, from, sweep);
    }

    /// An arc of the circle around `center` starting a new stroke at the
    /// angle `from`, `sweep` radians on.
    pub fn arc(&mut self, center: Point, radius: f32, from: f32, sweep: f32) {
        self.move_to(Point::new(
            center.x + radius * from.cos(),
            center.y + radius * from.sin(),
        ));
        self.sweep(center, radius, from, sweep);
    }

    /// Continue from the circle's point at `from` along it, as cubic
    /// curves of a quarter turn at most.
    fn sweep(&mut self, center: Point, radius: f32, from: f32, sweep: f32) {
        let pieces = (sweep.abs() / (PI / 2.0)).ceil().max(1.0);
        let step = sweep / pieces;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * radius;
        let mut angle = from;
        for _ in 0..pieces as usize {
            let next = angle + step;
            let (sin_a, cos_a) = angle.sin_cos();
            let (sin_b, cos_b) = next.sin_cos();
            let start = (center.x + radius * cos_a, center.y + radius * sin_a);
            let end = (center.x + radius * cos_b, center.y + radius * sin_b);
            self.path.cubic_to(
                start.0 - k * sin_a,
                start.1 + k * cos_a,
                end.0 + k * sin_b,
                end.1 - k * cos_b,
                end.0,
                end.1,
            );
            angle = next;
        }
    }
}

/// Draws a shape as coverage, to be stamped in any color.
pub struct Pen {
    pixmap: Pixmap,
}

impl Pen {
    fn paint() -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(tiny_skia::Color::WHITE);
        paint.anti_alias = true;
        paint
    }

    pub fn fill(&mut self, outline: Outline) {
        if let Some(path) = outline.path.finish() {
            self.pixmap.fill_path(
                &path,
                &Self::paint(),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    pub fn stroke(&mut self, outline: Outline, width: f32, cap: LineCap, join: LineJoin) {
        if let Some(path) = outline.path.finish() {
            let stroke = Stroke {
                width,
                line_cap: cap,
                line_join: join,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &Self::paint(), &stroke, Transform::identity(), None);
        }
    }

    /// The pixels drawn on, as coverage; `None` when there are none.
    fn into_bitmap(self) -> Option<Bitmap> {
        let width = self.pixmap.width();
        let alpha = |x: u32, y: u32| self.pixmap.data()[((y * width + x) * 4 + 3) as usize];
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..self.pixmap.height() {
            for x in 0..width {
                if alpha(x, y) > 0 {
                    left = left.min(x);
                    right = right.max(x + 1);
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        if right <= left {
            return None;
        }
        let mut mask = Vec::with_capacity(((right - left) * (bottom - top)) as usize);
        for y in top..bottom {
            mask.extend((left..right).map(|x| alpha(x, y)));
        }
        Some(Bitmap {
            dx: left as i32,
            dy: top as i32,
            width: right - left,
            height: bottom - top,
            pixels: Pixels::Mask(mask),
        })
    }
}

/// Stamp the shape `key` in `color`: `draw` draws it the first time, on
/// `width` × `height` pixels whose top-left one is pixel `(x, y)` of the
/// window.
#[allow(clippy::too_many_arguments)]
pub fn stamp(
    out: &mut Vec<Quad>,
    atlas: &mut Atlas,
    key: ShapeKey,
    (x, y): (i32, i32),
    (width, height): (u32, u32),
    color: [u8; 4],
    draw: impl FnOnce(&mut Pen),
) {
    if color[3] == 0 {
        return;
    }
    let slot = atlas.shape(key, || {
        let mut pen = Pen {
            pixmap: Pixmap::new(width, height)?,
        };
        draw(&mut pen);
        pen.into_bitmap()
    });
    if let Some(slot) = slot {
        out.push(Quad {
            x: x + slot.dx,
            y: y + slot.dy,
            w: slot.w,
            h: slot.h,
            color,
            fill: Fill::Mask,
            u: slot.u,
            v: slot.v,
        });
    }
}

/// How far `at` is past a whole pixel, in 256ths.
pub fn phase(at: f32) -> u8 {
    ((at - at.floor()) * 256.0) as u8
}

/// Whether `c` is a shape drawn into the atlas by [`sketch`].
fn is_sketched(c: char) -> bool {
    matches!(
        c,
        '╭' | '╮' | '╯' | '╰' | '\u{2800}'
            ..='\u{28FF}'
                | '🗀'
                | '🗁'
                | '✓'
                | '✔'
                | '✗'
                | '✘'
                | '✕'
                | '✖'
                | '▸'
                | '►'
                | '▶'
                | '◂'
                | '◄'
                | '◀'
                | '▾'
                | '▼'
                | '▴'
                | '▲'
                | '◦'
                | '◐'
                | '◑'
                | '◒'
                | '◓'
                | '◇'
                | '◆'
                | '◈'
                | '■'
                | '□'
                | '▪'
                | '▫'
                | '▰'
                | '▱'
                | '⊘'
                | '⏱'
    )
}

fn stamp_char(out: &mut Vec<Quad>, atlas: &mut Atlas, c: char, cell: Rectangle, color: [u8; 4]) {
    let (left, top) = (cell.x.floor(), cell.y.floor());
    // Room around the cell: strokes are centered on its edges, and the
    // folders are two cells wide.
    let margin = (cell.width / 2.0).ceil().max(4.0);
    let local = Rectangle {
        x: margin + (cell.x - left),
        y: margin + (cell.y - top),
        ..cell
    };
    let key = ShapeKey {
        shape: c as u32,
        width: cell.width.to_bits(),
        height: cell.height.to_bits(),
        phase: [phase(cell.x), phase(cell.y)],
    };
    let size = (
        (cell.width * 2.0 + margin * 2.0).ceil() as u32 + 1,
        (cell.height + margin * 2.0).ceil() as u32 + 1,
    );
    let at = ((left - margin) as i32, (top - margin) as i32);
    stamp(out, atlas, key, at, size, color, |pen| {
        sketch(pen, c, local);
    });
}

fn sketch(pen: &mut Pen, c: char, cell: Rectangle) {
    match c {
        '╭' | '╮' | '╯' | '╰' => rounded_corner(pen, c, cell),
        '\u{2800}'..='\u{28FF}' => braille(pen, c, cell),
        '🗀' | '🗁' => folder(pen, c == '🗁', cell),
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

#[allow(clippy::too_many_lines)]
fn symbol(pen: &mut Pen, c: char, cell: Rectangle) {
    let w = cell.width;
    let (cx, cy) = (cell.x + w / 2.0, cell.y + cell.height / 2.0);
    let p = |dx: f32, dy: f32| Point::new(cx + dx * w, cy + dy * w);
    let thin = (w * 0.11).max(1.0);
    let stroke = |pen: &mut Pen, path: Outline, width: f32| {
        pen.stroke(path, width, LineCap::Round, LineJoin::Round);
    };
    match c {
        '✓' | '✔' => {
            let mut check = Outline::new();
            check.move_to(p(-0.36, 0.02));
            check.line_to(p(-0.1, 0.28));
            check.line_to(p(0.38, -0.3));
            stroke(pen, check, if c == '✔' { thin * 1.8 } else { thin * 1.4 });
        }
        '✗' | '✘' | '✕' | '✖' => {
            let width = if matches!(c, '✘' | '✖') {
                thin * 1.8
            } else {
                thin * 1.4
            };
            let mut cross = Outline::new();
            cross.move_to(p(-0.3, -0.3));
            cross.line_to(p(0.3, 0.3));
            cross.move_to(p(0.3, -0.3));
            cross.line_to(p(-0.3, 0.3));
            stroke(pen, cross, width);
        }
        '▸' | '►' => pen.fill(Outline::polygon(&[
            p(-0.22, -0.3),
            p(0.3, 0.0),
            p(-0.22, 0.3),
        ])),
        '▶' => pen.fill(Outline::polygon(&[
            p(-0.32, -0.42),
            p(0.4, 0.0),
            p(-0.32, 0.42),
        ])),
        '◂' | '◄' => pen.fill(Outline::polygon(&[
            p(0.22, -0.3),
            p(-0.3, 0.0),
            p(0.22, 0.3),
        ])),
        '◀' => pen.fill(Outline::polygon(&[
            p(0.32, -0.42),
            p(-0.4, 0.0),
            p(0.32, 0.42),
        ])),
        '▾' => pen.fill(Outline::polygon(&[
            p(-0.3, -0.18),
            p(0.3, -0.18),
            p(0.0, 0.3),
        ])),
        '▼' => pen.fill(Outline::polygon(&[
            p(-0.42, -0.3),
            p(0.42, -0.3),
            p(0.0, 0.42),
        ])),
        '▴' => pen.fill(Outline::polygon(&[
            p(-0.3, 0.18),
            p(0.3, 0.18),
            p(0.0, -0.3),
        ])),
        '▲' => pen.fill(Outline::polygon(&[
            p(-0.42, 0.3),
            p(0.42, 0.3),
            p(0.0, -0.42),
        ])),
        '◦' => stroke(pen, Outline::circle(p(0.0, 0.0), w * 0.17), thin),
        '◐' | '◑' | '◒' | '◓' => {
            let r = w * 0.38;
            stroke(pen, Outline::circle(p(0.0, 0.0), r), thin);
            // The filled half: left, right, bottom or top.
            let start = match c {
                '◐' => PI / 2.0,
                '◑' => -PI / 2.0,
                '◒' => 0.0,
                _ => PI,
            };
            let mut half = Outline::new();
            half.arc(p(0.0, 0.0), r, start, PI);
            half.close();
            pen.fill(half);
        }
        '◇' | '◆' | '◈' => {
            let s = 0.4;
            let diamond = Outline::polygon(&[p(0.0, -s), p(s, 0.0), p(0.0, s), p(-s, 0.0)]);
            if c == '◆' {
                pen.fill(diamond);
            } else {
                stroke(pen, diamond, thin);
            }
            if c == '◈' {
                let s = 0.17;
                pen.fill(Outline::polygon(&[
                    p(0.0, -s),
                    p(s, 0.0),
                    p(0.0, s),
                    p(-s, 0.0),
                ]));
            }
        }
        '■' | '□' | '▪' | '▫' => {
            let s = if matches!(c, '■' | '□') {
                0.36
            } else {
                0.22
            };
            let square = Outline::polygon(&[p(-s, -s), p(s, -s), p(s, s), p(-s, s)]);
            if matches!(c, '■' | '▪') {
                pen.fill(square);
            } else {
                stroke(pen, square, thin);
            }
        }
        '▰' | '▱' => {
            let (dx, h) = (w * 0.12, (cell.height * 0.24).round());
            let (x0, x1) = (cell.x + 1.0, cell.x + w - 1.0);
            let (y0, y1) = (cy - h, cy + h);
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
        '⊘' => {
            stroke(pen, Outline::circle(p(0.0, 0.0), w * 0.38), thin);
            stroke(pen, Outline::line(p(-0.27, 0.27), p(0.27, -0.27)), thin);
        }
        '⏱' => {
            let face = p(0.0, 0.06);
            stroke(pen, Outline::circle(face, w * 0.36), thin);
            stroke(pen, Outline::line(p(0.0, -0.42), p(0.0, -0.36)), thin * 1.4);
            stroke(pen, Outline::line(face, p(0.14, -0.1)), thin);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextSystem;

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
    fn every_shape_the_canvas_draws_is_drawn_here() {
        let mut text = TextSystem::new();
        let mut quads = Vec::new();
        let shapes = "─━│┃┌┐└┘┏┓┗┛├┤┬┴┼╭╮╯╰═║╔╗╚╝▀▄█▌▐░▒▓▖▟⠿⣿🗀🗁\
                      ✓✔✗✘✕✖▸►▶◂◄◀▾▼▴▲◦◐◑◒◓◇◆◈■□▪▫▰▱⊘‖⏱";
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
        let end = outline.path.last_point().expect("an arc");
        assert!((end.x - 4.0).abs() < 1e-4 && end.y.abs() < 1e-4, "{end:?}");
        let path = outline.path.finish().expect("a path");
        // Around (4, 4): the bounds stop at the legs.
        let bounds = path.bounds();
        assert!(bounds.left().abs() < 1e-3 && bounds.top().abs() < 1e-3);
        assert!((bounds.bottom() - 10.0).abs() < 1e-3);
    }
}
