//! The title bars as quads.

use super::{Chrome, Hit, Kind};
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
                let lit = (filled, over, bar.focused);
                let ink = tab_box(&mut out.under, rect, lit, &palette, line);
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
            if let Some((mut rect, tab)) = bar.icon_tab {
                rect.y += top;
                let lit = (
                    bar.active == Some(tab),
                    self.hover == Some(Hit::Tab(tab)),
                    bar.focused,
                );
                let ink = tab_box(&mut out.under, rect, lit, &palette, line);
                // As wide as the window boxes' glyphs: their arm each way.
                let arm = (unit(&model.metrics) * 0.55).round();
                computer(&mut out.under, rect, (arm, line), ink);
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

/// A tab's box: filled with the selection while the tab is shown and its
/// bar has focus, otherwise outlined, brighter under the pointer. Gives
/// the ink of what is drawn in it.
fn tab_box(
    out: &mut Vec<Quad>,
    rect: Rectangle,
    (shown, over, focused): (bool, bool, bool),
    palette: &Palette,
    line: f32,
) -> Color {
    if shown && focused {
        fill(
            out,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            palette.selection,
        );
        palette.fg
    } else if shown {
        outline(out, rect, palette.faint, line);
        palette.fg
    } else if over {
        outline(out, rect, palette.muted, line);
        palette.fg
    } else {
        outline(out, rect, palette.faint, line);
        palette.muted
    }
}

/// A monitor on a stand centered in `rect`: a screen `2 * arm + line`
/// pixels wide framed in strokes `line` pixels thick, a neck under its
/// middle and a base under that. Laid out from the box's edges like
/// [`hamburger`], its width and height take the box's parity, so its
/// margins match across and down.
fn computer(out: &mut Vec<Quad>, rect: Rectangle, size: (f32, f32), ink: Color) {
    let (arm, line) = size;
    let mut width = 2.0 * arm + line;
    if (rect.width - width).rem_euclid(2.0) != 0.0 {
        width -= 1.0;
    }
    let mut screen = (width * 0.7).round().max(3.0 * line);
    let neck = (arm * 0.3).round().max(line);
    if (rect.height - (screen + neck + line)).rem_euclid(2.0) != 0.0 {
        screen -= 1.0;
    }
    let left = rect.x + ((rect.width - width) / 2.0).floor();
    let top = rect.y + ((rect.height - (screen + neck + line)) / 2.0).floor();
    fill(out, left, top, width, line, ink);
    fill(out, left, top + screen - line, width, line, ink);
    fill(out, left, top, line, screen, ink);
    fill(out, left + width - line, top, line, screen, ink);
    // Stand pieces as wide as asked, or a pixel wider to center them.
    let centered = |wanted: f32| wanted + (width - wanted).rem_euclid(2.0);
    let (stem, base) = (centered(line), centered((width * 0.6).round()));
    let (neck_y, base_y) = (top + screen, top + screen + neck);
    fill(out, left + (width - stem) / 2.0, neck_y, stem, neck, ink);
    fill(out, left + (width - base) / 2.0, base_y, base, line, ink);
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

    fn ink() -> Color {
        Color::from_rgb8(255, 255, 255)
    }

    /// Pixels between the glyph `draw` puts in a box `side` pixels wide
    /// and tall and the box, on the left, right, top and bottom.
    fn margins_of(side: i16, draw: impl Fn(&mut Vec<Quad>, Rectangle)) -> [i64; 4] {
        let size = f32::from(side);
        let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(size, size));
        let mut out = Vec::new();
        draw(&mut out, rect);
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

    fn margins(side: i16) -> [i64; 4] {
        margins_of(side, |out, rect| {
            hamburger(out, rect, (4.0, 1.0), ink());
            assert_eq!(out.len(), 3, "three bars");
        })
    }

    #[test]
    fn the_computer_sits_in_the_middle_of_its_box_across_and_down() {
        for side in [22, 23, 24, 25] {
            for arm in [4.0, 5.0] {
                let [left, right, top, bottom] = margins_of(side, |out, rect| {
                    computer(out, rect, (arm, 1.0), ink());
                    assert_eq!(out.len(), 6, "the screen's four sides, a neck, a base");
                });
                assert_eq!(left, right, "centered across a box of {side}");
                assert_eq!(top, bottom, "centered down a box of {side}");
                assert!(left > 0 && top > 0, "inside a box of {side}");
            }
        }
    }

    #[test]
    fn the_computers_stand_is_centered_under_its_screen() {
        for side in [23_u8, 24] {
            let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(f32::from(side), 23.0));
            let mut out = Vec::new();
            computer(&mut out, rect, (5.0, 1.0), ink());
            let middle = |q: &Quad| 2 * i64::from(q.x) + i64::from(q.w);
            let screen = middle(&out[0]);
            for piece in &out[4..] {
                assert_eq!(middle(piece), screen, "{piece:?} under {:?}", out[0]);
            }
        }
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
