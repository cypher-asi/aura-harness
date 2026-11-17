//! The title bars as quads, for the bare renderer: what [`super::paint`]
//! draws on an iced canvas.

use super::{Chrome, Hit, Kind};
use crate::geom::{rgba, Point};
use crate::scene::{fill, Layers};
use crate::tab_bar::{label_size, quads::outline};
use crate::text::{Align, Face, Label, Shaping, TextSystem, LABEL_LINE_EM};

impl Chrome {
    /// The bars' quads, the grid's canvas starting `top` pixels down the
    /// window.
    pub fn quads(&self, text: &mut TextSystem, out: &mut Layers, top: f32) {
        let Some(model) = &self.model else {
            return;
        };
        let palette = model.palette;
        let line = (model.metrics.font_size / 14.0).round().max(1.0);
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

        for bar in &self.layout.bars {
            let accent = if bar.focused {
                palette.accent
            } else {
                palette.muted
            };
            for boxed in &bar.boxes {
                let mut rect = boxed.rect;
                rect.y += top;
                let filled = match boxed.kind {
                    Kind::Title => true,
                    Kind::Tab(tab) => bar.active == Some(tab),
                };
                let over =
                    matches!(boxed.kind, Kind::Tab(tab) if self.hover == Some(Hit::Tab(tab)));
                let ink = if filled {
                    fill(
                        &mut out.under,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        accent,
                    );
                    palette.bg
                } else if over {
                    outline(&mut out.under, rect, palette.accent, line);
                    palette.fg
                } else {
                    outline(&mut out.under, rect, palette.muted, line);
                    palette.muted
                };
                let middle = Point::new(rect.center_x(), rect.center_y());
                let label = Label {
                    text: &boxed.text,
                    face: Face::mono(filled),
                    color: rgba(ink),
                    ..plain
                };
                text.write(&mut out.over, &label, middle);
            }
            if let Some(info) = &bar.info {
                let end = Point::new(info.end.x, info.end.y + top);
                let label = Label {
                    text: &info.text,
                    align: Align::Right,
                    color: rgba(info.color),
                    ..plain
                };
                text.write(&mut out.over, &label, end);
            }
        }
    }
}
