//! The row below the chat's screen, which the smooth scroll slides in.

use super::prose::{pack_prose, prose_columns};
use super::user_band::{paint_user_band, BandEdge};
use super::visible::NextRow;
use crate::renderer::text::Measure;
use crate::{
    app::{ChatScroll, ProseRow},
    App, Theme,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget},
    Frame,
};

/// What the smooth scroll draws of the row below the screen.
pub(super) struct BelowRow {
    pub(super) next: NextRow,
    pub(super) measure: Measure,
    /// How far the chat has faded in, while it does
    pub(super) fade: Option<(usize, usize)>,
}

/// Hand the front end the chat's `body`, scrolled to `start`, and the row
/// below it, for the smooth scroll.
pub(super) fn hand_over_scroll(
    frame: &mut Frame,
    app: &mut App,
    (body, padded, start): (Rect, Rect, usize),
    below: BelowRow,
    theme: &Theme,
) {
    let BelowRow {
        next: (next, (next_prose, next_user)),
        measure,
        fade,
    } = below;
    let width = frame.buffer_mut().area.width;
    let mut below = row_below(
        width,
        (body, padded),
        next,
        (next_prose, next_user),
        measure,
        theme,
    );
    if let Some(progress) = fade {
        let area = below.0.area;
        crate::animation::fade_area(&mut below.0, area, progress, theme);
    }
    let view = ChatScroll {
        body,
        above: start,
        below: app.scroll_offset(),
    };
    app.set_chat_scroll(view, frame.buffer_mut(), below);
}

/// The chat's `line` below `padded`, drawn alone into a buffer a row high
/// and `width` wide, with its prose when it has an indent (`prose`), and
/// the band of a user's message under it when it is one (`user`).
fn row_below(
    width: u16,
    (body, padded): (Rect, Rect),
    line: Option<Line<'static>>,
    (prose, user): (Option<u16>, Option<BandEdge>),
    measure: Measure,
    theme: &Theme,
) -> (Buffer, Option<ProseRow>) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
    let row = Rect {
        y: 0,
        height: 1,
        ..padded
    }
    .intersection(buffer.area);
    let Some(line) = line else {
        return (buffer, None);
    };
    let prose_line = prose.map(|indent| (indent, line.clone()));
    Paragraph::new(line).render(row, &mut buffer);
    let prose = prose_line.map(|(indent, line)| {
        let (width, code) = if measure.is_grid() {
            prose_columns(&line, row, theme)
        } else {
            pack_prose(&mut buffer, &line, (row, indent), measure, theme)
        };
        ProseRow::new(&buffer, (row.x, row.y), (width, indent), code)
    });
    if let Some(edge) = user {
        let body = Rect {
            y: 0,
            height: 1,
            ..body
        };
        paint_user_band(&mut buffer, (body, row), &[(0, edge)], theme);
    }
    (buffer, prose)
}
