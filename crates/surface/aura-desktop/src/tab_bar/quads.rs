//! The tab bar as quads.

use super::{label_size, unit, Bar, Hit, Title, CONTROLS};
use crate::geom::{rgba, Color, Point, Rectangle};
use crate::grid::Palette;
use crate::scene::{fill, Layers, Quad};
use crate::shapes::{self, LineCap, LineJoin, Outline};
use crate::text::{Align, Face, Label, ShapeKey, Shaping, TextSystem, LABEL_LINE_EM};

/// The close box's cross among the shapes in the atlas: past the last
/// character.
const CROSS: u32 = 0x0011_0000;

/// What every section of the bar is drawn with.
struct Pen {
    palette: Palette,
    /// Stroke thickness
    line: f32,
    /// How far the glyphs in the boxes reach from their middles
    arm: f32,
    plain: Label<'static>,
}

/// The middle of `rect`.
fn middle(rect: &Rectangle) -> Point {
    Point::new(rect.center_x(), rect.center_y())
}

impl Bar {
    /// The bar's quads, from the top of the window.
    pub fn quads(&self, text: &mut TextSystem, out: &mut Layers) {
        let Some(model) = &self.model else {
            return;
        };
        let size = label_size(&model.metrics);
        let pen = Pen {
            palette: model.palette,
            line: (model.metrics.font_size / 14.0).round().max(1.0),
            arm: (unit(&model.metrics) * 0.55).round(),
            plain: Label {
                text: "",
                face: Face::mono(false),
                size,
                line_height: size * LABEL_LINE_EM,
                shaping: Shaping::Advanced,
                align: Align::Center,
                middle: true,
                color: [0; 4],
                on_row: false,
            },
        };
        self.tabs_quads(&pen, text, out);
        self.plus_quads(&pen, out);
        if let Some(title) = &self.layout.title {
            title_quads(&pen, title, text, out);
        }
        self.controls_quads(&pen, text, out);
    }

    /// The tabs' boxes and labels.
    fn tabs_quads(&self, pen: &Pen, text: &mut TextSystem, out: &mut Layers) {
        let Some(model) = &self.model else {
            return;
        };
        let (palette, line) = (pen.palette, pen.line);
        for (index, (rect, item)) in self.layout.tabs.iter().zip(&model.items).enumerate() {
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
            } else if self.hover == Some(Hit::Tab(index)) {
                outline(&mut out.under, *rect, palette.muted, line);
                palette.fg
            } else {
                outline(&mut out.under, *rect, palette.faint, line);
                palette.muted
            };
            let label = Label {
                text: &item.label,
                face: Face::mono(front),
                color: rgba(ink),
                ..pen.plain
            };
            text.write(&mut out.over, &label, middle(rect));
            if item.busy {
                // A green border around the whole box while an agent of
                // the tab works; drawn last so it wins over the edge above.
                outline(&mut out.under, *rect, palette.accent, line);
            }
        }
    }

    /// The edge and ink of a box, brighter while `hit` is under the
    /// pointer.
    fn box_colors(&self, pen: &Pen, hit: Hit) -> (Color, Color) {
        if self.hover == Some(hit) {
            (pen.palette.muted, pen.palette.fg)
        } else {
            (pen.palette.faint, pen.palette.muted)
        }
    }

    /// The `+` box that opens a tab.
    fn plus_quads(&self, pen: &Pen, out: &mut Layers) {
        let (line, arm, plus) = (pen.line, pen.arm, self.layout.plus);
        let (edge, ink) = self.box_colors(pen, Hit::New);
        outline(&mut out.under, plus, edge, line);
        let (cx, cy) = (plus.center_x().round(), plus.center_y().round());
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
    }

    /// The window's minimize, maximize and close boxes.
    fn controls_quads(&self, pen: &Pen, text: &mut TextSystem, out: &mut Layers) {
        let (line, arm) = (pen.line, pen.arm);
        for (rect, control) in self.layout.controls.iter().zip(CONTROLS) {
            let (edge, ink) = self.box_colors(pen, control);
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

/// The title's box and text.
fn title_quads(pen: &Pen, title: &Title, text: &mut TextSystem, out: &mut Layers) {
    let palette = pen.palette;
    outline(&mut out.under, title.rect, palette.faint, pen.line);
    if !title.text.is_empty() {
        let ink = palette.fg.mix(palette.muted, 0.4);
        let at = Point::new(title.center, title.rect.center_y());
        let label = Label {
            text: &title.text,
            color: rgba(ink),
            ..pen.plain
        };
        text.write(&mut out.over, &label, at);
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
