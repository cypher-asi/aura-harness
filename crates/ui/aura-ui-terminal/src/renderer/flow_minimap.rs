//! The map of the whole GRAPH canvas, in its top right corner when the
//! canvas is twice the view or more: each card a block, muted when it went
//! well, in the colour of its state when it did not, the selected one in
//! the accent colour; the part of the canvas on screen is lit. A click or
//! drag on it moves the view there; `m` hides it.
//!
//! ```text
//!   ┌ map ───────────┐
//!   │▀▀ ▀▀ ▄▄ ██     │
//!   │   ▄▄ ▀▀ ▀▀ ▄▄  │
//!   └────────────────┘
//! ```
//!
//! It is drawn in cells over a canvas the front end draws too, which then
//! leaves those cells to it.

use crate::app::flow::MinimapView;
use crate::events::TraceNodeStatus;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Clear},
    Frame,
};
use std::ops::Range;

/// The largest the map's box gets, its border included.
const MAX_WIDTH: u16 = 34;
const MAX_HEIGHT: u16 = 12;
/// The smallest inside of the box worth drawing.
const MIN_INNER: (u16, u16) = (8, 2);

/// The map cells, of `scale` canvas cells each, whose middles are in
/// canvas cells `start..end`; the one `start` is in when none are.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn covered(start: usize, end: usize, scale: f32, count: usize) -> Range<usize> {
    let first = ((start as f32 / scale) - 0.5).ceil().max(0.0) as usize;
    let last = ((end as f32 / scale) - 0.5).ceil().max(0.0) as usize;
    let range = if last > first {
        first..last
    } else {
        let at = (start as f32 / scale) as usize;
        at..at + 1
    };
    range.start.min(count)..range.end.min(count)
}

/// Draw the map in the corner of `inner`, the canvas's cells, when the
/// canvas is large enough to need one; remember where for the mouse.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(super) fn render_minimap(frame: &mut Frame, inner: Rect, app: &mut App, theme: &Theme) {
    let flow = app.flow();
    let layout = flow.layout();
    let room = (
        MAX_WIDTH.min(inner.width / 4).saturating_sub(2),
        MAX_HEIGHT.min(inner.height / 3).saturating_sub(2),
    );
    if !flow.shows_minimap(inner.width, inner.height)
        || room.0 < MIN_INNER.0
        || room.1 < MIN_INNER.1
        || layout.width == 0
        || layout.height == 0
    {
        app.flow_mut().set_minimap(None);
        return;
    }
    // A map cell stands for as many canvas columns as rows, so the map
    // has the canvas's shape; its box is no larger than that takes.
    let scale = (layout.width as f32 / f32::from(room.0))
        .max(layout.height as f32 / f32::from(room.1))
        .max(1.0);
    let size = (
        ((layout.width as f32 / scale).ceil() as u16).clamp(MIN_INNER.0, room.0),
        ((layout.height as f32 / scale).ceil() as u16).clamp(1, room.1),
    );
    let area = Rect {
        x: inner.right() - size.0 - 2,
        y: inner.y,
        width: size.0 + 2,
        height: size.1 + 2,
    };
    let map = MinimapView {
        area,
        scale: (scale, scale),
    };
    let body = map.inner();

    // Each map cell is two halves, a top and a bottom, so a card a
    // half's height tall still shows.
    let (columns, halves) = (usize::from(body.width), usize::from(body.height) * 2);
    let mut paint: Vec<Option<(u8, Color)>> = vec![None; columns * halves];
    let selected = flow.selected_card();
    for (card, placed) in layout.cards.iter().enumerate() {
        let rect = layout.card_rect(card);
        let (rank, color) = if Some(card) == selected {
            (4, theme.colors.success)
        } else {
            match flow.nodes()[placed.head].status {
                TraceNodeStatus::Error => (3, theme.colors.error),
                TraceNodeStatus::Warn => (2, theme.colors.warning),
                TraceNodeStatus::Pending => (1, theme.colors.pending),
                TraceNodeStatus::Ok => (0, theme.colors.muted),
            }
        };
        for y in covered(rect.y, rect.bottom(), scale / 2.0, halves) {
            for x in covered(rect.x, rect.right(), scale, columns) {
                let cell = &mut paint[y * columns + x];
                if cell.is_none_or(|(had, _)| had < rank) {
                    *cell = Some((rank, color));
                }
            }
        }
    }

    // The part of the canvas on screen.
    let (pan, zoom) = (flow.pan(), flow.zoom());
    let seen = (
        pan.0 / scale,
        pan.1 / scale,
        (pan.0 + f32::from(inner.width) / zoom) / scale,
        (pan.1 + f32::from(inner.height) / zoom) / scale,
    );
    let in_view = |x: usize, y: usize| {
        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
        x >= seen.0 && x < seen.2 && y >= seen.1 && y < seen.3
    };

    frame.render_widget(Clear, area);
    let border = Style::default().fg(theme.colors.muted);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(Span::styled(" map ", border)),
        area,
    );
    let buffer = frame.buffer_mut();
    for row in 0..usize::from(body.height) {
        for column in 0..columns {
            let top = paint[row * 2 * columns + column].map(|(_, color)| color);
            let bottom = paint[(row * 2 + 1) * columns + column].map(|(_, color)| color);
            let mut style = Style::default();
            if in_view(column, row) {
                style = style.bg(theme.colors.selection);
            }
            let symbol = match (top, bottom) {
                (Some(top), Some(bottom)) if top == bottom => {
                    style = style.fg(top);
                    "█"
                }
                (Some(top), Some(bottom)) => {
                    style = style.fg(top).bg(bottom);
                    "▀"
                }
                (Some(top), None) => {
                    style = style.fg(top);
                    "▀"
                }
                (None, Some(bottom)) => {
                    style = style.fg(bottom);
                    "▄"
                }
                (None, None) => " ",
            };
            let at = (
                body.x + u16::try_from(column).unwrap_or(u16::MAX),
                body.y + u16::try_from(row).unwrap_or(u16::MAX),
            );
            if let Some(cell) = buffer.cell_mut(at) {
                cell.set_symbol(symbol).set_style(style);
                // A hosted canvas is not to be drawn under the map.
                cell.set_skip(false);
            }
        }
    }
    app.flow_mut().set_minimap(Some(map));
}
