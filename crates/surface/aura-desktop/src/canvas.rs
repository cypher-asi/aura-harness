//! The GRAPH tab's canvas as the window draws it, in place of the box
//! characters the terminal draws it with (`App::set_host_canvas`): the
//! same cards on the same grid of cells, with curved wires between them,
//! panned by any fraction of a cell. It is drawn only where the terminal
//! UI left the canvas blank (`App::flow_canvas_clip`), so menus and
//! overlays stay over it.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

use crate::geom::{rgba, Color, Point};
use crate::grid::{self, Metrics, Palette};
use crate::scene::{fill, Fill, Layers, Quad};
use crate::shapes::{self, LineCap, LineJoin, Outline};
use crate::text::{Align, Face, Label, ShapeKey, Shaping, TextSystem};
use aura_ui::events::{FlowKind, TraceNodeStatus};
use aura_ui_terminal::flow_layout::{CellRect, FlowLayout, Wire, WireKind, CARD_WIDTH};
use aura_ui_terminal::FlowState;
use ratatui::layout::Rect;
use std::collections::HashSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The canvas's shapes among those in the atlas: past the last character
/// and the tab bar's cross.
const PORT: u32 = 0x0012_0003;
/// Plus 1 for a wire that goes down
const BEND: u32 = 0x0012_0010;

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

/// How a card stands to the selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Selected,
    /// A wire joins it to the selected card
    Linked,
    Other,
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
}

impl Pen<'_, '_> {
    fn colors(&self) -> Colors {
        self.canvas.colors
    }

    /// The pixel column of canvas column `x`, which may be a fraction.
    fn x(&self, x: f32) -> f32 {
        self.left + (x - self.pan.0) * self.metrics.cell.width
    }

    fn y(&self, y: f32) -> f32 {
        self.top + (y - self.pan.1) * self.metrics.cell.height
    }

