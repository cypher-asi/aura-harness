//! A message the chat jumped to from a channel's HISTORY: scrolling to
//! it once its rows are known, and lighting it up for a moment (see
//! `app::apps::channels::history`).

use crate::renderer::chat_lines::ChatLineCache;
use crate::{App, Theme};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use std::ops::Range;
use std::time::Instant;

/// The blocks a frame looks for: the one a jump waits to scroll to and
/// the one lit up, each by its key.
pub(super) struct Targets {
    jump: Option<u64>,
    flash: Option<u64>,
}

/// Where those blocks fell: the jump's block with its rows, and the lit
/// block's rows.
pub(super) struct Spans {
    jump: Option<(u64, Range<usize>)>,
    flash: Option<Range<usize>>,
}

impl Targets {
    /// What `app` wants found this frame.
    pub(super) fn of(app: &App) -> Self {
        Self {
            jump: app.chat_jump_block(),
            flash: app.chat_flash_block(Instant::now()),
        }
    }

    /// Where the blocks fall in `cache`, laid out for this frame.
    pub(super) fn spans(&self, cache: &ChatLineCache) -> Spans {
        Spans {
            jump: self
                .jump
                .and_then(|key| cache.block_span(key).map(|rows| (key, rows))),
            flash: self.flash.and_then(|key| cache.block_span(key)),
        }
    }
}

/// Scroll the chat, `total` rows long and `height` tall, to the jump's
/// block when it was found, or give up a jump that waited too long.
/// Returns the rows to light up.
pub(super) fn settle_jump(
    app: &mut App,
    spans: Spans,
    (total, height): (usize, usize),
) -> Option<Range<usize>> {
    if let Some((key, rows)) = spans.jump {
        app.jump_chat_to_row(key, rows.start, (total, height));
        return Some(rows);
    }
    app.expire_chat_jump(Instant::now());
    spans.flash
}

/// Light up `rows` of the chat that fall on screen, in `padded`, whose
/// top row is chat row `start`.
pub(super) fn paint(
    buffer: &mut Buffer,
    rows: Option<Range<usize>>,
    (padded, start): (Rect, usize),
    theme: &Theme,
) {
    let Some(rows) = rows else {
        return;
    };
    let height = usize::from(padded.height);
    let first = rows.start.max(start);
    let end = rows.end.min(start + height);
    if first >= end {
        return;
    }
    let lit = Rect {
        x: padded.x,
        y: padded.y + u16::try_from(first - start).unwrap_or(u16::MAX),
        width: padded.width,
        height: u16::try_from(end - first).unwrap_or(u16::MAX),
    }
    .intersection(padded);
    buffer.set_style(lit, Style::default().bg(theme.colors.selection));
}
