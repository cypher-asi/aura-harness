//! Characters drawn as shapes instead of font glyphs: box drawing, block
//! elements, braille and the status symbols the UI uses.
//!
//! Fonts size these to their own design, so borders leave gaps between
//! rows and symbols missing from Geist Mono come from whatever fallback
//! font is installed. Drawn here, they fill the cell exactly and line
//! strokes sit on whole pixels, the way GPU terminals render them.

// Pixel math on cell sizes and small counts.
#![allow(clippy::cast_precision_loss)]

use iced::widget::canvas::{path::Arc, Frame, LineCap, LineJoin, Path, Stroke};
use iced::{Color, Point, Radians, Rectangle, Size};
use std::f32::consts::PI;

/// Draw `c` into `cell`; `false` when it isn't one of the built-in shapes
/// and should come from the font.
pub fn draw(frame: &mut Frame, c: char, cell: Rectangle, color: Color) -> bool {
    if let Some(edges) = box_edges(c) {
        lines(frame, cell, edges, color);
        return true;
    }
    match c {
        '╭' | '╮' | '╯' | '╰' => rounded_corner(frame, c, cell, color),
        '═' | '║' | '╔' | '╗' | '╚' | '╝' => double(frame, c, cell, color),
        '\u{2580}'..='\u{259F}' => block(frame, c, cell, color),
        '\u{2800}'..='\u{28FF}' => braille(frame, c, cell, color),
        '🗀' | '🗁' => folder(frame, c == '🗁', cell, color),
        _ => return shape(frame, c, cell, color),
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

fn rect(frame: &mut Frame, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
    if x1 > x0 && y1 > y0 {
        frame.fill_rectangle(Point::new(x0, y0), Size::new(x1 - x0, y1 - y0), color);
    }
}

fn lines(frame: &mut Frame, cell: Rectangle, [up, right, down, left]: [u8; 4], color: Color) {
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
            rect(frame, sx, a, sx + w, b, color);
        }
    }
    for (weight, west) in [(left, true), (right, false)] {
        if weight > 0 {
            let w = width(weight);
            let (_, sy) = center(cell, w);
            let (a, b) = if west { (x0, vx + tv) } else { (vx, x1) };
            rect(frame, a, sy, b, sy + w, color);
        }
    }
}

fn rounded_corner(frame: &mut Frame, c: char, cell: Rectangle, color: Color) {
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
    let path = Path::new(|b| {
        b.move_to(start);
        b.arc_to(corner, end, r);
        b.line_to(end);
    });
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(color)
            .with_width(t)
            .with_line_cap(LineCap::Butt),
    );
}

fn double(frame: &mut Frame, c: char, cell: Rectangle, color: Color) {
    let t = light(cell);
    let gap = (cell.width / 5.0).round().max(t + 1.0);
    let (cx, cy) = center(cell, t);
    let (left, right) = (cell.x - 0.5, cell.x + cell.width + 0.5);
    let (top, bottom) = (cell.y, cell.y + cell.height);
    let (outer_x, inner_x) = (cx - gap, cx + gap);
    let (outer_y, inner_y) = (cy - gap, cy + gap);
    let h = |frame: &mut Frame, x0: f32, x1: f32, y: f32| rect(frame, x0, y, x1, y + t, color);
    match c {
        '═' => {
            h(frame, left, right, outer_y);
            h(frame, left, right, inner_y);
        }
        '║' => {
            rect(frame, outer_x, top, outer_x + t, bottom, color);
            rect(frame, inner_x, top, inner_x + t, bottom, color);
        }
        '╔' => {
            h(frame, outer_x, right, outer_y);
            h(frame, inner_x, right, inner_y);
            rect(frame, outer_x, outer_y, outer_x + t, bottom, color);
            rect(frame, inner_x, inner_y, inner_x + t, bottom, color);
        }
        '╗' => {
            h(frame, left, inner_x + t, outer_y);
            h(frame, left, outer_x + t, inner_y);
            rect(frame, inner_x, outer_y, inner_x + t, bottom, color);
            rect(frame, outer_x, inner_y, outer_x + t, bottom, color);
        }
        '╚' => {
            h(frame, outer_x, right, inner_y);
            h(frame, inner_x, right, outer_y);
            rect(frame, outer_x, top, outer_x + t, inner_y + t, color);
            rect(frame, inner_x, top, inner_x + t, outer_y + t, color);
        }
        _ => {
            h(frame, left, inner_x + t, inner_y);
            h(frame, left, outer_x + t, outer_y);
            rect(frame, inner_x, top, inner_x + t, inner_y + t, color);
            rect(frame, outer_x, top, outer_x + t, outer_y + t, color);
        }
    }
}

