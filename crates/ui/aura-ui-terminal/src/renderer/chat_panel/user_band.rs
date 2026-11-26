//! The frame drawn around the user's messages.

use crate::Theme;
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

/// Where a row falls in the frame around a user's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BandEdge {
    /// The frame's top border
    Top,
    /// A row between the borders: padding or text
    Side,
    /// The frame's bottom border
    Bottom,
}

impl BandEdge {
    /// The edge of row `offset` of a user's block `len` rows long. The
    /// block's first and last rows are left blank for the borders (see
    /// `chat_lines::USER_FRAME_ROWS`).
    pub(super) const fn at(offset: usize, len: usize) -> Self {
        if offset == 0 {
            Self::Top
        } else if offset + 1 == len {
            Self::Bottom
        } else {
            Self::Side
        }
    }
}

/// Frame the rows of the user's messages: a border two columns out from
/// the text on each side, clipped to `body`. The frame is not shaded, so
/// a sent message sits on the chat's own background.
pub(super) fn paint_user_band(
    buffer: &mut Buffer,
    (body, padded): (Rect, Rect),
    rows: &[(usize, BandEdge)],
    theme: &Theme,
) {
    let border = Style::default().fg(theme.colors.faint);
    let symbols = ratatui::widgets::BorderType::border_symbols(theme.border_style.to_border_type());
    let left = padded.x.saturating_sub(2).max(body.x);
    let right = padded
        .right()
        .saturating_add(1)
        .min(body.right().saturating_sub(1));
    if right <= left {
        return;
    }
    for &(row, edge) in rows {
        let Ok(offset) = u16::try_from(row) else {
            continue;
        };
        let y = padded.y + offset;
        let (start, middle, end) = match edge {
            BandEdge::Top => (symbols.top_left, symbols.horizontal_top, symbols.top_right),
            BandEdge::Bottom => (
                symbols.bottom_left,
                symbols.horizontal_bottom,
                symbols.bottom_right,
            ),
            BandEdge::Side => (symbols.vertical_left, "", symbols.vertical_right),
        };
        let mut put = |x: u16, symbol: &str| {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(symbol).set_style(border);
            }
        };
        put(left, start);
        put(right, end);
        if !middle.is_empty() {
            for x in left + 1..right {
                put(x, middle);
            }
        }
    }
}
