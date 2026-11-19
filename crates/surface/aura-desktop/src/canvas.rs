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

use crate::geom::{rgba, Color, Point};
use crate::grid::{self, Metrics, Palette};
use crate::scene::{fill, Fill, Layers, Quad};
use crate::shapes::{self, Outline};
use crate::text::{Align, Face, Label, ShapeKey, Shaping, TextSystem};
use aura_ui::events::{FlowKind, TraceNodeStatus};
use aura_ui_terminal::flow_layout::{CellRect, FlowLayout, Wire, WireKind, CARD_WIDTH};
use aura_ui_terminal::FlowState;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The canvas's shapes among those in the atlas: past the last character
/// and the tab bar's cross.
const PORT: u32 = 0x0012_0003;

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
        for (hot, wire) in &wires {
            if !self.layout.stacked(wire) {
                self.port(*wire, *hot);
            }
        }
    }

    /// Each agent's name above its turns, kept on screen while any of
    /// them is. The agent on screen goes unnamed; its name is the chat's.
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
            let name = fit(&flow.lanes()[span.lane].name, end - start);
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

    /// The dot a wire leaves its card from.
    fn port(&mut self, wire: Wire, hot: bool) {
        let (x, y) = self.layout.port_out(wire.from, wire.row);
        let color = self.wire_color(&wire, hot);
        let width = self.line();
        let middle = width / 2.0 - (width / 2.0).floor();
        let center = Point::new(self.column_px(x) + middle, self.row_px(y) + middle);
        self.dot(center, color);
    }

    /// A port's dot, centered on pixel `center`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn dot(&mut self, center: Point, color: Color) {
        let radius = ((self.size.1 * 0.18).max(1.5) * 2.0).round() / 2.0;
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
        let edge = self.line();
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

        self.title(card, rect);
        self.report(card, rect);

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
        let label = fit(&node.label, rect.width - 4);
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

    /// `↩ reported to Agent`: over the bottom edge of the card at `rect`
    /// an agent reported back from, to whom and how it went.
    fn report(&mut self, card: usize, rect: CellRect) {
        let Some((label, status)) = self.canvas.flow.card_report(card) else {
            return;
        };
        let colors = self.colors();
        let label = fit(&label, rect.width.saturating_sub(6));
        if label.is_empty() {
            return;
        }
        let color = if status == TraceNodeStatus::Error {
            colors.error
        } else {
            colors.success
        };
        let right = rect.right() as f32 - 3.0;
        let left = right - label.width() as f32 - 1.0;
        let y = rect.bottom() as f32 - 1.0;
        let (top, bottom) = (self.y(y).round(), self.y(y + 1.0).round());
        let (x0, x1) = (self.x(left).round(), self.x(right + 1.0).round());
        fill(
            &mut self.out.under,
            x0,
            top,
            x1 - x0,
            bottom - top,
            colors.bg,
        );
        self.write(&label, (right, y), color, false, Align::Right);
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

    /// `▾ ✓ model ······ 1.2k/300 · 4s`: fold marker (on cards that
    /// fold), status, kind, and the card's facts against the right edge,
    /// on the title row of the card at `rect`.
    fn title(&mut self, card: usize, rect: CellRect) {
        let colors = self.colors();
        let flow = self.canvas.flow;
        let placed = &self.layout.cards[card];
        let node = &flow.nodes()[placed.head];
        let (x1, y) = (rect.right() as f32, rect.y as f32);
        let mut at = rect.x as f32 + 2.0;
        if placed.folds() {
            self.fold_button(at, y);
            let marker = if placed.folded { "▸" } else { "▾" };
            self.write(marker, (at, y), colors.secondary, false, Align::Left);
            at += 2.0;
        }
        let (glyph, color) = status_glyph(node.status, colors);
        self.write(glyph, (at, y), color, false, Align::Left);
        at += 2.0;
        let kind = flow.card_kind_label(card);
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

    /// The square a fold marker at canvas cell `(x, y)` sits on, over the
    /// cells a click on it folds from.
    fn fold_button(&mut self, x: f32, y: f32) {
        if self.zoom < GREEK_BELOW {
            return;
        }
        let colors = self.colors();
        let (left, right) = (self.x(x - 0.6).round(), self.x(x + 1.6).round());
        let (top, bottom) = (self.y(y + 0.05).round(), self.y(y + 0.95).round());
        let edge = self.line();
        let face = colors.muted.mix(colors.bg, 0.85);
        let border = colors.muted.mix(colors.bg, 0.45);
        let out = &mut self.out.under;
        fill(out, left, top, right - left, bottom - top, border);
        fill(
            out,
            left + edge,
            top + edge,
            right - left - 2.0 * edge,
            bottom - top - 2.0 * edge,
            face,
        );
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
        let (glyph, glyph_color) = match node.kind {
            FlowKind::Reply => ("↳", colors.secondary),
            _ => status_glyph(node.status, colors),
        };
        let glyph_color = if selected { text } else { glyph_color };
        let x = rect.x as f32;
        self.write(glyph, (x + 2.0, y), glyph_color, false, Align::Left);
        let room = rect.width - 4;
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
        FlowKind::Gate | FlowKind::Judge | FlowKind::Title | FlowKind::Event => colors.muted,
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