fn block(frame: &mut Frame, c: char, cell: Rectangle, color: Color) {
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
        '▀' => rect(frame, l, y, r, y + eighth_h(4), color),
        '▁'..='█' => {
            let n = c as u32 - 0x2580;
            rect(frame, l, y + h - eighth_h(n), r, y + h, color);
        }
        '▉'..='▏' => {
            let n = 0x2590 - c as u32;
            rect(frame, x, y, x + eighth_w(n), y + h, color);
        }
        '▐' => rect(frame, x + w / 2.0, y, r, y + h, color),
        '░' | '▒' | '▓' => {
            let a = match c {
                '░' => 0.25,
                '▒' => 0.5,
                _ => 0.75,
            };
            rect(
                frame,
                x,
                y,
                x + w,
                y + h,
                Color {
                    a: color.a * a,
                    ..color
                },
            );
        }
        '▔' => rect(frame, l, y, r, y + eighth_h(1), color),
        '▕' => rect(frame, x + w - eighth_w(1), y, x + w, y + h, color),
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
                    rect(frame, x0, y0, x1, y1, color);
                }
            }
        }
    }
}

fn braille(frame: &mut Frame, c: char, cell: Rectangle, color: Color) {
    let bits = c as u32 - 0x2800;
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
    let radius = (cell.width * 0.12).max(0.8);
    for (bit, (col, row)) in DOTS.into_iter().enumerate() {
        if bits & (1 << bit) != 0 {
            let at = Point::new(
                cell.x + cell.width * (0.28 + 0.44 * col),
                cell.y + cell.height * (0.125 + 0.25 * row),
            );
            frame.fill(&Path::circle(at, radius), color);
        }
    }
}

/// An outline folder, open or closed, on a 24-unit grid. A folder one cell
/// wide reads as a sliver, so it spans this cell and the next, which must
/// be blank: the projects list's two-cell icon column.
fn folder(frame: &mut Frame, open: bool, cell: Rectangle, color: Color) {
    let s = (cell.width * 1.7 / 20.0).min(cell.height * 0.66 / 17.0);
    let (cx, cy) = (cell.x + cell.width, cell.y + cell.height / 2.0);
    let q = |x: f32, y: f32| Point::new(cx + (x - 12.0) * s, cy + (y - 11.5) * s);
    let path = Path::new(|b| {
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
    });
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(color)
            .with_width((1.8 * s).max(1.0))
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    );
}

