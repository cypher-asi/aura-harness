//! The title bars as quads.

use super::{Chrome, Hit, Kind};
use crate::geom::{rgba, Color, Point, Rectangle};
use crate::scene::{fill, Layers, Quad};
use crate::tab_bar::{
    label_size,
    quads::{cross, outline},
    unit,
};
use crate::text::{Align, Face, Label, Shaping, TextSystem, LABEL_LINE_EM};
use aura_ui_terminal::SideScope;

impl Chrome {
    /// The bars' quads, the grid starting `top` pixels down the
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
            for boxed in &bar.boxes {
                let mut rect = boxed.rect;
                rect.y += top;
                let filled = match boxed.kind {
                    Kind::Title => true,
                    Kind::Tab(tab) => bar.active == Some(tab),
                };
                let over =
                    matches!(boxed.kind, Kind::Tab(tab) if self.hover == Some(Hit::Tab(tab)));
                let ink = if filled && bar.focused {
                    fill(
                        &mut out.under,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        palette.selection,
                    );
                    palette.fg
                } else if filled {
                    outline(&mut out.under, rect, palette.faint, line);
                    palette.fg
                } else if over {
                    outline(&mut out.under, rect, palette.muted, line);
                    palette.fg
                } else {
                    outline(&mut out.under, rect, palette.faint, line);
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
            if let Some(mut rect) = bar.close {
                rect.y += top;
                let (edge, ink) = if self.hover == Some(Hit::Close(bar.pane)) {
                    (palette.muted, palette.fg)
                } else {
                    (palette.faint, palette.muted)
                };
                outline(&mut out.under, rect, edge, line);
                let arm = (unit(&model.metrics) * 0.55).round();
                let middle = Point::new(rect.center_x(), rect.center_y());
                cross(text, out, middle, (arm, line), ink);
            }
            if let Some((mut rect, on)) = bar.scope {
                rect.y += top;
                let over = self.hover == Some(Hit::Scope(bar.pane));
                let ink = if on && bar.focused {
                    fill(
                        &mut out.under,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        palette.selection,
                    );
                    palette.fg
                } else if on || over {
                    let edge = if over { palette.muted } else { palette.faint };
                    outline(&mut out.under, rect, edge, line);
                    palette.fg
                } else {
                    outline(&mut out.under, rect, palette.faint, line);
                    palette.muted
                };
                let label = Label {
                    text: SideScope::BUTTON,
                    face: Face::mono(on),
                    color: rgba(ink),
                    ..plain
                };
                text.write(
                    &mut out.over,
                    &label,
                    Point::new(rect.center_x(), rect.center_y()),
                );
            }
            if let Some((mut rect, expanded)) = bar.expand {
                rect.y += top;
                let (edge, ink) = if self.hover == Some(Hit::Expand(bar.pane)) {
                    (palette.muted, palette.fg)
                } else {
                    (palette.faint, palette.muted)
                };
                outline(&mut out.under, rect, edge, line);
                let reach = (unit(&model.metrics) * 0.6).round();
                let middle = Point::new(rect.center_x().round(), rect.center_y().round());
                corners(&mut out.under, middle, (reach, line), expanded, ink);
            }
        }
        if let Some(mut rect) = self.layout.side_toggle {
            rect.y += top;
            let (edge, ink) = if self.hover == Some(Hit::SideToggle) {
                (palette.muted, palette.fg)
            } else {
                (palette.faint, palette.muted)
            };
            outline(&mut out.under, rect, edge, line);
            // As wide as the window boxes' glyphs: their arm each way.
            let arm = (unit(&model.metrics) * 0.55).round();
            hamburger(&mut out.under, rect, (arm, line), ink);
        }
    }
}

/// Three bars centered in `rect`, about `arm` pixels each way from the
/// middle, strokes `line` pixels thick. The glyph is laid out from the
/// box's edges rather than from a rounded middle, which lands half a
/// pixel off and then rounds a whole pixel right and down in a box of
/// odd size; its width takes the box's parity so the margins match.
fn hamburger(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let step = (arm * 0.7).round().max(2.0 * line);
    let height = 2.0 * step + line;
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - height) / 2.0).floor();
    for offset in [0.0, step, 2.0 * step] {
        fill(out, left, top + offset, width, line, ink);
    }
}

/// Four corner brackets around `middle`, `reach` pixels out, strokes
/// `line` pixels thick: at the corners and opening inward to expand, or
/// near the middle and opening outward to restore.
fn corners(out: &mut Vec<Quad>, middle: Point, size: (f32, f32), restore: bool, ink: Color) {
    let (reach, line) = size;
    let gap = (reach * 0.3).round().max(line);
    let (from, arm) = if restore {
        (gap, reach - gap)
    } else {
        (reach, (reach * 0.7).round())
    };
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        // The bracket's corner, and the way its arms run from it.
        let corner = Point::new(middle.x + sx * from, middle.y + sy * from);
        let (dx, dy): (f32, f32) = if restore { (sx, sy) } else { (-sx, -sy) };
        let span = |at: f32, dir: f32, len: f32| {
            if dir > 0.0 {
                (at, len)
            } else {
                (at - len, len)
            }
        };
        let (x, w) = span(corner.x, dx, arm);
        let (y, h) = span(corner.y, dy, line);
        fill(out, x, y, w, h, ink);
        let (x, w) = span(corner.x, dx, line);
        let (y, h) = span(corner.y, dy, arm);
        fill(out, x, y, w, h, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Size;

    /// Pixels between the glyph and the box on the left, right, top
    /// and bottom.
    fn margins(side: i16) -> [i64; 4] {
        let size = f32::from(side);
        let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(size, size));
        let mut out = Vec::new();
        hamburger(&mut out, rect, (4.0, 1.0), Color::from_rgb8(255, 255, 255));
        assert_eq!(out.len(), 3, "three bars");
        let left = out.iter().map(|q| i64::from(q.x)).min().unwrap();
        let right = out
            .iter()
            .map(|q| i64::from(q.x) + i64::from(q.w))
            .max()
            .unwrap();
        let top = out.iter().map(|q| i64::from(q.y)).min().unwrap();
        let bottom = out
            .iter()
            .map(|q| i64::from(q.y) + i64::from(q.h))
            .max()
            .unwrap();
        let side = i64::from(side);
        [left - 10, 10 + side - right, top - 20, 20 + side - bottom]
    }

    #[test]
    fn the_hamburger_sits_in_the_middle_of_an_odd_box() {
        let [left, right, top, bottom] = margins(23);
        assert_eq!(left, right, "centered across");
        assert_eq!(top, bottom, "centered down");
    }

    #[test]
    fn the_hamburger_sits_in_the_middle_of_an_even_box_across() {
        let [left, right, top, bottom] = margins(24);
        assert_eq!(left, right, "centered across");
        // Three equal bars with equal gaps have an odd height, so an even
        // box leaves one pixel more on one side.
        assert!(top.abs_diff(bottom) <= 1, "{top} {bottom}");
    }
}
