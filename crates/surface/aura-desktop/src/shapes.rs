//! Characters drawn as shapes instead of font glyphs, as quads: box
//! drawing, block elements, braille and the status symbols the UI uses.
//!
//! Lines and blocks are rectangles, so they become solid quads.
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
use ratatui::symbols::pixel::OCTANTS;
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
    if let Some(bits) = octant_bits(c) {
        octant(out, bits, cell, color);
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

/// The 2×4 pattern of octant character `c`, row-major from bit 0 at the
/// top left, as pasted images are drawn in some terminals. Patterns that
/// are ordinary block elements are left to [`block`].
fn octant_bits(c: char) -> Option<u8> {
    if !matches!(c, '\u{1CD00}'..='\u{1CEAF}' | '\u{1FB82}'..='\u{1FBE7}') {
        return None;
    }
    OCTANTS
        .iter()
        .position(|&o| o == c)
        .and_then(|bits| u8::try_from(bits).ok())
}

/// An octant character: each set bit of `bits` fills its eighth of the
/// cell, two across and four down.
fn octant(out: &mut Vec<Quad>, bits: u8, cell: Rectangle, color: [u8; 4]) {
    let Rectangle {
        x,
        y,
        width: w,
        height: h,
    } = cell;
    let (l, r, mx) = (x - 0.5, x + w + 0.5, x + w / 2.0);
    let quarter = |n: u32| y + (h * n as f32 / 4.0).round();
    for i in 0..8u32 {
        if bits & (1 << i) == 0 {
            continue;
        }
        let (x0, x1) = if i % 2 == 0 { (l, mx) } else { (mx, r) };
        rect(out, x0, quarter(i / 2), x1, quarter(i / 2 + 1), color);
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

/// A path being built: tiny-skia's builder, with arcs.
///
/// An arc is cut into quadratic curves of an eighth of a turn at most.
/// How a curve is cut shows in a thin stroke, so the cut is always the
/// same one: a symbol looks the same at every size it is drawn.
pub struct Outline {
    path: PathBuilder,
    /// Where the path is, once it has started
    at: Option<Point>,
}

impl Outline {
    pub fn new() -> Self {
        Self {
            path: PathBuilder::new(),
            at: None,
        }
    }

    pub fn move_to(&mut self, to: Point) {
        self.path.move_to(to.x, to.y);
        self.at = Some(to);
    }

    pub fn line_to(&mut self, to: Point) {
        self.path.line_to(to.x, to.y);
        self.at = Some(to);
    }

    fn quad_to(&mut self, control: Point, to: Point) {
        self.path.quad_to(control.x, control.y, to.x, to.y);
        self.at = Some(to);
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

    /// A full circle, left open where it starts and ends.
    pub fn circle(center: Point, radius: f32) -> Self {
        let mut outline = Self::new();
        outline.arc(center, radius, 0.0, 2.0 * PI);
        outline
    }

    /// A new stroke along the circle around `center`, from the angle
    /// `start` to the angle `end`.
    pub fn arc(&mut self, center: Point, radius: f32, start: f32, end: f32) {
        let sweep = end - start;
        self.move_to(Point::new(
            center.x + radius * start.cos(),
            center.y + radius * start.sin(),
        ));
        // lyon cuts these arcs with twice the digits.
        let (cx, cy) = (f64::from(center.x), f64::from(center.y));
        let radii = (f64::from(radius), f64::from(radius));
        quads_f64(
            (cx, cy),
            radii,
            f64::from(start),
            f64::from(sweep),
            |control, to| {
                self.quad_to(
                    Point::new(control.0 as f32, control.1 as f32),
                    Point::new(to.0 as f32, to.1 as f32),
                );
            },
        );
    }

    /// A line towards `a` that turns towards `b` on a circle of `radius`
    /// touching both legs, ending where the circle meets the second.
    pub fn arc_to(&mut self, a: Point, b: Point, radius: f32) {
        let Some(start) = self.at else {
            return;
        };
        if start == a || a == b || radius == 0.0 {
            return self.line_to(a);
        }
        let double_area = start.x * (a.y - b.y) + a.x * (b.y - start.y) + b.x * (start.y - a.y);
        if double_area == 0.0 {
            return self.line_to(a);
        }
        let unit = |from: Point, to: Point| {
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            let length = (dx * dx + dy * dy).sqrt();
            (dx / length, dy / length)
        };
        let to_start = unit(a, start);
        let to_end = unit(a, b);
        let inner = (to_start.0 * to_end.0 + to_start.1 * to_end.1).acos();
        let along = radius / (inner / 2.0).tan();
        let arc_start = Point::new(a.x + to_start.0 * along, a.y + to_start.1 * along);
        let arc_end = Point::new(a.x + to_end.0 * along, a.y + to_end.1 * along);
        let clockwise = to_start.0 * to_end.1 - to_start.1 * to_end.0 < 0.0;
        self.line_to(arc_start);
        self.short_arc(arc_end, radius, clockwise);
    }

    /// The short arc of a circle of `radius` from where the path is to
    /// `to`: an SVG arc, found and cut as lyon does.
    fn short_arc(&mut self, to: Point, radius: f32, clockwise: bool) {
        let Some(from) = self.at else {
            return;
        };
        if radius.abs() <= f32::EPSILON || from == to {
            return self.line_to(to);
        }
        let (mut rx, mut ry) = (radius.abs(), radius.abs());
        let half = ((from.x - to.x) / 2.0, (from.y - to.y) / 2.0);
        let middle = ((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
        // Radii too short to span the two points are stretched.
        let short = half.0 * half.0 / (rx * rx) + half.1 * half.1 / (ry * ry);
        if short > 1.0 {
            let scale = short.sqrt();
            rx *= scale;
            ry *= scale;
        }
        let rxry = rx * ry;
        let rxpy = rx * half.1;
        let rypx = ry * half.0;
        let sum = rxpy * rxpy + rypx * rypx;
        let side = if clockwise { 1.0 } else { -1.0 };
        let reach = side * ((rxry * rxry - sum) / sum).abs().sqrt();
        let offset = (reach * rxpy / ry, -reach * rypx / rx);
        let center = Point::new(offset.0 + middle.0, offset.1 + middle.1);
        let start = ((half.0 - offset.0) / rx, (half.1 - offset.1) / ry);
        let end = ((-half.0 - offset.0) / rx, (-half.1 - offset.1) / ry);
        let start_angle = fast_atan2(start.1, start.0);
        let mut sweep = (fast_atan2(end.1, end.0) - start_angle) % (2.0 * PI);
        if clockwise && sweep < 0.0 {
            sweep += 2.0 * PI;
        } else if !clockwise && sweep > 0.0 {
            sweep -= 2.0 * PI;
        }

        // The arc starts at the angle the path is at around the center,
        // which is not quite the one just found.
        if (from.x - center.x).abs() < 1e-6 && (from.y - center.y).abs() < 1e-6 {
            return;
        }
        let start_angle = fast_atan2(from.y - center.y, from.x - center.x);
        let arc_start = Point::new(
            center.x + rx * start_angle.cos(),
            center.y + ry * start_angle.sin(),
        );
        let (dx, dy) = (arc_start.x - from.x, arc_start.y - from.y);
        if dx * dx + dy * dy > 0.01 {
            self.line_to(arc_start);
        }
        quads_f32(
            (center.x, center.y),
            (rx, ry),
            start_angle,
            sweep,
            |control, to| self.quad_to(Point::new(control.0, control.1), Point::new(to.0, to.1)),
        );
    }
}

/// lyon's `atan2`, a little off the real one.
#[allow(clippy::excessive_precision)]
fn fast_atan2(y: f32, x: f32) -> f32 {
    let (x_abs, y_abs) = (x.abs(), y.abs());
    let a = x_abs.min(y_abs) / x_abs.max(y_abs);
    let s = a * a;
    let mut result = ((-0.046_496_474_9 * s + 0.159_314_22) * s - 0.327_622_764) * s * a + a;
    if y_abs > x_abs {
        result = std::f32::consts::FRAC_PI_2 - result;
    }
    if x < 0.0 {
        result = PI - result;
    }
    if y < 0.0 {
        result = -result;
    }
    result
}

/// lyon's cut of an arc into quadratic curves, an eighth of a turn each
/// at most: `quad` gets each curve's control point and end.
macro_rules! arc_quads {
    ($name:ident, $float:ident) => {
        #[allow(clippy::cast_lossless)]
        fn $name(
            center: ($float, $float),
            radii: ($float, $float),
            start: $float,
            sweep: $float,
            mut quad: impl FnMut(($float, $float), ($float, $float)),
        ) {
            use std::$float::consts::{FRAC_PI_4, PI};
            let sign = sweep.signum();
            let sweep = sweep.abs().min(PI * 2.0);
            let steps = (sweep / FRAC_PI_4).ceil();
            let step = sweep / steps * sign;
            let alpha = (step * 0.5).tan();
            for i in 0..steps as i32 {
                let a1 = start + step * (i as $float);
                let a2 = start + step * ((i + 1) as $float);
                let from = (center.0 + radii.0 * a1.cos(), center.1 + radii.1 * a1.sin());
                let to = (center.0 + radii.0 * a2.cos(), center.1 + radii.1 * a2.sin());
                let tangent = (-radii.0 * a1.sin(), radii.1 * a1.cos());
                quad((from.0 + tangent.0 * alpha, from.1 + tangent.1 * alpha), to);
            }
        }
    };
}
arc_quads!(quads_f32, f32);
arc_quads!(quads_f64, f64);

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
                | '⬢'
                | '☐'
                | '☒'
                | '▣'
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
            half.arc(p(0.0, 0.0), r, start, start + PI);
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
        '☐' | '☒' | '▣' => {
            // Wider than the cell's other squares: the to-do boxes are read
            // at a glance, and the space after them leaves room to overhang.
            let s = 0.5;
            let square = Outline::polygon(&[p(-s, -s), p(s, -s), p(s, s), p(-s, s)]);
            stroke(pen, square, thin);
            if c == '☒' {
                let s = 0.28;
                let mut cross = Outline::new();
                cross.move_to(p(-s, -s));
                cross.line_to(p(s, s));
                cross.move_to(p(s, -s));
                cross.line_to(p(-s, s));
                stroke(pen, cross, thin);
            } else if c == '▣' {
                let s = 0.26;
                pen.fill(Outline::polygon(&[p(-s, -s), p(s, -s), p(s, s), p(-s, s)]));
            }
        }
        '⬢' => {
            let r = 0.42;
            let corners: Vec<Point> = (0..6u8)
                .map(|i| {
                    let angle = PI / 3.0 * f32::from(i) - PI / 2.0;
                    p(r * angle.cos(), r * angle.sin())
                })
                .collect();
            pen.fill(Outline::polygon(&corners));
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
    fn every_shape_the_ui_uses_is_drawn_and_letters_are_left_to_the_font() {
        let mut text = TextSystem::new();
        let mut quads = Vec::new();
        let shapes = "─━│┃┌┐└┘┏┓┗┛├┤┬┴┼╭╮╯╰═║╔╗╚╝▀▄█▌▐░▒▓▖▟⠿⣿🗀🗁\
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
}
