//! The GRAPH tab's canvas as the window draws it, in place of the box
//! characters the terminal draws it with (`App::set_host_canvas`): the
//! same cards on the same grid of cells, with right-angled wires between them,
//! panned by any fraction of a cell and zoomed (`FlowState::zoom`). It is
//! drawn only where the terminal UI left the canvas blank
//! (`App::flow_canvas_clip`), so menus and overlays stay over it.
//!
//! Only what is on screen is drawn, found through the layout's indexes.
//! Text is set at sizes snapped to half a pixel and the shapes are few, so
//! zooming does not fill the atlas; zoomed far out, text is a bar.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

mod card;

use crate::geom::{rgba, Color, Point};
use crate::grid::{self, Metrics, Palette};
use crate::scene::{fill, Fill, Layers, Quad};
use crate::text::{Align, Face, Label, Shaping, TextSystem};
use aura_ui::events::TraceNodeStatus;
use aura_ui_terminal::flow_layout::{FlowLayout, Wire, WireKind, CARD_WIDTH};
use aura_ui_terminal::FlowState;
use ratatui::layout::Rect;
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Below this zoom, text is drawn as bars.
const GREEK_BELOW: f32 = 0.5;

/// The theme's colors the canvas is drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    fg: Color,
    bg: Color,
    muted: Color,
    primary: Color,
    secondary: Color,
    success: Color,
    warning: Color,
    error: Color,
    pending: Color,
    selection: Color,
    on_selection: Color,
}

impl Colors {
    pub fn new(theme: &aura_ui_terminal::Theme, palette: Palette) -> Self {
        let c = &theme.colors;
        let color = |color| grid::color(color, palette.fg);
        Self {
            fg: color(c.foreground),
            bg: palette.bg,
            muted: palette.muted,
            primary: palette.accent,
            secondary: color(c.secondary),
            success: color(c.success),
            warning: color(c.warning),
            error: color(c.error),
            pending: color(c.pending),
            selection: color(c.selection),
            on_selection: grid::color(c.on_selection, palette.bg),
        }
    }
}

/// The canvas of the tab in front, as its UI last rendered it.
pub struct Canvas<'a> {
    pub flow: &'a FlowState,
    /// The grid cells it is in
    pub area: Rect,
    /// The parts of `area` it is drawn in
    pub clip: &'a [Rect],
    pub colors: Colors,
}

impl Canvas<'_> {
    /// The canvas's quads, the grid's top-left corner at `origin`.
    #[allow(clippy::cast_possible_truncation)]
    pub fn quads(&self, text: &mut TextSystem, metrics: Metrics, origin: Point, out: &mut Layers) {
        let cell = metrics.cell;
        let zoom = self.flow.zoom();
        let mut drawn = Layers::default();
        Pen {
            canvas: self,
            layout: self.flow.layout(),
            text,
            metrics,
            out: &mut drawn,
            left: origin.x + f32::from(self.area.x) * cell.width,
            top: origin.y + f32::from(self.area.y) * cell.height,
            pan: self.flow.pan(),
            zoom,
            size: (cell.width * zoom, cell.height * zoom),
        }
        .draw();
        let edge = |at: f32, cells: u16, size: f32| (at + f32::from(cells) * size).round() as i32;
        let clip: Vec<[i32; 4]> = self
            .clip
            .iter()
            .map(|rect| {
                [
                    edge(origin.x, rect.x, cell.width),
                    edge(origin.y, rect.y, cell.height),
                    edge(origin.x, rect.right(), cell.width),
                    edge(origin.y, rect.bottom(), cell.height),
                ]
            })
            .collect();
        for (quads, into) in [(&drawn.under, &mut out.under), (&drawn.over, &mut out.over)] {
            for quad in quads {
                into.extend(clip.iter().filter_map(|rect| clipped(quad, *rect)));
            }
        }
    }
}

/// The part of `quad` inside `[left, top, right, bottom]`.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn clipped(quad: &Quad, [left, top, right, bottom]: [i32; 4]) -> Option<Quad> {
    let x0 = quad.x.max(left);
    let y0 = quad.y.max(top);
    let x1 = (quad.x + quad.w as i32).min(right);
    let y1 = (quad.y + quad.h as i32).min(bottom);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut part = *quad;
    (part.x, part.y) = (x0, y0);
    (part.w, part.h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    if quad.fill != Fill::Solid {
        part.u += (x0 - quad.x) as u32;
        part.v += (y0 - quad.y) as u32;
    }
    Some(part)
}

