//! Scrolling the channel's chat to a message HISTORY jumped to, and
//! lighting it for a moment (see `app::apps::channels::history`).
//!
//! The jump is resolved while the chat is laid out, when every block's
//! rows in the whole conversation are known, so a message far back in the
//! scrollback is found as surely as one on screen.

use super::rows::RowLayout;
use crate::renderer::chat_lines::ChatLineCache;
use crate::{App, Theme};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use std::ops::Range;

/// What the chat's layout needs of a jump: the block the jump waiting
/// goes to, and the block lit by the last one.
#[derive(Clone, Copy)]
pub(super) struct Wanted {
    /// `Some` while a jump waits: the key of its block, `None` when its
    /// message is gone
    jump: Option<Option<u64>>,
    /// The block lit by the last jump, while it still is
    flash: Option<u64>,
}

/// Where the wanted blocks fall in the chat.
pub(super) struct JumpRows {
    jump: Option<(Option<u64>, Option<Range<usize>>)>,
    flash: Option<Range<usize>>,
}

/// Take the jump waiting for the chat on screen, and the block lit.
/// The terminal pane takes no jump.
pub(super) fn wanted(app: &mut App) -> Wanted {
    let jump = if app.terminal_pane() {
        None
    } else {
        app.take_history_jump()
    };
    Wanted {
        jump,
        flash: app.history_flash(),
    }
}

/// The rows of the `wanted` blocks in `cache`, as just laid out.
pub(super) fn rows_of(cache: &ChatLineCache, wanted: Wanted) -> JumpRows {
    let rows = |key: Option<u64>| key.and_then(|key| cache.block_rows(key));
    JumpRows {
        jump: wanted.jump.map(|key| (key, rows(key))),
        flash: rows(wanted.flash),
    }
}

/// Make the jump in `rows`, if one waits, in a chat `total` rows long
/// shown `height` at a time from `start`; returns the first row to show
/// and the rows to light.
pub(super) fn apply(
    app: &mut App,
    rows: JumpRows,
    start: usize,
    (total, height): (usize, usize),
) -> (usize, Option<Range<usize>>) {
    let Some((key, block)) = rows.jump else {
        return (start, rows.flash);
    };
    match app.finish_history_jump(key, block.clone(), (total, height)) {
        Some(first) => (first, block),
        None => (start, rows.flash),
    }
}

/// Fill the rows `layout` lights that are on screen, the chat shown in
/// `padded`, with the hover shade, under the text drawn next.
pub(super) fn paint_flash(buffer: &mut Buffer, padded: Rect, layout: &RowLayout, theme: &Theme) {
    let (Some(lit), start) = (layout.flash.as_ref(), layout.start) else {
        return;
    };
    let fill = Style::default().bg(theme.colors.surface_hover);
    let on_screen = lit.start.max(start)..lit.end.min(start + usize::from(padded.height));
    for row in on_screen {
        let Ok(offset) = u16::try_from(row - start) else {
            continue;
        };
        let line = Rect {
            y: padded.y + offset,
            height: 1,
            ..padded
        };
        buffer.set_style(line, fill);
    }
}