    /// Canvas cells on screen, with one to spare each side.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn shown(&self) -> ((usize, usize), (usize, usize)) {
        let area = self.canvas.area;
        let first = |pan: f32| (pan.floor() - 1.0).max(0.0) as usize;
        let last = |pan: f32, cells: u16| (pan + f32::from(cells) + 1.0).max(0.0).ceil() as usize;
        (
            (first(self.pan.0), last(self.pan.0, area.width)),
            (first(self.pan.1), last(self.pan.1, area.height)),
        )
    }

    /// Width of the lines wires and edges are drawn with.
    fn line(&self) -> f32 {
        (self.metrics.font_size / 14.0).round().max(1.0)
    }

    fn draw(&mut self) {
        self.names();
        let selected = self.canvas.flow.selected_card();
        let mut wires: Vec<(bool, Wire)> = self
            .layout
            .wires
            .iter()
            .map(|wire| {
                (
                    Some(wire.from) == selected || Some(wire.to) == selected,
                    *wire,
                )
            })
            .collect();
        wires.sort_by_key(|(hot, _)| *hot);
        for (hot, wire) in &wires {
            self.wire(*wire, *hot);
        }
        let linked = self.canvas.flow.linked_cards();
        for card in 0..self.layout.cards.len() {
            self.card(card, selected, &linked);
        }
        for (hot, wire) in &wires {
            if self.layout.stacked(wire) {
                self.drop(*wire, *hot);
            } else {
                self.port(*wire, *hot);
            }
        }
    }

    /// Each agent's name above its cards, kept on screen while any of
    /// them is.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn names(&mut self) {
        let flow = self.canvas.flow;
        for span in &self.layout.spans {
            let y = self.layout.bands[span.band].y;
            let start = FlowLayout::column_x(span.first) as f32;
            let end = (FlowLayout::column_x(span.last) + CARD_WIDTH) as f32;
            let name = fit(&flow.lanes()[span.lane].name, (end - start) as usize);
            let width = name.width() as f32;
            let x = (self.pan.0 + 1.0).clamp(start, (end - width).max(start));
            let color = if span.lane == 0 {
                self.colors().primary
            } else {
                self.colors().secondary
            };
            self.write(&name, (x, y as f32), color, true, Align::Left);
        }
    }

    /// Text from canvas point `(x, y)`, the top of a row; in the mono face
    /// a character is a cell wide, as in the terminal.
    fn write(&mut self, text: &str, (x, y): (f32, f32), color: Color, bold: bool, align: Align) {
        let label = Label {
            text,
            face: Face::mono(bold),
            size: self.metrics.font_size,
            line_height: self.metrics.cell.height,
            shaping: Shaping::Advanced,
            align,
            middle: false,
            color: rgba(color),
            on_row: false,
        };
        let at = Point::new(self.x(x).round(), self.y(y).round());
        self.text.write(&mut self.out.over, &label, at);
    }

    fn wire_color(&self, kind: WireKind, hot: bool) -> Color {
        let colors = self.colors();
        if hot {
            return colors.primary;
        }
        match kind {
            WireKind::Next => colors.muted.mix(colors.bg, 0.25),
            WireKind::Spawn | WireKind::Link => colors.secondary,
            WireKind::Return => colors.success,
            WireKind::Message => colors.warning,
        }
    }

    /// The pixel row a wire along canvas row `y` is centered on, and how
    /// thick it is: whole pixels, so a bend is the same shape wherever
    /// the view is.
    fn row_line(&self, y: usize, hot: bool) -> (f32, f32) {
        let width = self.line() * if hot { 2.0 } else { 1.0 };
        let middle = self.y(y as f32 + 0.5).round();
        (middle, width)
    }

    /// One wire: out of its port, along its row, bending in the gutter
    /// before the card it goes to onto that card's row, and into its
    /// side.
    fn wire(&mut self, wire: Wire, hot: bool) {
        let (from_x, from_y) = self.layout.port_out(wire.from, wire.row);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let ((x0, x1), (y0, y1)) = self.shown();
        let off_screen = to_x < x0 || from_x > x1 || from_y.max(to_y) < y0 || from_y.min(to_y) > y1;
        if to_x < from_x + 3 || off_screen || self.layout.stacked(&wire) {
            return;
        }
        let color = self.wire_color(wire.kind, hot);
        let cell = self.metrics.cell;
        let (sy, width) = self.row_line(from_y, hot);
        let (ey, _) = self.row_line(to_y, hot);
        let start = self.x(from_x as f32 + 0.5).round();
        let end = self.x(to_x as f32 + 0.5).round();
        if from_y == to_y {
            self.run(start, end, sy, width, color);
        } else {
            let bend_end = self
                .x(to_x as f32 - 1.0 - wire.track as f32 * 0.5)
                .round()
                .min(end);
            let reach = (4.0 * cell.width).max((ey - sy).abs() * 0.6);
            let bend_start = (bend_end - reach).max(start);
            self.run(start, bend_start, sy, width, color);
            self.bend((bend_start, sy), (bend_end, ey), width, color);
            self.run(bend_end, end, ey, width, color);
        }
    }

    /// A wire straight down from the bottom edge of a card to the top
    /// edge of the card under it, with a port where it leaves.
    fn drop(&mut self, wire: Wire, hot: bool) {
        let (from, to) = (
            self.layout.card_rect(wire.from),
            self.layout.card_rect(wire.to),
        );
        let ((_, _), (y0, y1)) = self.shown();
        if from.bottom() < y0 || to.y > y1 {
            return;
        }
        let color = self.wire_color(wire.kind, hot);
        let width = self.line() * if hot { 2.0 } else { 1.0 };
        let column = FlowLayout::stack_x(self.layout.cards[wire.from].column);
        let middle = self.x(column as f32 + 0.5).round();
        let (top, bottom) = (self.card_bottom(from), self.card_top(to));
        let left = middle - (width / 2.0).floor();
        fill(&mut self.out.under, left, top, width, bottom - top, color);
        let center = Point::new(left + width / 2.0, top);
        self.dot(center, color);
    }

    /// A straight stretch of wire from `x0` to `x1`, centered on pixel
    /// row `y`.
    fn run(&mut self, x0: f32, x1: f32, y: f32, width: f32, color: Color) {
        let top = y - (width / 2.0).floor();
        fill(&mut self.out.under, x0, top, x1 - x0, width, color);
    }

    /// An S from `from` to `to`, leaving and arriving level.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn bend(&mut self, from: (f32, f32), to: (f32, f32), width: f32, color: Color) {
        let (w, h) = (to.0 - from.0, (to.1 - from.1).abs());
        if w < 1.0 {
            return;
        }
        let down = to.1 > from.1;
        let pad = width.ceil() + 1.0;
        let key = ShapeKey {
            shape: BEND + u32::from(down),
            width: w as u32,
            height: h as u32,
            phase: [width as u8, 0],
        };
        // Pixel row `y` is centered where its top is plus half a line.
        let middle = (width / 2.0).floor();
        let center = |y: f32| y - middle + width / 2.0;
        let (a, b) = if down { (pad, pad + h) } else { (pad + h, pad) };
        let (a, b) = (center(a), center(b));
        let at = ((from.0 - pad) as i32, (from.1.min(to.1) - pad) as i32);
        let size = ((w + 2.0 * pad) as u32, (h + 2.0 * pad) as u32 + 1);
        let atlas = self.text.atlas_mut();
        shapes::stamp(
            &mut self.out.under,
            atlas,
            key,
            at,
            size,
            rgba(color),
            |pen| {
                let (p0, p3) = (Point::new(pad, a), Point::new(pad + w, b));
                let (p1, p2) = (Point::new(pad + w / 2.0, a), Point::new(pad + w / 2.0, b));
                let mut outline = Outline::new();
                outline.move_to(p0);
                for i in 1..=32u8 {
                    let t = f32::from(i) / 32.0;
                    let u = 1.0 - t;
                    let (c0, c1, c2, c3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    outline.line_to(Point::new(
                        c0 * p0.x + c1 * p1.x + c2 * p2.x + c3 * p3.x,
                        c0 * p0.y + c1 * p1.y + c2 * p2.y + c3 * p3.y,
                    ));
                }
                pen.stroke(outline, width, LineCap::Butt, LineJoin::Round);
            },
        );
    }

    /// The dot a wire leaves its card from.
    fn port(&mut self, wire: Wire, hot: bool) {
        let (x, y) = self.layout.port_out(wire.from, wire.row);
        let ((x0, x1), (y0, y1)) = self.shown();
        if !(x0..=x1).contains(&x) || !(y0..=y1).contains(&y) {
            return;
        }
        let color = self.wire_color(wire.kind, hot);
        let (middle, width) = self.row_line(y, hot);
        let center = Point::new(
            self.x(x as f32 + 0.5).round(),
            middle - (width / 2.0).floor() + width / 2.0,
        );
        self.dot(center, color);
    }

    /// A port's dot, centered on pixel `center`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn dot(&mut self, center: Point, color: Color) {
        let radius = (self.metrics.cell.height * 0.18).max(2.5);
        let size = (radius * 2.0).ceil() + 2.0;
        let key = ShapeKey {
            shape: PORT,
            width: radius.to_bits(),
            height: 0,
            phase: [shapes::phase(center.y), 0],
        };
        let at = (
            (center.x - size / 2.0).floor() as i32,
            (center.y - size / 2.0).floor() as i32,
        );
        let local = Point::new(center.x - at.0 as f32, center.y - at.1 as f32);
        let atlas = self.text.atlas_mut();
        shapes::stamp(
            &mut self.out.under,
            atlas,
            key,
            at,
            (size as u32 + 1, size as u32 + 1),
            rgba(color),
            |pen| pen.fill(Outline::circle(local, radius)),
        );
    }

    fn card(&mut self, card: usize, selected: Option<usize>, linked: &HashSet<usize>) {
        let rect = self.layout.card_rect(card);
        let ((x0, x1), (y0, y1)) = self.shown();
        if rect.right() < x0 || rect.x > x1 || rect.bottom() < y0 || rect.y > y1 {
            return;
        }
        let standing = if selected == Some(card) {
            Standing::Selected
        } else if linked.contains(&card) {
            Standing::Linked
        } else {
            Standing::Other
        };
        let colors = self.colors();
        let flow = self.canvas.flow;
        let placed = &self.layout.cards[card];
        let node = &flow.nodes()[placed.head];

        // From the middle of the side borders' cells, and from the top of
        // the title row to the middle of the bottom border's.
        let left = self.x(rect.x as f32 + 0.5).round();
        let right = self.x(rect.right() as f32 - 0.5).round();
        let (top, bottom) = (self.card_top(rect), self.card_bottom(rect));
        let edge = self.line()
            * if standing == Standing::Selected {
                2.0
            } else {
                1.0
            };
        let border = match standing {
            Standing::Selected => colors.primary,
            Standing::Linked => colors.secondary.mix(colors.bg, 0.3),
            Standing::Other => colors.muted.mix(colors.bg, 0.45),
        };
        let out = &mut self.out.under;
        fill(out, left, top, right - left, bottom - top, colors.bg);
        fill(out, left, top, right - left, edge, border);
        fill(out, left, bottom - edge, right - left, edge, border);
        fill(out, left, top, edge, bottom - top, border);
        fill(out, right - edge, top, edge, bottom - top, border);

        self.title(card, rect.x as f32, rect.right() as f32, rect.y as f32);

        let selected_place = flow.selected_place().filter(|(at, _)| *at == card);
        let head_selected = selected_place == Some((card, None));
        let line_top = |line: usize| rect.y as f32 + line as f32;
        if head_selected {
            self.highlight(left + edge, right - edge, line_top(1));
        }
        let head_color = if head_selected {
            colors.on_selection
        } else {
            colors.fg
        };
        let bold = head_selected || matches!(node.kind, FlowKind::Prompt | FlowKind::Reply);
        let label = fit(&node.label, CARD_WIDTH - 4);
        self.write(
            &label,
            (rect.x as f32 + 2.0, line_top(1)),
            head_color,
            bold,
            Align::Left,
        );

        let shown = placed.shown_rows();
        for row in 0..placed.lines() {
            let line = 2 + row;
            if row == shown {
                match flow.last_line_row(card) {
                    Some(hidden) => self.row(card, hidden, line, true, (left + edge, right - edge)),
                    None => {
                        let more = flow.more_label(card);
                        self.write(
                            &more,
                            (rect.x as f32 + 2.0, line_top(line)),
                            colors.muted,
                            false,
                            Align::Left,
                        );
                    }
                }
                continue;
            }
            let is_selected = selected_place == Some((card, Some(row)));
            self.row(card, row, line, is_selected, (left + edge, right - edge));
        }
    }

    /// The selection's fill across a card's line from canvas row `y`.
    fn highlight(&mut self, left: f32, right: f32, y: f32) {
        let (top, bottom) = (self.y(y).round(), self.y(y + 1.0).round());
        let color = self.colors().selection;
        fill(
            &mut self.out.under,
            left,
            top,
            right - left,
            bottom - top,
            color,
        );
    }

    /// The pixel row of the top edge of a card at `rect`.
    fn card_top(&self, rect: CellRect) -> f32 {
        self.y(rect.y as f32).round()
    }

    /// The pixel row of the bottom edge of a card at `rect`.
    fn card_bottom(&self, rect: CellRect) -> f32 {
        self.y(rect.bottom() as f32 - 0.5).round()
    }

    /// `▾ ✓ model ······ 1.2k/300 · 4s`: fold marker (turns only),
    /// status, kind, and the card's facts against the right edge, on the
    /// title row `y` of a card from column `x0` to `x1`.
    fn title(&mut self, card: usize, x0: f32, x1: f32, y: f32) {
        let colors = self.colors();
        let flow = self.canvas.flow;
        let placed = &self.layout.cards[card];
        let node = &flow.nodes()[placed.head];
        let mut at = x0 + 2.0;
        if placed.steps > 0 {
            let marker = if placed.folded { "▸" } else { "▾" };
            self.write(marker, (at, y), colors.secondary, false, Align::Left);
            at += 2.0;
        }
        let (glyph, color) = status_glyph(node.status, colors);
        self.write(glyph, (at, y), color, false, Align::Left);
        at += 2.0;
        let kind = node.kind.label();
        self.write(
            kind,
            (at, y),
            kind_color(node.kind, colors),
            true,
            Align::Left,
        );
        at += kind.width() as f32 + 1.0;
        let room = (x1 - 2.0 - at - 1.0).max(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let facts = fit(&flow.card_facts(card), room as usize);
        if !facts.is_empty() {
            self.write(&facts, (x1 - 2.0, y), colors.muted, false, Align::Right);
        }
    }

    /// `✓ read_file src/lib.rs          12ms`: a row's status, label, and
    /// its detail against the right edge, on line `line` of its card.
    fn row(&mut self, card: usize, row: usize, line: usize, selected: bool, fill: (f32, f32)) {
        let colors = self.colors();
        let rect = self.layout.card_rect(card);
        let node = &self.canvas.flow.nodes()[self.layout.cards[card].rows[row]];
        let y = (rect.y + line) as f32;
        if selected {
            self.highlight(fill.0, fill.1, y);
        }
        let text = if selected {
            colors.on_selection
        } else {
            match node.kind {
                FlowKind::Event => colors.muted,
                FlowKind::Spawn => colors.secondary,
                _ => colors.fg,
            }
        };
        let (glyph, glyph_color) = status_glyph(node.status, colors);
        let glyph_color = if selected { text } else { glyph_color };
        let x = rect.x as f32;
        self.write(glyph, (x + 2.0, y), glyph_color, false, Align::Left);
        let room = CARD_WIDTH - 4;
        let detail = fit(&node.detail, room / 3);
        let detail_width = detail.width();
        let label_room = room - 2 - detail_width - usize::from(detail_width > 0);
        let label = fit(&node.label, label_room);
        self.write(&label, (x + 4.0, y), text, selected, Align::Left);
        if detail_width > 0 {
            let color = if selected { text } else { colors.muted };
            self.write(
                &detail,
                (rect.right() as f32 - 2.0, y),
                color,
                false,
                Align::Right,
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

const fn status_glyph(status: TraceNodeStatus, colors: Colors) -> (&'static str, Color) {
    match status {
        TraceNodeStatus::Pending => ("●", colors.pending),
        TraceNodeStatus::Ok => ("✓", colors.success),
        TraceNodeStatus::Warn => ("!", colors.warning),
        TraceNodeStatus::Error => ("✗", colors.error),
    }
}

const fn kind_color(kind: FlowKind, colors: Colors) -> Color {
    match kind {
        FlowKind::Prompt => colors.primary,
        FlowKind::Model | FlowKind::Spawn | FlowKind::Message | FlowKind::Result => {
            colors.secondary
        }
        FlowKind::Event => colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => colors.fg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];

    #[test]
    fn a_quad_is_cut_to_the_clip_and_its_atlas_pixels_move_with_it() {
        let solid = Quad {
            x: 10,
            y: 20,
            w: 30,
            h: 10,
            color: RED,
            fill: Fill::Solid,
            u: 0,
            v: 0,
        };
        let part = clipped(&solid, [15, 0, 100, 25]).expect("overlaps");
        assert_eq!((part.x, part.y, part.w, part.h), (15, 20, 25, 5));
        assert_eq!((part.u, part.v), (0, 0));
        assert!(
            clipped(&solid, [40, 0, 100, 100]).is_none(),
            "touching only"
        );

        let glyph = Quad {
            fill: Fill::Mask,
            u: 100,
            v: 200,
            ..solid
        };
        let part = clipped(&glyph, [12, 23, 30, 100]).expect("overlaps");
        assert_eq!((part.x, part.y, part.w, part.h), (12, 23, 18, 7));
        assert_eq!((part.u, part.v), (102, 203));
    }

    #[test]
    fn text_that_does_not_fit_is_cut_short() {
        assert_eq!(fit("sonnet", 6), "sonnet");
        assert_eq!(fit("task coder · write greet()", 10), "task code…");
        assert_eq!(fit("日本語", 4), "日…");
        assert_eq!(fit("abc", 0), "");
    }
}
