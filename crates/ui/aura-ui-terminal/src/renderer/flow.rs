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

use super::flow_minimap::render_minimap;
use super::flow_timeline::render_timeline;
use super::graph::status_glyph;
use super::panels::{body_block, render_scrollbar, truncate_record_info, ScrollView};
use super::text::display_width;
use crate::app::flow::layout::{FlowLayout, Wire, WireKind, CARD_WIDTH};
use crate::app::flow::{FlowMode, FlowState, FlowView};
use crate::events::{FlowKind, TraceNodeStatus};
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
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
    let muted = Style::default().fg(theme.colors.muted);
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Nothing of the conversation is on screen to click or select.
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);

    app.flow_mut().set_room(usize::from(inner.height));
    app.flow_mut().ensure_layout();
    if inner.height < 3 || inner.width < 12 || app.flow().layout().cards.is_empty() {
        app.flow_mut().set_view(None, false);
        app.flow_mut().set_minimap(None);
        let text = if app.flow().loaded() {
            "Nothing recorded yet. Prompts, model calls, tool calls and the agents \
             they start show up here as the agent works."
        } else {
            "Loading the graph…"
        };
        let padded = Rect {
            x: inner.x.saturating_add(1),
            width: inner.width.saturating_sub(2),
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, muted))).wrap(Wrap { trim: true }),
            padded,
        );
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
mod tests {
    use super::*;
    use crate::app::ChatTab;
    use crate::events::{FlowLane, FlowNode, TraceNodeStatus, UiCommand};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, Terminal};

    const LEAD: &str = "aa";
    const CODER: &str = "bb";

    fn node(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
        FlowNode {
            id: format!("{agent}:{seq}"),
            agent_id: agent.into(),
            kind,
            label: label.into(),
            status: TraceNodeStatus::Ok,
            seq,
            at_ms,
            ..FlowNode::default()
        }
    }

    /// A lead whose model call gives a coder a task, and gets its reply.
    fn app_with_flow() -> App {
        app_with_flow_and(Vec::new())
    }

    /// [`app_with_flow`], with `more` nodes after it.
    fn app_with_flow_and(more: Vec<FlowNode>) -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        let mut model = node(LEAD, 2, 105, FlowKind::Model, "sonnet");
        model.detail = "1.2k/300 · 4s".into();
        let mut read = node(LEAD, 3, 108, FlowKind::Tool, "read_file src/lib.rs");
        read.detail = "12ms".into();
        let mut call = node(LEAD, 4, 110, FlowKind::Tool, "task coder · write greet()");
        call.links = vec![
            ("spawn".into(), "bb:1".into()),
            ("result".into(), "aa:5".into()),
        ];
        call.detail = "4.2s".into();
        let mut reply = node(CODER, 2, 130, FlowKind::Reply, "done");
        reply.links = vec![("return".into(), "aa:5".into())];
        let mut pending = node(LEAD, 6, 150, FlowKind::Model, "sonnet");
        pending.status = TraceNodeStatus::Pending;
        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: vec![
                FlowLane {
                    agent_id: LEAD.into(),
                    name: "Lead".into(),
                    ..FlowLane::default()
                },
                FlowLane {
                    agent_id: CODER.into(),
                    name: "coder #1".into(),
                    parent: Some(LEAD.into()),
                    ..FlowLane::default()
                },
            ],
            nodes: [
                node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
                model,
                read,
                call,
                node(CODER, 1, 120, FlowKind::Prompt, "write greet()"),
                reply,
                node(LEAD, 5, 140, FlowKind::Result, "coder #1 replied"),
                pending,
            ]
            .into_iter()
            .chain(more)
            .collect(),
        });
        app
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::cyber();
        terminal
            .draw(|frame| render_flow(frame, frame.area(), app, &theme, true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }

    /// The whole screen, 160 × 30, as the backend got it.
    fn whole_screen(app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// Where `text` first is on screen.
    fn find(buffer: &Buffer, text: &str) -> Option<(u16, u16)> {
        (0..buffer.area.height).find_map(|y| {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.find(text)
                .map(|at| (u16::try_from(row[..at].chars().count()).unwrap(), y))
        })
    }

    #[test]
    fn a_turn_is_a_main_card_with_its_model_calls_on_a_spine() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 84, 24);
        let expected = [
            "│                                                                                  │",
            "│  ┌ ▾ ✓ turn ───────────── 2 steps ┐                                              │",
            "│  │ add a greeting                 │                                              │",
            "│  └─┬──────────────────────────────┘                                              │",
            "│    │                                                                             │",
            "│    │ ┌ ▾ ✓ model ── 1.2k/300 · 4s ┐                                              │",
            "│    ├─┤ sonnet                     │                                              │",
            "│    │ │ ✓ read_file src/lib.… 12ms │                                              │",
            "│    │ │ ✓ task coder · write… 4.2s ├────┐                                         │",
            "│    │ └────────────────────────────┘    │                                         │",
            "│    │                                   │                                         │",
            "│    │ ┌ ● model ───────────────────┐    │                                         │",
            "│    └─┤ sonnet                     │    │                                         │",
            "│      └────────────────────────────┘    │                                         │",
            "│                                        │                                         │",
            "│                                        │ ▾ coder #1                              │",
            "│                                        │ ┌ ✓ turn ────────────── coder #1 ┐      │",
            "│                                        └─┤ write greet()                  │      │",
            "│                                          │ ↳ done                         │      │",
            "│                                          └─────────── ↩ reported to Lead ─┘      │",
        ];
        assert_eq!(rows[1..=20], expected, "\n{}", rows.join("\n"));
    }

    #[test]
    fn turns_go_down_a_column_while_it_has_room_then_wrap_right() {
        let more = || {
            vec![
                node(LEAD, 7, 160, FlowKind::Prompt, "now say bye"),
                node(LEAD, 8, 165, FlowKind::Model, "haiku"),
            ]
        };
        let mut app = app_with_flow_and(more());
        let rows = screen(&mut app, 84, 40);
        assert!(
            rows.iter().any(|row| row.starts_with("│  │ now say bye")),
            "under the first turn, folded now\n{}",
            rows.join("\n")
        );

        let mut app = app_with_flow_and(more());
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 84, 12);
        assert!(
            rows.iter()
                .any(|row| row.contains("add a greeting") && row.contains("now say bye")),
            "no room under it: the next column\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn a_folded_turn_is_its_main_card_and_says_how_much_it_hides() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char(' '));
        let rows = screen(&mut app, 120, 16).join("\n");
        assert!(
            rows.contains("┌ ▸ ✓ turn ──────────── +2 steps ┐"),
            "{rows}"
        );
        assert!(!rows.contains("sonnet"), "{rows}");
        assert!(
            rows.contains("add a greeting                 ├────┐"),
            "the spawn comes from the main card\n{rows}"
        );
        assert!(rows.contains("└─┤ write greet()"), "{rows}");
    }

    #[test]
    fn a_folded_model_call_hides_its_tool_calls() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");
        press(&mut app, KeyCode::Char(' '));
        let rows = screen(&mut app, 120, 24).join("\n");
        assert!(rows.contains("┌ ▸ ✓ model ─ +2 tools · "), "{rows}");
        assert!(!rows.contains("read_file"), "{rows}");
        assert!(
            rows.contains("├─┤ sonnet                     ├────┐"),
            "the spawn comes from the model call\n{rows}"
        );
        press(&mut app, KeyCode::Char(' '));
        let rows = screen(&mut app, 120, 24).join("\n");
        assert!(rows.contains("read_file"), "{rows}");
    }

    #[test]
    fn the_view_follows_the_selection_across_the_canvas() {
        let mut app = app_with_flow();
        let rows = screen(&mut app, 80, 10).join("\n");
        assert!(rows.contains("● model"), "the newest card, pending\n{rows}");
        assert!(!rows.contains("add a greeting"), "{rows}");
        press(&mut app, KeyCode::Home);
        let rows = screen(&mut app, 80, 10).join("\n");
        assert!(rows.contains("add a greeting"), "{rows}");

        // Panned. Other agents are named over their cards; the agent on
        // screen goes unnamed.
        app.pan_flow(40.0, 14.0);
        let rows = screen(&mut app, 80, 10).join("\n");
        assert!(!rows.contains("add a greeting"));
        assert!(rows.contains("coder #1"), "{rows}");
        assert!(
            rows.lines()
                .all(|row| !row.contains("Lead") || row.contains("↩ reported to Lead")),
            "only the report back names the agent on screen\n{rows}"
        );
    }

    #[test]
    fn clicks_select_and_open_lines_and_drags_and_the_wheel_pan() {
        let mut app = app_with_flow();
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        let label = find(&buffer, "sonnet").unwrap();
        let marker = find(&buffer, "▾ ✓ turn").unwrap();
        let down = MouseEventKind::Down(MouseButton::Left);
        mouse(&mut app, down, label);
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");
        assert!(app.detail_view().is_none());
        mouse(&mut app, down, label);
        assert!(app.detail_view().is_some(), "the second click opens it");
        press(&mut app, KeyCode::Esc);

        // The lead's turn is the first with a marker.
        mouse(&mut app, down, marker);
        app.flow_mut().ensure_layout();
        assert!(app.flow().layout().cards[0].folded);

        // A drag on empty canvas moves the view with the pointer, over a
        // flow wider than the screen.
        let turns = (0..12).flat_map(|turn| {
            let at = 200 + turn * 10;
            [
                node(LEAD, 7 + 2 * turn, at, FlowKind::Prompt, "and then"),
                node(LEAD, 8 + 2 * turn, at + 5, FlowKind::Model, "haiku"),
            ]
        });
        let mut app = app_with_flow_and(turns.collect());
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        // The gutter right of the prompt.
        let (x, y) = find(&buffer, "add a greeting").unwrap();
        let empty = (x + 35, y);
        let pan = app.flow().pan();
        mouse(&mut app, down, empty);
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            (empty.0 - 5, y),
        );
        mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            (empty.0 - 5, y),
        );
        assert_eq!(app.flow().pan(), (pan.0 + 5.0, pan.1));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");

        // The wheel moves the view and leaves the selection alone.
        let _ = whole_screen(&mut app);
        let pan = app.flow().pan();
        mouse(&mut app, MouseEventKind::ScrollLeft, empty);
        assert!(app.flow().pan().0 < pan.0);
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn a_front_end_that_draws_the_canvas_gets_it_blank() {
        let mut app = app_with_flow();
        app.set_host_canvas(true);
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        assert!(find(&buffer, "add a greeting").is_none());
        let canvas = app.flow_canvas().expect("the canvas is on screen");
        assert_eq!(app.flow_canvas_clip(), [canvas]);
        for y in canvas.top()..canvas.bottom() {
            for x in canvas.left()..canvas.right() {
                let cell = &buffer[(x, y)];
                assert!(!cell.skip && cell.symbol() == " ", "({x}, {y})");
            }
        }
        assert_eq!(
            buffer[(canvas.x - 1, canvas.y)].symbol(),
            "│",
            "the border is drawn"
        );

        // Its clicks come back in canvas cells; terminal clicks are not its.
        let (x, y) = (canvas.x + 5, canvas.y + 2);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), (x, y));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");
        let model = app.flow().layout().card_rect(1);
        assert!(app.click_flow_at(model.x as f32 + 4.5, model.y as f32 + 1.5));
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:2");

        // Away from the tab, there is no canvas to draw.
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.chat_tab(), ChatTab::Terminal);
        let _ = whole_screen(&mut app);
        assert!(app.flow_canvas().is_none());
        assert!(app.flow_canvas_clip().is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));

        // A menu over the canvas is left out of where it is drawn.
        app.handle_key(KeyEvent::from(KeyCode::Char('/')));
        let buffer = whole_screen(&mut app);
        let clip = app.flow_canvas_clip().to_vec();
        let drawn: u32 = clip.iter().map(|rect| rect.area()).sum();
        assert!(drawn > 0 && drawn < canvas.area(), "{clip:?}");
        let menu = (canvas.top()..canvas.bottom())
            .flat_map(|y| (canvas.left()..canvas.right()).map(move |x| (x, y)))
            .find(|&(x, y)| buffer[(x, y)].symbol() != " ")
            .expect("the menu is over the canvas");
        assert!(
            !clip.iter().any(|rect| rect.contains(menu.into())),
            "{menu:?}"
        );
    }

    #[test]
    fn a_canvas_larger_than_the_view_has_a_map_that_moves_the_view() {
        let mut small = app_with_flow();
        let _ = screen(&mut small, 160, 60);
        assert_eq!(small.flow().minimap(), None, "it all fits");

        let turns = (0..12).flat_map(|turn| {
            let at = 200 + turn * 10;
            [
                node(LEAD, 7 + 2 * turn, at, FlowKind::Prompt, "and then"),
                node(LEAD, 8 + 2 * turn, at + 5, FlowKind::Model, "haiku"),
            ]
        });
        let mut app = app_with_flow_and(turns.collect());
        press(&mut app, KeyCode::Home);
        let buffer = whole_screen(&mut app);
        let map = app.flow().minimap().expect("the map is drawn");
        assert!(find(&buffer, " map ").is_some());
        let inner = map.inner();
        let cells = || {
            (inner.top()..inner.bottom())
                .flat_map(|y| (inner.left()..inner.right()).map(move |x| (x, y)))
        };
        let theme = Theme::cyber();
        assert!(
            cells().any(|at| buffer[at].bg == theme.colors.selection),
            "the view is lit on it"
        );
        assert!(cells().any(|at| buffer[at].symbol() != " "), "cards on it");

        // A press on its far end moves the view there; the selection stays.
        let pan = app.flow().pan();
        let far = (inner.right() - 1, inner.y);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), far);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), far);
        assert!(app.flow().pan().0 > pan.0, "{:?}", app.flow().pan());
        assert_eq!(app.flow().selected_node().unwrap().id, "aa:1");

        press(&mut app, KeyCode::Char('m'));
        let buffer = whole_screen(&mut app);
        assert!(find(&buffer, " map ").is_none(), "m hides it");
        assert_eq!(app.flow().minimap(), None);
        press(&mut app, KeyCode::Char('m'));

        // Over a canvas the front end draws, the map's cells are left to
        // the terminal.
        app.set_host_canvas(true);
        let _ = whole_screen(&mut app);
        let map = app.flow().minimap().expect("the map is drawn");
        assert!(!app.flow_canvas_clip().is_empty());
        assert!(
            app.flow_canvas_clip()
                .iter()
                .all(|rect| rect.intersection(map.area).is_empty()),
            "{:?} {:?}",
            app.flow_canvas_clip(),
            map.area
        );
    }

    #[test]
    fn says_so_while_loading_and_when_nothing_is_recorded() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        let rows = screen(&mut app, 60, 8).join("\n");
        assert!(rows.contains("Loading the graph"), "{rows}");

        app.process_command(UiCommand::FlowLoad {
            agent_id: LEAD.into(),
            lanes: Vec::new(),
            nodes: Vec::new(),
        });
        let rows = screen(&mut app, 60, 8).join("\n");
        assert!(rows.contains("Nothing recorded yet"), "{rows}");
        assert!(app.flow_canvas().is_none());
    }
}
