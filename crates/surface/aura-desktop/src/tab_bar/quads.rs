//! The tab bar as quads.

use super::{label_size, unit, Bar, Hit, CONTROLS};
use crate::geom::{rgba, Color, Point, Rectangle};
use crate::scene::{fill, Layers, Quad};
use crate::shapes::{self, LineCap, LineJoin, Outline};
use crate::text::{Align, Face, Label, ShapeKey, Shaping, TextSystem, LABEL_LINE_EM};

/// The close box's cross among the shapes in the atlas: past the last
/// character.
const CROSS: u32 = 0x0011_0000;

impl Bar {
    /// The bar's quads, from the top of the window.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn quads(&self, text: &mut TextSystem, out: &mut Layers) {
        let Some(model) = &self.model else {
            return;
        };
        let (layout, hover) = (&self.layout, self.hover);
        let palette = model.palette;
        let unit = unit(&model.metrics);
        let line = (model.metrics.font_size / 14.0).round().max(1.0);
        let idle = palette.faint;
        let size = label_size(&model.metrics);
        let plain = Label {
            text: "",
            face: Face::mono(false),
            size,
            line_height: size * LABEL_LINE_EM,
            shaping: Shaping::Advanced,
            align: Align::Center,
            middle: true,
            color: [0; 4],
            on_row: false,
        };
        let middle = |rect: &Rectangle| Point::new(rect.center_x(), rect.center_y());

        for (index, (rect, item)) in layout.tabs.iter().zip(&model.items).enumerate() {
            let front = index == model.front;
            let ink = if front {
                fill(
                    &mut out.under,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    palette.selection,
                );
                palette.fg
            } else if hover == Some(Hit::Tab(index)) {
                outline(&mut out.under, *rect, palette.muted, line);
                palette.fg
            } else {
                outline(&mut out.under, *rect, idle, line);
                palette.muted
            };
            let label = Label {
                text: &item.label,
                face: Face::mono(front),
                color: rgba(ink),
                ..plain
            };
            text.write(&mut out.over, &label, middle(rect));
            if item.busy {
                // A green border around the whole box while an agent of
                // the tab works; drawn last so it wins over the edge above.
                outline(&mut out.under, *rect, palette.accent, line);
            }
        }

        let over = hover == Some(Hit::New);
        let (edge, ink) = if over {
            (palette.muted, palette.fg)
        } else {
            (idle, palette.muted)
        };
        outline(&mut out.under, layout.plus, edge, line);
        let arm = (unit * 0.55).round();
        let (cx, cy) = (
            layout.plus.center_x().round(),
            layout.plus.center_y().round(),
        );
        let half = (line / 2.0).floor();
        fill(
            &mut out.under,
            cx - arm,
            cy - half,
            2.0 * arm + line,
            line,
            ink,
        );
        fill(
            &mut out.under,
            cx - half,
            cy - arm,
            line,
            2.0 * arm + line,
            ink,
        );

        if let Some(title) = &layout.title {
            outline(&mut out.under, title.rect, idle, line);
            if !title.text.is_empty() {
                let ink = palette.fg.mix(palette.muted, 0.4);
                let at = Point::new(title.center, title.rect.center_y());
                let label = Label {
                    text: &title.text,
                    color: rgba(ink),
                    ..plain
                };
                text.write(&mut out.over, &label, at);
            }
        }

        for (rect, control) in layout.controls.iter().zip(CONTROLS) {
            let (edge, ink) = if hover == Some(control) {
                (palette.muted, palette.fg)
            } else {
                (idle, palette.muted)
            };
            outline(&mut out.under, *rect, edge, line);
            let (cx, cy) = (rect.center_x().round(), rect.center_y().round());
            match control {
                Hit::Minimize => fill(
                    &mut out.under,
                    cx - arm,
                    cy + arm - line,
                    2.0 * arm + line,
                    line,
                    ink,
                ),
                Hit::Maximize => {
                    let side = 2.0 * arm + line;
                    let square = Rectangle {
                        x: cx - arm,
                        y: cy - arm,
                        width: side,
                        height: side,
                    };
                    outline(&mut out.under, square, ink, line);
                }
                _ => cross(text, out, middle(rect), (arm, line), ink),
            }
        }
    }
}

/// An `×` centered on `middle`, `arm` pixels to each end, strokes `line`
/// pixels thick: two strokes corner to corner. One drawing serves every
/// place with the same offset from whole pixels.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cross(text: &mut TextSystem, out: &mut Layers, middle: Point, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let width = line * 1.25;
    let reach = 2.0 * arm;
    let pad = width.ceil() + 1.0;
    let corner = Point::new(middle.x - arm - pad, middle.y - arm - pad);
    let (left, top) = (corner.x.floor(), corner.y.floor());
    let (dx, dy) = (corner.x - left, corner.y - top);
    let key = ShapeKey {
        shape: CROSS,
        width: arm.to_bits(),
        height: line.to_bits(),
        phase: [shapes::phase(corner.x), shapes::phase(corner.y)],
    };
    let side = (reach + 2.0 * pad).ceil() as u32 + 1;
    let atlas = text.atlas_mut();
    shapes::stamp(
        &mut out.under,
        atlas,
        key,
        (left as i32, top as i32),
        (side, side),
        rgba(ink),
        |pen| {
            let (near, far) = (pad, pad + reach);
            for (from, to) in [(near, far), (far, near)] {
                let stroke = Outline::line(
                    Point::new(near + dx, from + dy),
                    Point::new(far + dx, to + dy),
                );
                pen.stroke(stroke, width, LineCap::Butt, LineJoin::Miter);
            }
        },
    );
}

/// A box outline `line` pixels thick, on whole pixels inside `rect`.
pub fn outline(out: &mut Vec<Quad>, rect: Rectangle, color: Color, line: f32) {
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    fill(out, x, y, w, line, color);
    fill(out, x, y + h - line, w, line, color);
    fill(out, x, y, line, h, color);
    fill(out, x + w - line, y, line, h, color);
}
