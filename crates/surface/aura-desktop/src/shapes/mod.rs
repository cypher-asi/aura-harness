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

mod cells;
mod outline;
mod pen;
mod sketch;

use crate::geom::Rectangle;
use crate::scene::Quad;
use crate::text::{Atlas, ShapeKey};
use cells::{bars, block, box_edges, double, lines, octant, octant_bits};
use sketch::sketch;

pub use outline::Outline;
pub use pen::stamp;
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

/// Center of the cell on the pixel grid, where strokes of width `t` start.
fn center(cell: Rectangle, t: f32) -> (f32, f32) {
    let x = (cell.x + cell.width / 2.0 - t / 2.0).round();
    let y = (cell.y + cell.height / 2.0 - t / 2.0).round();
    (x, y)
}

fn rect(out: &mut Vec<Quad>, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
    out.extend(Quad::solid(x0, y0, x1, y1, color));
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
                | '☁'
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
    // folders and the cloud are two cells wide.
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

#[cfg(test)]
mod tests;
