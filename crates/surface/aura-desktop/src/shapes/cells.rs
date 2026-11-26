//! Box drawing, block elements and octants: rectangles, drawn as solid quads.

use super::{center, light, rect};
use crate::geom::Rectangle;
use crate::scene::Quad;
use ratatui::symbols::pixel::OCTANTS;

/// Weights of the four arms (up, right, down, left): 0 none, 1 light, 2 heavy.
pub(super) fn box_edges(c: char) -> Option<[u8; 4]> {
    straight_or_corner(c)
        .or_else(|| tee_or_cross(c))
        .or_else(|| half_or_mixed(c))
}

/// Lines across the cell and corners.
fn straight_or_corner(c: char) -> Option<[u8; 4]> {
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
        _ => return None,
    })
}

/// Lines with a branch off them, and crossings.
fn tee_or_cross(c: char) -> Option<[u8; 4]> {
    Some(match c {
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
        _ => return None,
    })
}

/// Half lines, and lines that change weight halfway.
fn half_or_mixed(c: char) -> Option<[u8; 4]> {
    Some(match c {
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

pub(super) fn lines(
    out: &mut Vec<Quad>,
    cell: Rectangle,
    [up, right, down, left]: [u8; 4],
    color: [u8; 4],
) {
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

pub(super) fn double(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
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

pub(super) fn block(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
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
        '░' | '▒' | '▓' => shade(out, c, cell, color),
        '▔' => rect(out, l, y, r, y + eighth_h(1), color),
        '▕' => rect(out, x + w - eighth_w(1), y, x + w, y + h, color),
        _ => quadrants(out, c, cell, color),
    }
}

/// A shade: the whole cell in the color made fainter.
fn shade(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
    let Rectangle {
        x,
        y,
        width: w,
        height: h,
    } = cell;
    let a = match c {
        '░' => 0.25,
        '▒' => 0.5,
        _ => 0.75,
    };
    let [red, green, blue, alpha] = color;
    let alpha = (f32::from(alpha) * a).round() as u8;
    rect(out, x, y, x + w, y + h, [red, green, blue, alpha]);
}

/// A quadrant block: some of the cell's four quarters filled.
fn quadrants(out: &mut Vec<Quad>, c: char, cell: Rectangle, color: [u8; 4]) {
    let Rectangle {
        x,
        y,
        width: w,
        height: h,
    } = cell;
    let (l, r) = (x - 0.5, x + w + 0.5);
    let eighth_h = |n: u32| (h * n as f32 / 8.0).round();
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

/// The 2×4 pattern of octant character `c`, row-major from bit 0 at the
/// top left, as pasted images are drawn in some terminals. Patterns that
/// are ordinary block elements are left to [`block`].
pub(super) fn octant_bits(c: char) -> Option<u8> {
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
pub(super) fn octant(out: &mut Vec<Quad>, bits: u8, cell: Rectangle, color: [u8; 4]) {
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
pub(super) fn bars(out: &mut Vec<Quad>, cell: Rectangle, color: [u8; 4]) {
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
