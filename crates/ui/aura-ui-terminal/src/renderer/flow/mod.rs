//! The chat panel's GRAPH tab: what an agent and the agents it spawned
//! did, as cards wired together on a canvas, drawn with box characters.
//!
//! ```text
//!   ┌ ▾ ✓ turn ───────────── 2 steps ┐
//!   │ add a greeting                 │
//!   └─┬──────────────────────────────┘
//!     │ ┌ ▾ ✓ model ── 1.2k/300 · 4s ┐
//!     ├─┤ sonnet                     │
//!     │ │ ✓ task coder · write… 4.2s ├────┐
//!     │ └────────────────────────────┘    │ coder #1
//!     │ ┌ ● model ───────────────────┐    │ ┌ ✓ turn ────────────── coder #1 ┐
//!     └─┤ sonnet                     │    └─┤ write greet()                  │
//!       └────────────────────────────┘      │ ↳ done                         │
//! ```
//!
//! Only the part of the canvas on screen is drawn, so a long flow costs no
//! more per frame than a short one. A front end that draws the canvas
//! itself ([`crate::App::set_host_canvas`]) gets the cells left blank.

use super::chrome::{body_block, render_scrollbar, ScrollView};
use super::flow_minimap::render_minimap;
use super::flow_timeline::render_timeline;
use super::graph::status_glyph;
use super::side_list::render_empty;
use super::text::display_width;
use crate::app::flow::layout::{FlowLayout, Wire, WireKind, CARD_WIDTH};
use crate::app::flow::{FlowMode, FlowState, FlowView};
use crate::components::truncate_record_info;
use crate::events::{FlowKind, TraceNodeStatus};
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// Draw the GRAPH tab in `area`, the chat panel below its header bar.
pub(super) fn render_flow(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Nothing of the conversation is on screen to click or select.
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);

    app.flow_mut().set_room(usize::from(inner.height));
    app.flow_mut().ensure_layout();
    if inner.height < 3 || inner.width < 12 || app.flow().layout().cards.is_empty() {
        app.flow_mut().set_view(None, false);
        app.flow_mut().set_minimap(None);
        let text = if app.flow().loaded() {
            "Nothing recorded yet."
        } else {
            "Loading the graph…"
        };
        render_empty(frame, inner, text, theme);
        return;
    }

    if app.flow().mode() == FlowMode::Timeline {
        // Drawn in cells by every front end.
        app.flow_mut().set_view(None, false);
        app.flow_mut().set_minimap(None);
        render_timeline(frame, area, inner, app, theme, is_focused);
        return;
    }
    app.flow_mut().timeline_mut().set_view(None);

    let pan = app
        .flow_mut()
        .sync_view(f32::from(inner.width), f32::from(inner.height));
    let hosted = app.host_canvas();
    app.flow_mut()
        .set_view(Some(FlowView { area: inner, pan }), hosted);
    let first = (cell_of(pan.0), cell_of(pan.1));
    if hosted {
        let buffer = frame.buffer_mut();
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_skip(true);
                }
            }
        }
    } else {
        let flow = app.flow();
        Canvas {
            buffer: frame.buffer_mut(),
            theme,
            flow,
            layout: flow.layout(),
            area: inner,
            first,
        }
        .draw();
    }
    render_minimap(frame, inner, app, theme);

    let total = app.flow().layout().height;
    let visible = usize::from(inner.height);
    render_scrollbar(
        frame,
        area,
        inner,
        ScrollView {
            total,
            visible,
            first: whole(first.1).min(total.saturating_sub(visible)),
        },
        is_focused,
        theme,
    );
}

/// The cell a canvas position is in, left of or above the canvas too.
#[allow(clippy::cast_possible_truncation)]
fn cell_of(at: f32) -> i64 {
    at.floor() as i64
}

/// A canvas cell, or the canvas's first for one off its top or left.
fn whole(at: i64) -> usize {
    usize::try_from(at).unwrap_or(0)
}

/// How a card stands to the selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Selected,
    /// A wire joins it to the selected card
    Linked,
    Other,
}