struct Pen<'a, 'b> {
    canvas: &'a Canvas<'a>,
    layout: &'a FlowLayout,
    text: &'b mut TextSystem,
    metrics: Metrics,
    out: &'b mut Layers,
    /// Pixel of the canvas area's left and top edges
    left: f32,
    top: f32,
    /// Canvas cell at the area's top left
    pan: (f32, f32),
    zoom: f32,
    /// Pixels of a canvas cell, across and down
    size: (f32, f32),
}

impl Pen<'_, '_> {
    fn colors(&self) -> Colors {
        self.canvas.colors
    }

    /// The pixel column of canvas column `x`, which may be a fraction.
    fn x(&self, x: f32) -> f32 {
        self.left + (x - self.pan.0) * self.size.0
    }

    fn y(&self, y: f32) -> f32 {
        self.top + (y - self.pan.1) * self.size.1
    }

    /// Canvas columns and rows on screen, with one to spare each side.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn shown(&self) -> (Range<usize>, Range<usize>) {
        let area = self.canvas.area;
        let first = |pan: f32| (pan.floor() - 1.0).max(0.0) as usize;
        let last = |pan: f32, cells: u16| {
            (pan + f32::from(cells) / self.zoom + 1.0).max(0.0).ceil() as usize
        };
        (
            first(self.pan.0)..last(self.pan.0, area.width),
            first(self.pan.1)..last(self.pan.1, area.height),
        )
    }

    /// Width of the lines wires and edges are drawn with, in whole pixels;
    /// the same for the selection's, which only changes colour.
    fn line(&self) -> f32 {
        (self.metrics.font_size * self.zoom / 14.0).round().max(1.0)
    }

    fn draw(&mut self) {
        let (xs, ys) = self.shown();
        self.names(&xs);
        let selected = self.canvas.flow.selected_card();
        let mut wires: Vec<(bool, Wire)> = self
            .layout
            .visible_wires(&xs, &ys)
            .into_iter()
            .map(|at| {
                let wire = self.layout.wires[at];
                (
                    Some(wire.from) == selected || Some(wire.to) == selected,
                    wire,
                )
            })
            .collect();
        wires.sort_by_key(|(hot, _)| *hot);
        for (hot, wire) in &wires {
            if self.layout.stacked(wire) {
                self.spine(*wire, *hot);
            } else {
                self.wire(*wire, *hot);
            }
        }
        let linked = self.canvas.flow.linked_cards();
        for card in self.layout.visible_cards(&xs, &ys) {
            self.card(card, selected, &linked);
        }
    }

    /// Each agent's name above its turns, kept on screen while any of
    /// them is, after the marker that folds it. The agent on screen goes
    /// unnamed; its name is the chat's.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn names(&mut self, xs: &Range<usize>) {
        let flow = self.canvas.flow;
        let color = self.colors().secondary;
        for span in self.layout.spans.iter().filter(|span| span.lane != 0) {
            let start = FlowLayout::column_x(span.first);
            let end = FlowLayout::column_x(span.last) + CARD_WIDTH;
            if end < xs.start || start > xs.end {
                continue;
            }
            let y = self.layout.bands[span.band].y;
            let marker = if flow.lane_is_folded(span.lane) {
                "▸"
            } else {
                "▾"
            };
            let name = format!("{marker} {}", flow.lanes()[span.lane].name);
            let name = fit(&name, end - start);
            let width = name.width() as f32;
            let (start, end) = (start as f32, end as f32);
            let x = (self.pan.0 + 1.0).clamp(start, (end - width).max(start));
            self.write(&name, (x, y as f32), color, true, Align::Left);
        }
    }

    /// Text from canvas point `(x, y)`, the top of a row; in the mono face
    /// a character is a cell wide, as in the terminal. Zoomed far out, a
    /// bar as long as the text.
    fn write(&mut self, text: &str, (x, y): (f32, f32), color: Color, bold: bool, align: Align) {
        let at = Point::new(self.x(x).round(), self.y(y).round());
        if self.zoom < GREEK_BELOW {
            let width = (text.width() as f32 * self.size.0).round();
            let height = (self.size.1 * 0.4).round().max(1.0);
            let left = match align {
                Align::Right => at.x - width,
                _ => at.x,
            };
            let top = at.y + ((self.size.1 - height) / 2.0).round();
            let color = color.mix(self.colors().bg, 0.4);
            fill(&mut self.out.over, left, top, width, height, color);
            return;
        }
        let label = Label {
            text,
            face: Face::mono(bold),
            size: (self.metrics.font_size * self.zoom * 2.0).round() / 2.0,
            line_height: self.size.1,
            shaping: Shaping::Advanced,
            align,
            middle: false,
            color: rgba(color),
            on_row: false,
        };
        self.text.write(&mut self.out.over, &label, at);
    }

    /// A wire's color: the selection's when it joins the selected card;
    /// for a wire giving an agent its work, how the work went once it
    /// came back; else its kind's.
    fn wire_color(&self, wire: &Wire, hot: bool) -> Color {
        let colors = self.colors();
        if hot {
            return colors.primary;
        }
        match self.canvas.flow.wire_status(wire) {
            Some(TraceNodeStatus::Ok) => return colors.success,
            Some(TraceNodeStatus::Error) => return colors.error,
            _ => {}
        }
        match wire.kind {
            WireKind::Next | WireKind::Spine => colors.muted.mix(colors.bg, 0.25),
            WireKind::Spawn | WireKind::Link => colors.secondary,
            WireKind::Return => colors.success,
            WireKind::Message => colors.warning,
        }
    }

    /// The pixel column a wire down canvas column `x` is drawn from.
    fn column_px(&self, x: usize) -> f32 {
        self.x(x as f32 + 0.5).round()
    }

    /// The pixel row a wire along canvas row `y` is drawn from.
    fn row_px(&self, y: usize) -> f32 {
        self.y(y as f32 + 0.5).round()
    }

    /// One wire: out of its port, along its row to the gutter before the
    /// block it goes to, square up or down there onto its card's row,
    /// and into its side (over the block's spine, for a subcard).
    fn wire(&mut self, wire: Wire, hot: bool) {
        let (from_x, from_y) = self.layout.port_out(wire.from, wire.row);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let gutter = FlowLayout::column_x(self.layout.cards[wire.to].column);
        if gutter < from_x + 3 {
            return;
        }
        let color = self.wire_color(&wire, hot);
        let width = self.line();
        let (start, end) = (self.column_px(from_x), self.column_px(to_x));
        let (sy, ey) = (self.row_px(from_y), self.row_px(to_y));
        if from_y == to_y {
            self.run(start, end, sy, width, color);
            return;
        }
        let turn = self
            .x(gutter as f32 - 1.5 - wire.track as f32 * 0.5)
            .round()
            .max(start);
        self.run(start, turn, sy, width, color);
        self.rise(turn, sy.min(ey), sy.max(ey), width, color);
        self.run(turn, end, ey, width, color);
    }

    /// A spine wire: down from the main card's bottom edge, then across
    /// into the subcard's side. The wires of one block share the spine.
    fn spine(&mut self, wire: Wire, hot: bool) {
        let main = self.layout.card_rect(wire.from);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let color = self.wire_color(&wire, hot);
        let width = self.line();
        let x = self.column_px(self.layout.spine_x(wire.from));
        let (top, ey) = (self.card_bottom(main), self.row_px(to_y));
        self.rise(x, top, ey, width, color);
        self.run(x, self.column_px(to_x), ey, width, color);
    }

    /// A straight stretch of wire from pixel column `x0` to `x1`, on the
    /// line drawn from pixel row `y`.
    fn run(&mut self, x0: f32, x1: f32, y: f32, width: f32, color: Color) {
        if x1 > x0 {
            let top = y - (width / 2.0).floor();
            let left = x0 - (width / 2.0).floor();
            fill(
                &mut self.out.under,
                left,
                top,
                x1 - x0 + width,
                width,
                color,
            );
        }
    }

    /// A straight stretch of wire from pixel row `y0` down to `y1`, on
    /// the line drawn from pixel column `x`.
    fn rise(&mut self, x: f32, y0: f32, y1: f32, width: f32, color: Color) {
        if y1 > y0 {
            let left = x - (width / 2.0).floor();
            let top = y0 - (width / 2.0).floor();
            fill(
                &mut self.out.under,
                left,
                top,
                width,
                y1 - y0 + width,
                color,
            );
        }
    }
}

/// `text` in at most `width` columns, cut short with `…`.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests;
