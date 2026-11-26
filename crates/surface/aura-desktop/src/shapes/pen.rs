//! Drawing a shape once into the atlas, as coverage, and stamping it.

use super::outline::Outline;
use super::{LineCap, LineJoin};
use crate::scene::{Fill, Quad};
use crate::text::{Atlas, Bitmap, Pixels, ShapeKey};
use tiny_skia::{FillRule, Paint, Pixmap, Stroke, Transform};

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