fn shape(frame: &mut Frame, c: char, cell: Rectangle, color: Color) -> bool {
    let w = cell.width;
    let (cx, cy) = (cell.x + w / 2.0, cell.y + cell.height / 2.0);
    let p = |dx: f32, dy: f32| Point::new(cx + dx * w, cy + dy * w);
    let stroke = |width: f32| {
        Stroke::default()
            .with_color(color)
            .with_width(width)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round)
    };
    let thin = (w * 0.11).max(1.0);
    let polygon = |points: &[Point]| {
        Path::new(|b| {
            b.move_to(points[0]);
            for point in &points[1..] {
                b.line_to(*point);
            }
            b.close();
        })
    };
    match c {
        '✓' | '✔' => {
            let check = Path::new(|b| {
                b.move_to(p(-0.36, 0.02));
                b.line_to(p(-0.1, 0.28));
                b.line_to(p(0.38, -0.3));
            });
            frame.stroke(
                &check,
                stroke(if c == '✔' { thin * 1.8 } else { thin * 1.4 }),
            );
        }
        '✗' | '✘' | '✕' | '✖' => {
            let width = if matches!(c, '✘' | '✖') {
                thin * 1.8
            } else {
                thin * 1.4
            };
            let cross = Path::new(|b| {
                b.move_to(p(-0.3, -0.3));
                b.line_to(p(0.3, 0.3));
                b.move_to(p(0.3, -0.3));
                b.line_to(p(-0.3, 0.3));
            });
            frame.stroke(&cross, stroke(width));
        }
        '▸' | '►' => frame.fill(
            &polygon(&[p(-0.22, -0.3), p(0.3, 0.0), p(-0.22, 0.3)]),
            color,
        ),
        '▶' => frame.fill(
            &polygon(&[p(-0.32, -0.42), p(0.4, 0.0), p(-0.32, 0.42)]),
            color,
        ),
        '◂' | '◄' => frame.fill(
            &polygon(&[p(0.22, -0.3), p(-0.3, 0.0), p(0.22, 0.3)]),
            color,
        ),
        '◀' => frame.fill(
            &polygon(&[p(0.32, -0.42), p(-0.4, 0.0), p(0.32, 0.42)]),
            color,
        ),
        '▾' => frame.fill(
            &polygon(&[p(-0.3, -0.18), p(0.3, -0.18), p(0.0, 0.3)]),
            color,
        ),
        '▼' => frame.fill(
            &polygon(&[p(-0.42, -0.3), p(0.42, -0.3), p(0.0, 0.42)]),
            color,
        ),
        '▴' => frame.fill(
            &polygon(&[p(-0.3, 0.18), p(0.3, 0.18), p(0.0, -0.3)]),
            color,
        ),
        '▲' => frame.fill(
            &polygon(&[p(-0.42, 0.3), p(0.42, 0.3), p(0.0, -0.42)]),
            color,
        ),
        '◦' => frame.stroke(&Path::circle(p(0.0, 0.0), w * 0.17), stroke(thin)),
        '◐' | '◑' | '◒' | '◓' => {
            let r = w * 0.38;
            frame.stroke(&Path::circle(p(0.0, 0.0), r), stroke(thin));
            // The filled half: left, right, bottom or top.
            let start = match c {
                '◐' => PI / 2.0,
                '◑' => -PI / 2.0,
                '◒' => 0.0,
                _ => PI,
            };
            let half = Path::new(|b| {
                b.arc(Arc {
                    center: p(0.0, 0.0),
                    radius: r,
                    start_angle: Radians(start),
                    end_angle: Radians(start + PI),
                });
                b.close();
            });
            frame.fill(&half, color);
        }
        '◇' | '◆' | '◈' => {
            let s = 0.4;
            let diamond = polygon(&[p(0.0, -s), p(s, 0.0), p(0.0, s), p(-s, 0.0)]);
            if c == '◆' {
                frame.fill(&diamond, color);
            } else {
                frame.stroke(&diamond, stroke(thin));
            }
            if c == '◈' {
                let s = 0.17;
                frame.fill(
                    &polygon(&[p(0.0, -s), p(s, 0.0), p(0.0, s), p(-s, 0.0)]),
                    color,
                );
            }
        }
        '■' | '□' | '▪' | '▫' => {
            let s = if matches!(c, '■' | '□') {
                0.36
            } else {
                0.22
            };
            let square = polygon(&[p(-s, -s), p(s, -s), p(s, s), p(-s, s)]);
            if matches!(c, '■' | '▪') {
                frame.fill(&square, color);
            } else {
                frame.stroke(&square, stroke(thin));
            }
        }
        '▰' | '▱' => {
            let (dx, h) = (w * 0.12, (cell.height * 0.24).round());
            let (x0, x1) = (cell.x + 1.0, cell.x + w - 1.0);
            let (y0, y1) = (cy - h, cy + h);
            let shape = polygon(&[
                Point::new(x0 + dx, y0),
                Point::new(x1, y0),
                Point::new(x1 - dx, y1),
                Point::new(x0, y1),
            ]);
            if c == '▰' {
                frame.fill(&shape, color);
            } else {
                frame.stroke(&shape, stroke(thin));
            }
        }
        '⊘' => {
            frame.stroke(&Path::circle(p(0.0, 0.0), w * 0.38), stroke(thin));
            frame.stroke(&Path::line(p(-0.27, 0.27), p(0.27, -0.27)), stroke(thin));
        }
        '‖' => {
            let t = light(cell);
            for dx in [-0.18, 0.18] {
                let x = (cx + dx * w).round();
                rect(
                    frame,
                    x,
                    cell.y + 1.0,
                    x + t,
                    cell.y + cell.height - 1.0,
                    color,
                );
            }
        }
        '⏱' => {
            let face = p(0.0, 0.06);
            frame.stroke(&Path::circle(face, w * 0.36), stroke(thin));
            frame.stroke(
                &Path::line(p(0.0, -0.42), p(0.0, -0.36)),
                stroke(thin * 1.4),
            );
            frame.stroke(&Path::line(face, p(0.14, -0.1)), stroke(thin));
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_corners_point_into_the_box() {
        assert_eq!(box_edges('┌'), Some([0, 1, 1, 0]));
        assert_eq!(box_edges('┘'), Some([1, 0, 0, 1]));
        assert_eq!(box_edges('┼'), Some([1, 1, 1, 1]));
        assert_eq!(box_edges('━'), Some([0, 2, 0, 2]));
        assert_eq!(box_edges('a'), None);
    }

    #[test]
    fn every_box_drawing_character_the_ui_uses_is_built_in() {
        for c in "─━│┃┌┐└┘┏┓┗┛├┤┬┴┼╭╮╯╰═║╔╗╚╝".chars()
        {
            let built_in = box_edges(c).is_some() || "╭╮╯╰═║╔╗╚╝".contains(c);
            assert!(built_in, "{c} falls back to the font");
        }
    }
}
