//! The title bars as quads.

use super::glyphs::{computer, corners, hamburger, plug, wrench};
use super::{BarLayout, Boxed, Chrome, Hit, Info, Kind, SIDE};
use crate::geom::{rgba, Color, Point, Rectangle, Size};
use crate::grid::Palette;
use crate::scene::{fill, Layers, Quad};
use crate::tab_bar::{
    label_size,
    quads::{cross, outline},
    unit,
};
use crate::text::{Align, Face, Label, Shaping, TextSystem, LABEL_LINE_EM};
use aura_ui_terminal::{AgentsTab, PaneTab, SideScope};
use unicode_width::UnicodeWidthStr;

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
    /// What the bars are drawn with, the grid starting `top` pixels down
    /// the window; `None` before there is a model.
    fn pen(&self, top: f32) -> Option<Pen> {
        let model = self.model.as_ref()?;
        let size = label_size(&model.metrics);
        Some(Pen {
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
        })
    }

    /// The bars' quads, the grid starting `top` pixels down the
    /// window.
    pub fn quads(&self, text: &mut TextSystem, out: &mut Layers, top: f32) {
        let Some(pen) = self.pen(top) else {
            return;
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
            for icon in [bar.lead_icon_tab, bar.icon_tab].into_iter().flatten() {
                self.icon_tab_quads(&pen, bar, icon, out);
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
        let lit = (filled, over, bar.focused);
        let ink = tab_box(&mut out.under, rect, lit, &palette, line);
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

    /// The box of the tab drawn as its icon, and the icon.
    fn icon_tab_quads(
        &self,
        pen: &Pen,
        bar: &BarLayout,
        icon: (Rectangle, PaneTab),
        out: &mut Layers,
    ) {
        let (mut rect, tab) = icon;
        rect.y += pen.top;
        let lit = (
            bar.active == Some(tab),
            self.hover == Some(Hit::Tab(tab)),
            bar.focused,
        );
        let ink = tab_box(&mut out.under, rect, lit, &pen.palette, pen.line);
        // As wide as the window boxes' glyphs: their arm each way.
        let arm = (pen.unit * 0.55).round();
        let glyph = match tab {
            PaneTab::Agents(AgentsTab::Bench) => wrench,
            PaneTab::Agents(AgentsTab::Integrations) => plug,
            // HOST's computer, the side panel's icon tab.
            _ => computer,
        };
        glyph(&mut out.under, rect, (arm, pen.line), ink);
    }

    /// The name of the tab drawn as an icon under the pointer, in a box
    /// under the icon's, their right edges lined up. It covers the grid
    /// below the bar, so it goes in `out`, a layer painted after the rest
    /// of the frame.
    pub fn tooltip_quads(&self, text: &mut TextSystem, out: &mut Layers, top: f32) {
        let Some(Hit::Tab(tab)) = self.hover else {
            return;
        };
        let Some(name) = tab.tooltip() else {
            return;
        };
        let icon = self.layout.bars.iter().find_map(|bar| {
            [bar.icon_tab, bar.lead_icon_tab]
                .into_iter()
                .flatten()
                .find(|&(_, icon)| icon == tab)
        });
        let (Some((icon, _)), Some(pen)) = (icon, self.pen(top)) else {
            return;
        };
        let width = ((name.width() as f32 + 2.0 * SIDE) * pen.unit).round();
        let gap = (pen.unit * 0.4).round().max(2.0);
        let rect = Rectangle::new(
            Point::new(
                (icon.x + icon.width - width).max(0.0),
                icon.y + icon.height + gap + pen.top,
            ),
            Size::new(width, icon.height),
        );
        let palette = pen.palette;
        fill(
            &mut out.under,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            palette.bg,
        );
        outline(&mut out.under, rect, palette.muted, pen.line);
        let label = Label {
            text: name,
            color: rgba(palette.fg),
            ..pen.plain
        };
        let middle = Point::new(rect.center_x(), rect.center_y());
        text.write(&mut out.over, &label, middle);
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

    /// The hamburger glyph under the window's close box, drawn bare with
    /// no outline around its box; the pointer only brightens its ink.
    fn side_toggle_quads(&self, pen: &Pen, mut rect: Rectangle, out: &mut Layers) {
        rect.y += pen.top;
        let (_, ink) = self.box_colors(pen, Hit::SideToggle);
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
