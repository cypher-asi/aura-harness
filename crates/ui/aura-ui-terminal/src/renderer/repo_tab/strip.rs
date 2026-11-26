//! The REPO tab's strip of filters: the picked one lit as a shown tab
//! is, and on a panel too narrow for all of them, those around it with
//! `‹` and `›` where it is cut.

use crate::app::repo_tab::RepoFilter;
use crate::app::{HoverTarget, PanelFocus};
use crate::renderer::header::tab_style;
use crate::{App, Theme};
use ratatui::{buffer::Buffer, layout::Rect, style::Style, Frame};
use std::ops::Range;

/// A piece of the strip: a filter, or the bar that sets the lists apart
/// from the timeline's filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Piece {
    Filter(RepoFilter),
    Bar,
}

impl Piece {
    /// Columns it takes: a filter's label with a space on each side.
    pub(super) fn width(self) -> u16 {
        match self {
            Self::Filter(filter) => u16::try_from(filter.label().len() + 2).unwrap_or(u16::MAX),
            Self::Bar => 1,
        }
    }
}

/// The strip's pieces for `filters`, in order.
pub(super) fn pieces(filters: &[RepoFilter]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for &filter in filters {
        let first_list = filter.is_list() && !pieces.iter().any(|&p| is_list_piece(p));
        if first_list && !pieces.is_empty() {
            pieces.push(Piece::Bar);
        }
        pieces.push(Piece::Filter(filter));
    }
    pieces
}

const fn is_list_piece(piece: Piece) -> bool {
    matches!(piece, Piece::Filter(filter) if filter.is_list())
}

/// The pieces that fit in `room` columns with the one at `active` among
/// them. When they don't all fit, a column at each end is kept for `‹`
/// and `›`, and the pieces shown start as early as they can.
pub(super) fn window(widths: &[u16], active: usize, room: u16) -> Range<usize> {
    let sum = |range: Range<usize>| -> u32 { widths[range].iter().map(|&w| u32::from(w)).sum() };
    if sum(0..widths.len()) <= u32::from(room) {
        return 0..widths.len();
    }
    let room = u32::from(room.saturating_sub(2));
    let active = active.min(widths.len().saturating_sub(1));
    let mut first = 0;
    while first < active && sum(first..active + 1) > room {
        first += 1;
    }
    let mut end = (active + 1).min(widths.len());
    while end < widths.len() && sum(first..end + 1) <= room {
        end += 1;
    }
    first..end
}

/// How the strip lights its filters.
#[derive(Debug, Clone, Copy)]
struct Lit {
    /// The filter picked
    active: RepoFilter,
    /// The filter under the pointer
    hovered: Option<RepoFilter>,
    /// Whether the panel has focus
    focused: bool,
}

/// Draw the strip of filters into the one-row `strip`, the picked one lit
/// as a shown tab is, and record where each filter went.
pub(super) fn render_strip(frame: &mut Frame, strip: Rect, app: &mut App, theme: &Theme) {
    if strip.height == 0 {
        app.set_repo_strip(Vec::new(), (None, None));
        return;
    }
    let filters = app.repo_filters();
    let lit = Lit {
        active: app.repo_filter(),
        hovered: match app.hover() {
            Some(HoverTarget::RepoFilter(filter)) => Some(filter),
            _ => None,
        },
        focused: app.focus() == PanelFocus::Sandbox,
    };
    let pieces = pieces(&filters);
    let widths: Vec<u16> = pieces.iter().map(|piece| piece.width()).collect();
    let at = pieces
        .iter()
        .position(|&piece| piece == Piece::Filter(lit.active))
        .unwrap_or(0);
    let shown = window(&widths, at, strip.width);
    let cut = shown.len() < pieces.len();
    // The last column left to the pieces, past which `›` goes.
    let end = strip.right().saturating_sub(u16::from(cut));
    let buf = frame.buffer_mut();
    let start = strip.x + u16::from(cut);
    let drawn = (&pieces[shown.clone()], &widths[shown.clone()]);
    let chips = draw_pieces(buf, drawn, (start, end, strip.y), lit, theme);
    let back = (cut && shown.start > 0).then(|| Rect::new(strip.x, strip.y, 1, 1));
    let on = (cut && shown.end < pieces.len()).then(|| Rect::new(end, strip.y, 1, 1));
    let muted = Style::default().fg(theme.colors.muted);
    for (step, glyph) in [(back, "‹"), (on, "›")] {
        if let Some(cell) = step {
            buf.set_string(cell.x, cell.y, glyph, muted);
        }
    }
    app.set_repo_strip(chips, (back, on));
}

/// Draw `pieces`, as many columns wide as their `widths`, on row `y` from
/// column `start` up to `end`, returning where each filter went.
fn draw_pieces(
    buf: &mut Buffer,
    (pieces, widths): (&[Piece], &[u16]),
    (start, end, y): (u16, u16, u16),
    lit: Lit,
    theme: &Theme,
) -> Vec<(Rect, RepoFilter)> {
    let mut x = start;
    let mut chips = Vec::new();
    for (&piece, &width) in pieces.iter().zip(widths) {
        let width = width.min(end.saturating_sub(x));
        if width == 0 {
            break;
        }
        let cell = Rect::new(x, y, width, 1);
        match piece {
            Piece::Filter(filter) => {
                let hovered = lit.hovered == Some(filter);
                let style = tab_style(filter == lit.active, hovered, lit.focused, theme);
                buf.set_style(cell, style);
                let room = usize::from(width.saturating_sub(1));
                buf.set_stringn(x + 1, y, filter.label(), room, style);
                chips.push((cell, filter));
            }
            Piece::Bar => {
                buf.set_string(x, y, "│", Style::default().fg(theme.colors.faint));
            }
        }
        x += width;
    }
    chips
}
