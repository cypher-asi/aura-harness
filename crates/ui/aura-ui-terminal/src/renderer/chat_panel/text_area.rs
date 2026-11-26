//! Where the chat's and the terminal's text sits in the panel's body, and
//! the blank rows kept below the conversation.

use crate::renderer::chrome::pad_sides;
use ratatui::layout::Rect;

/// The text area of the chat and the terminal in `area`, the body's rows
/// below its blank top row. The terminal keeps to the panel's side
/// padding, so output starts at the top left like a terminal's; the chat
/// keeps to it as well, stops at [`MAX_CHAT_WIDTH`] columns so long lines
/// stay readable, then gives up [`CHAT_NARROW_PERCENT`] of that width and
/// sits centered, with equal padding on both sides.
pub(in crate::renderer) fn terminal_text_area(area: Rect, terminal: bool) -> Rect {
    let padded = pad_sides(area);
    if terminal {
        return padded;
    }
    let width = narrowed(padded.width.min(MAX_CHAT_WIDTH));
    // Center the narrowed text so the blank space on either side matches.
    let left = (area.width - width) / 2;
    Rect {
        x: area.x.saturating_add(left),
        width,
        ..area
    }
}

/// Widest the chat's text runs, in columns, before [`CHAT_NARROW_PERCENT`]
/// is taken off.
const MAX_CHAT_WIDTH: u16 = 110;

/// Share of the chat's text width, in percent, given up to extra padding
/// split evenly on both sides.
const CHAT_NARROW_PERCENT: u32 = 17;

/// `width` less [`CHAT_NARROW_PERCENT`], rounded to the nearest column.
fn narrowed(width: u16) -> u16 {
    let kept = (u32::from(width) * (100 - CHAT_NARROW_PERCENT) + 50) / 100;
    u16::try_from(kept).unwrap_or(width)
}

/// Share of the chat's height, in percent, kept blank below the
/// conversation once it reaches the bottom.
const CHAT_ROOM_PERCENT: usize = 25;

/// Blank rows kept below the conversation's last line in a chat `height`
/// rows tall: [`CHAT_ROOM_PERCENT`] of them, rounded down, so the newest
/// text stops short of the bottom border and the input box below it, and
/// a short panel keeps its few rows for text.
pub(in crate::renderer) const fn breathing_room(height: usize) -> usize {
    height * CHAT_ROOM_PERCENT / 100
}
