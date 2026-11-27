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

mod card;
mod wires;

use super::chrome::{body_block, pad_top, render_scrollbar, ScrollView};
use super::flow_minimap::render_minimap;
use super::flow_timeline::render_timeline;
use super::side_list::render_empty;
use super::text::display_width;
use crate::app::flow::layout::{FlowLayout, Wire, CARD_WIDTH};
use crate::app::flow::{FlowMode, FlowState, FlowView};
use crate::components::truncate_record_info;
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;
use wires::{LEFT, RIGHT};

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
    clear_chat_layout(app, inner);

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
        render_empty(frame, pad_top(inner), text, theme);
        return;
    }

    if app.flow().mode() == FlowMode::Timeline {
        // Drawn in cells by every front end.
        app.flow_mut().set_view(None, false);
        app.flow_mut().set_minimap(None);
        render_timeline(frame, area, pad_top(inner), app, theme, is_focused);
        return;
    }
    app.flow_mut().timeline_mut().set_view(None);

    let first = render_canvas(frame, inner, app, theme);
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

/// Nothing of the conversation is on screen to click or select.
fn clear_chat_layout(app: &mut App, inner: Rect) {
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.clear_chat_links();
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
}

/// Draw the part of the canvas on screen in `inner`, or leave its cells
/// blank for a front end that draws it; returns the canvas cell at its
/// top left.
fn render_canvas(frame: &mut Frame, inner: Rect, app: &mut App, theme: &Theme) -> (i64, i64) {
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
    first
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
}

#[cfg(test)]
mod tests;
