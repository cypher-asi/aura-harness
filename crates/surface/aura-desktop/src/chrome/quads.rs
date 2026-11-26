//! The title bars as quads.

use super::{BarLayout, Boxed, Chrome, Hit, Info, Kind};
use crate::geom::{rgba, Color, Point, Rectangle};
use crate::grid::Palette;
use crate::scene::{fill, Layers, Quad};
use crate::tab_bar::{
    label_size,
    quads::{cross, outline},
    unit,
};
use crate::text::{Align, Face, Label, Shaping, TextSystem, LABEL_LINE_EM};
use aura_ui_terminal::SideScope;

/// What every section of the bars is drawn with.
struct Pen {
    palette: Palette,
    /// Stroke thickness
    line: f32,
    /// Size of the glyphs in the window boxes
    unit: f32,
    /// How far down the window the grid starts
    top: f32,
    plain: Label<'static>,
}

impl Chrome {
    /// The bars' quads, the grid starting `top` pixels down the
    /// window.
    pub fn quads(&self, text: &mut TextSystem, out: &mut Layers, top: f32) {
        let Some(model) = &self.model else {
            return;
        };
        let size = label_size(&model.metrics);
        let pen = Pen {
            palette: model.palette,
            line: (model.metrics.font_size / 14.0).round().max(1.0),
            unit: unit(&model.metrics),
            top,
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
        for bar in &self.layout.bars {
            for boxed in &bar.boxes {
                self.label_box(&pen, bar, boxed, text, out);
            }
            if let Some(info) = &bar.info {
                info_quads(&pen, info, text, out);
            }
            if let Some(rect) = bar.close {
                self.close_quads(&pen, bar, rect, text, out);
            }
            if let Some(scope) = bar.scope {
                self.scope_quads(&pen, bar, scope, text, out);
            }
            if let Some(expand) = bar.expand {
                self.expand_quads(&pen, bar, expand, out);
            }
        }
        if let Some(rect) = self.layout.side_toggle {
            self.side_toggle_quads(&pen, rect, out);
        }
    }

    /// A title or tab box and its label.
    fn label_box(
        &self,
        pen: &Pen,
        bar: &BarLayout,
        boxed: &Boxed,
        text: &mut TextSystem,
        out: &mut Layers,
    ) {
        let (palette, line) = (pen.palette, pen.line);
        let mut rect = boxed.rect;
        rect.y += pen.top;
        let filled = match boxed.kind {
            Kind::Title => true,
            Kind::Tab(tab) => bar.active == Some(tab),
        };
        let over = matches!(boxed.kind, Kind::Tab(tab) if self.hover == Some(Hit::Tab(tab)));
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
            ..pen.plain
        };
        text.write(&mut out.over, &label, middle);
    }

    /// The edge and ink of a window-style box, brighter while `hit` is
    /// under the pointer.
    fn box_colors(&self, pen: &Pen, hit: Hit) -> (Color, Color) {
        if self.hover == Some(hit) {
            (pen.palette.muted, pen.palette.fg)
        } else {
            (pen.palette.faint, pen.palette.muted)
        }
    }

    /// The close box and its cross.
    fn close_quads(
        &self,
        pen: &Pen,
        bar: &BarLayout,
        mut rect: Rectangle,
        text: &mut TextSystem,
        out: &mut Layers,
    ) {
        rect.y += pen.top;
        let (edge, ink) = self.box_colors(pen, Hit::Close(bar.pane));
        outline(&mut out.under, rect, edge, pen.line);
        let arm = (pen.unit * 0.55).round();
        let middle = Point::new(rect.center_x(), rect.center_y());
        cross(text, out, middle, (arm, pen.line), ink);
    }

    /// The `*` box and its label.
    fn scope_quads(
        &self,
        pen: &Pen,
        bar: &BarLayout,
        scope: (Rectangle, bool),
        text: &mut TextSystem,
        out: &mut Layers,
    ) {
        let (palette, line) = (pen.palette, pen.line);
        let (mut rect, on) = scope;
        rect.y += pen.top;
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
            ..pen.plain
        };
        text.write(
            &mut out.over,
            &label,
            Point::new(rect.center_x(), rect.center_y()),
        );
    }

    /// The expand box and its corner brackets.
    fn expand_quads(
        &self,
        pen: &Pen,
        bar: &BarLayout,
        expand: (Rectangle, bool),
        out: &mut Layers,
    ) {
        let (mut rect, expanded) = expand;
        rect.y += pen.top;
        let (edge, ink) = self.box_colors(pen, Hit::Expand(bar.pane));
        outline(&mut out.under, rect, edge, pen.line);
        let reach = (pen.unit * 0.6).round();
        let middle = Point::new(rect.center_x().round(), rect.center_y().round());
        corners(&mut out.under, middle, (reach, pen.line), expanded, ink);
    }

    /// The hamburger box under the window's close box.
    fn side_toggle_quads(&self, pen: &Pen, mut rect: Rectangle, out: &mut Layers) {
        rect.y += pen.top;
        let (edge, ink) = self.box_colors(pen, Hit::SideToggle);
        outline(&mut out.under, rect, edge, pen.line);
        // As wide as the window boxes' glyphs: their arm each way.
        let arm = (pen.unit * 0.55).round();
        hamburger(&mut out.under, rect, (arm, pen.line), ink);
    }
}

/// The panel's info, right-aligned at the bar's end.
fn info_quads(pen: &Pen, info: &Info, text: &mut TextSystem, out: &mut Layers) {
    let end = Point::new(info.end.x, info.end.y + pen.top);
    let label = Label {
        text: &info.text,
        align: Align::Right,
        color: rgba(info.color),
        ..pen.plain
    };
    text.write(&mut out.over, &label, end);
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