/// What is needed to draw the part of the canvas that is on screen.
struct Canvas<'a> {
    buffer: &'a mut Buffer,
    theme: &'a Theme,
    flow: &'a FlowState,
    layout: &'a FlowLayout,
    area: Rect,
    /// Canvas cell at the top left of `area`, which may be off the canvas
    first: (i64, i64),
}

impl Canvas<'_> {
    /// The canvas columns and rows on screen.
    fn shown(&self) -> (Range<usize>, Range<usize>) {
        let span = |first: i64, cells: u16| whole(first)..whole(first + i64::from(cells));
        (
            span(self.first.0, self.area.width),
            span(self.first.1, self.area.height),
        )
    }

    /// The screen cell of canvas cell `(x, y)`, when it is on screen.
    fn screen(&self, x: usize, y: usize) -> Option<(u16, u16)> {
        let from = |at: usize, first: i64| u16::try_from(i64::try_from(at).ok()? - first).ok();
        let (dx, dy) = (from(x, self.first.0)?, from(y, self.first.1)?);
        (dx < self.area.width && dy < self.area.height)
            .then(|| (self.area.x + dx, self.area.y + dy))
    }

    fn put(&mut self, x: usize, y: usize, symbol: &str, style: Style) {
        if let Some(at) = self.screen(x, y) {
            if let Some(cell) = self.buffer.cell_mut(at) {
                cell.set_symbol(symbol).set_style(style);
            }
        }
    }

    fn symbol(&self, x: usize, y: usize) -> &str {
        self.screen(x, y)
            .and_then(|at| self.buffer.cell(at))
            .map_or("", |cell| cell.symbol())
    }

    /// `text` from canvas cell `(x, y)`, in at most `width` columns.
    fn text(&mut self, x: usize, y: usize, text: &str, width: usize, style: Style) {
        let mut at = 0;
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if at + w > width {
                break;
            }
            self.put(x + at, y, c.encode_utf8(&mut buf), style);
            if w == 2 {
                self.put(x + at + 1, y, " ", style);
            }
            at += w;
        }
    }

    fn muted(&self) -> Style {
        Style::default().fg(self.theme.colors.muted)
    }

    fn draw(&mut self) {
        self.names();
        let (xs, ys) = self.shown();
        let selected = self.flow.selected_card();
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
            if !self.layout.stacked(wire) {
                self.wire(*wire, self.wire_style(wire, *hot));
            }
        }
        let linked = self.flow.linked_cards();
        for card in self.layout.visible_cards(&xs, &ys) {
            let standing = if selected == Some(card) {
                Standing::Selected
            } else if linked.contains(&card) {
                Standing::Linked
            } else {
                Standing::Other
            };
            self.card(card, standing);
        }
        for (hot, wire) in &wires {
            let style = self.wire_style(wire, *hot);
            if self.layout.stacked(wire) {
                self.spine(*wire, style);
                continue;
            }
            let (x, y) = self.layout.port_out(wire.from, wire.row);
            self.join(x, y, RIGHT, style);
            let (x, y) = self.layout.port_in(wire.to);
            self.join(x, y, LEFT, style);
        }
    }

    /// Each agent's name above its cards, kept on screen while any of
    /// them is, after the marker that folds it. The agent on screen goes
    /// unnamed; its name is the chat's.
    fn names(&mut self) {
        let style = Style::default()
            .fg(self.theme.colors.secondary)
            .add_modifier(Modifier::BOLD);
        for span in self.layout.spans.iter().filter(|span| span.lane != 0) {
            let y = self.layout.bands[span.band].y;
            let start = FlowLayout::column_x(span.first);
            let end = FlowLayout::column_x(span.last) + CARD_WIDTH;
            let marker = if self.flow.lane_is_folded(span.lane) {
                "▸"
            } else {
                "▾"
            };
            let name = format!("{marker} {}", self.flow.lanes()[span.lane].name);
            let name = truncate_record_info(&name, end - start);
            let width = display_width(&name);
            let x = whole(self.first.0 + 1).clamp(start, end - width);
            self.text(x, y, &name, width, style);
        }
    }

    /// A wire's color: the selection's when it joins the selected card;
    /// for a wire giving an agent its work, how the work went once it
    /// came back; else its kind's.
    fn wire_style(&self, wire: &Wire, hot: bool) -> Style {
        if hot {
            return Style::default().fg(self.theme.colors.primary);
        }
        match self.flow.wire_status(wire) {
            Some(TraceNodeStatus::Ok) => return Style::default().fg(self.theme.colors.success),
            Some(TraceNodeStatus::Error) => return Style::default().fg(self.theme.colors.error),
            _ => {}
        }
        Style::default().fg(match wire.kind {
            WireKind::Next | WireKind::Spine => self.theme.colors.muted,
            WireKind::Spawn | WireKind::Link => self.theme.colors.secondary,
            WireKind::Return => self.theme.colors.success,
            WireKind::Message => self.theme.colors.warning,
        })
    }

    /// One wire: out of its port, along to the gutter before the block it
    /// goes to, up or down there, and into its card's side (over the
    /// block's spine, for a subcard).
    fn wire(&mut self, wire: Wire, style: Style) {
        let (from_x, from_y) = self.layout.port_out(wire.from, wire.row);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let gutter = FlowLayout::column_x(self.layout.cards[wire.to].column);
        if gutter < from_x + 3 {
            return;
        }
        if from_y == to_y {
            self.horizontal(from_x + 1..to_x, from_y, style);
        } else {
            let turn = (gutter - 2 - wire.track).max(from_x + 1);
            self.horizontal(from_x + 1..turn, from_y, style);
            let (top, bottom) = (from_y.min(to_y), from_y.max(to_y));
            for y in top + 1..bottom {
                self.join(turn, y, UP | DOWN, style);
            }
            let (leave, arrive) = if to_y > from_y {
                (LEFT | DOWN, UP | RIGHT)
            } else {
                (LEFT | UP, DOWN | RIGHT)
            };
            self.join(turn, from_y, leave, style);
            self.join(turn, to_y, arrive, style);
            self.horizontal(turn + 1..to_x, to_y, style);
        }
    }

    /// A spine wire: down from the main card's bottom border, then across
    /// into the subcard's side. The wires of one block share the spine,
    /// so it branches `├` at every subcard but the last.
    fn spine(&mut self, wire: Wire, style: Style) {
        let main = self.layout.card_rect(wire.from);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let x = self.layout.spine_x(wire.from);
        self.join(x, main.bottom() - 1, DOWN, style);
        for y in main.bottom()..to_y {
            self.join(x, y, UP | DOWN, style);
        }
        self.join(x, to_y, UP | RIGHT, style);
        self.horizontal(x + 1..to_x, to_y, style);
        self.join(to_x, to_y, LEFT, style);
    }

    fn horizontal(&mut self, xs: std::ops::Range<usize>, y: usize, style: Style) {
        for x in xs {
            self.join(x, y, LEFT | RIGHT, style);
        }
    }

    /// A piece of wire with `ends` at `(x, y)`, joined to any wire already
    /// there: `─` over `│` makes `┼`, over `┌` makes `┬`.
    fn join(&mut self, x: usize, y: usize, ends: u8, style: Style) {
        let ends = ends | ends_of(self.symbol(x, y));
        self.put(x, y, glyph_of(ends), style);
    }

    fn border(&self, standing: Standing) -> Style {
        match standing {
            Standing::Selected => Style::default().fg(self.theme.colors.primary),
            Standing::Linked => Style::default().fg(self.theme.colors.secondary),
            Standing::Other => self.muted(),
        }
    }

    fn selection(&self) -> Style {
        Style::default()
            .bg(self.theme.colors.selection)
            .fg(self.theme.colors.on_selection)
            .add_modifier(Modifier::BOLD)
    }

    fn card(&mut self, card: usize, standing: Standing) {
        let rect = self.layout.card_rect(card);
        let (xs, ys) = self.shown();
        let on_screen = rect.right() > xs.start
            && rect.x < xs.end
            && rect.bottom() > ys.start
            && rect.y < ys.end;
        if !on_screen {
            return;
        }
        let border = self.border(standing);
        let (x, y, w) = (rect.x, rect.y, rect.width);
        let bottom = rect.bottom() - 1;
        for line in y..=bottom {
            let (left, fill, right) = if line == y {
                ("┌", "─", "┐")
            } else if line == bottom {
                ("└", "─", "┘")
            } else {
                ("│", " ", "│")
            };
            self.put(x, line, left, border);
            for column in x + 1..x + w - 1 {
                self.put(
                    column,
                    line,
                    fill,
                    if fill == " " {
                        Style::default()
                    } else {
                        border
                    },
                );
            }
            self.put(x + w - 1, line, right, border);
        }
        self.title(card, border);
        self.report(card);

        let placed = &self.layout.cards[card];
        let selected = self.flow.selected_place().filter(|(at, _)| *at == card);
        let node = &self.flow.nodes()[placed.head];
        let head = if selected == Some((card, None)) {
            self.selection()
        } else {
            let style = Style::default().fg(self.theme.colors.foreground);
            match node.kind {
                FlowKind::Prompt | FlowKind::Reply => style.add_modifier(Modifier::BOLD),
                _ => style,
            }
        };
        self.line_fill(card, 1, head);
        self.text(x + 2, y + 1, &node.label, w - 4, head);
        if let Some((tokens, spent)) = self.flow.card_stats(card) {
            let style = Style::default().fg(self.theme.colors.secondary);
            let spent = truncate_record_info(&spent, w - 4);
            let spent_width = display_width(&spent);
            self.text(x + w - 2 - spent_width, y + 2, &spent, spent_width, style);
            let room = (w - 4).saturating_sub(spent_width + 1);
            self.text(x + 2, y + 2, &tokens, room, style);
        }

        let shown = placed.shown_rows();
        for row in 0..placed.lines() {
            let line = placed.first_row_line() + row;
            if row == shown {
                // The last line of a card with more rows than lines.
                match self.flow.last_line_row(card) {
                    Some(hidden) => self.row(card, hidden, line, true),
                    None => {
                        let more = self.flow.more_label(card);
                        self.text(x + 2, y + line, &more, w - 4, self.muted());
                    }
                }
                continue;
            }
            let is_selected = selected == Some((card, Some(row)));
            self.row(card, row, line, is_selected);
        }
    }

    /// `┌ ▾ ✓ model ─────── 1.2k/300 · 4s ┐`: fold marker (on cards that
    /// fold), status, kind, and the card's facts against the right corner.
    fn title(&mut self, card: usize, border: Style) {
        let rect = self.layout.card_rect(card);
        let placed = &self.layout.cards[card];
        let node = &self.flow.nodes()[placed.head];
        let (glyph, glyph_color) = status_glyph(node.status, self.theme);
        let mut parts: Vec<(&str, Style)> = Vec::new();
        if placed.folds() {
            let marker = if placed.folded { "▸" } else { "▾" };
            parts.push((marker, Style::default().fg(self.theme.colors.secondary)));
        }
        parts.push((glyph, Style::default().fg(glyph_color)));
        parts.push((
            self.flow.card_kind_label(card),
            Style::default()
                .fg(kind_color(node.kind, self.theme))
                .add_modifier(Modifier::BOLD),
        ));
        let end = rect.right() - 1;
        let mut at = rect.x + 1;
        self.put(at, rect.y, " ", border);
        at += 1;
        for (text, style) in parts {
            let width = display_width(text);
            if at + width + 1 >= end {
                break;
            }
            self.text(at, rect.y, text, width, style);
            self.put(at + width, rect.y, " ", border);
            at += width + 1;
        }
        let room = end.saturating_sub(at + 1);
        let facts = truncate_record_info(&self.flow.card_facts(card), room.saturating_sub(2));
        if room >= 5 && !facts.is_empty() {
            let width = display_width(&facts);
            let start = end - width - 2;
            self.put(start, rect.y, " ", self.muted());
            self.text(start + 1, rect.y, &facts, width, self.muted());
            self.put(start + 1 + width, rect.y, " ", self.muted());
        }
    }

    /// `└─ ↩ reported to Agent ─┘`: on the bottom border of the card an
    /// agent reported back from, to whom and how it went.
    fn report(&mut self, card: usize) {
        let Some((label, status)) = self.flow.card_report(card) else {
            return;
        };
        let rect = self.layout.card_rect(card);
        let color = if status == TraceNodeStatus::Error {
            self.theme.colors.error
        } else {
            self.theme.colors.success
        };
        let room = rect.width.saturating_sub(6);
        let label = truncate_record_info(&label, room);
        let width = display_width(&label);
        if width == 0 {
            return;
        }
        let y = rect.bottom() - 1;
        let start = rect.right() - 3 - width;
        self.put(start - 1, y, " ", self.muted());
        self.text(start, y, &label, width, Style::default().fg(color));
        self.put(start + width, y, " ", self.muted());
    }

    /// Paint line `line` of card `card`, inside its borders, in `style`.
    fn line_fill(&mut self, card: usize, line: usize, style: Style) {
        let rect = self.layout.card_rect(card);
        for x in rect.x + 1..rect.right() - 1 {
            self.put(x, rect.y + line, " ", style);
        }
    }

    /// `✓ read_file src/lib.rs          12ms`: a row's status, label, and
    /// its detail against the right border.
    fn row(&mut self, card: usize, row: usize, line: usize, selected: bool) {
        let rect = self.layout.card_rect(card);
        let node = &self.flow.nodes()[self.layout.cards[card].rows[row]];
        let (glyph, glyph_color) = match node.kind {
            FlowKind::Reply => ("↳", self.theme.colors.secondary),
            _ => status_glyph(node.status, self.theme),
        };
        let base = if selected {
            self.selection()
        } else {
            Style::default().fg(match node.kind {
                FlowKind::Event => self.theme.colors.muted,
                FlowKind::Spawn => self.theme.colors.secondary,
                _ => self.theme.colors.foreground,
            })
        };
        if selected {
            self.line_fill(card, line, base);
        }
        let y = rect.y + line;
        let room = rect.width - 4;
        let glyph_style = if selected { base } else { base.fg(glyph_color) };
        self.text(rect.x + 2, y, glyph, 1, glyph_style);
        let detail = truncate_record_info(&node.detail, room / 3);
        let detail_width = display_width(&detail);
        let label_room = room - 2 - detail_width - usize::from(detail_width > 0);
        let label = truncate_record_info(&node.label, label_room);
        self.text(rect.x + 4, y, &label, label_room, base);
        if detail_width > 0 {
            let detail_style = if selected { base } else { self.muted() };
            self.text(
                rect.right() - 2 - detail_width,
                y,
                &detail,
                detail_width,
                detail_style,
            );
        }
    }
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Box glyphs by the ends they join.
const GLYPHS: [(&str, u8); 11] = [
    ("─", LEFT | RIGHT),
    ("│", UP | DOWN),
    ("├", UP | DOWN | RIGHT),
    ("┤", UP | DOWN | LEFT),
    ("┬", DOWN | LEFT | RIGHT),
    ("┴", UP | LEFT | RIGHT),
    ("┼", UP | DOWN | LEFT | RIGHT),
    ("┌", DOWN | RIGHT),
    ("┐", DOWN | LEFT),
    ("└", UP | RIGHT),
    ("┘", UP | LEFT),
];

fn ends_of(symbol: &str) -> u8 {
    GLYPHS
        .iter()
        .find(|(glyph, _)| *glyph == symbol)
        .map_or(0, |(_, ends)| *ends)
}

fn glyph_of(ends: u8) -> &'static str {
    GLYPHS
        .iter()
        .find(|(_, glyph_ends)| *glyph_ends == ends)
        .map_or(
            if ends & (UP | DOWN) == 0 {
                "─"
            } else {
                "│"
            },
            |(glyph, _)| glyph,
        )
}

fn kind_color(kind: FlowKind, theme: &Theme) -> Color {
    match kind {
        FlowKind::Prompt => theme.colors.primary,
        FlowKind::Model | FlowKind::Spawn | FlowKind::Message | FlowKind::Result => {
            theme.colors.secondary
        }
        FlowKind::Gate | FlowKind::Judge | FlowKind::Title | FlowKind::Event => theme.colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => theme.colors.foreground,
    }
}

#[cfg(test)]
mod tests;
