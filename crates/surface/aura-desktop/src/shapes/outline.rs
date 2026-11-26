//! Paths with arcs, cut into quadratic curves the way lyon cuts them.

use crate::geom::Point;
use std::f32::consts::PI;
use tiny_skia::PathBuilder;

/// A path being built: tiny-skia's builder, with arcs.
///
/// An arc is cut into quadratic curves of an eighth of a turn at most.
/// How a curve is cut shows in a thin stroke, so the cut is always the
/// same one: a symbol looks the same at every size it is drawn.
pub struct Outline {
    pub(super) path: PathBuilder,
    /// Where the path is, once it has started
    pub(super) at: Option<Point>,
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
        self.move_to(Point::new(
            center.x + radius * start.cos(),
            center.y + radius * start.sin(),
        ));
        self.sweep(center, radius, start, end);
    }

    /// A line to the angle `start` on the circle around `center`, then
    /// along it to the angle `end`: the stroke goes on unbroken, so its
    /// corners are joined rather than capped.
    pub(super) fn arc_on(&mut self, center: Point, radius: f32, start: f32, end: f32) {
        self.line_to(Point::new(
            center.x + radius * start.cos(),
            center.y + radius * start.sin(),
        ));
        self.sweep(center, radius, start, end);
    }

    /// Along the circle around `center` from the angle `start`, where the
    /// path is, to the angle `end`.
    fn sweep(&mut self, center: Point, radius: f32, start: f32, end: f32) {
        let sweep = end - start;
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
        let SvgArc {
            center,
            rx,
            ry,
            sweep,
        } = SvgArc::new(from, to, radius, clockwise);

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

/// The circle (an ellipse, once stretched) of an SVG arc between two
/// points, and how far the arc turns, found as lyon finds them.
struct SvgArc {
    center: Point,
    rx: f32,
    ry: f32,
    sweep: f32,
}

impl SvgArc {
    fn new(from: Point, to: Point, radius: f32, clockwise: bool) -> Self {
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
        Self {
            center,
            rx,
            ry,
            sweep,
        }
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
