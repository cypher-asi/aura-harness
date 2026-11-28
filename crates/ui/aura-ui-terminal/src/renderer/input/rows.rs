//! Which of the input's rows are on screen, and those rows cut out of
//! the styled text: a draft of any length costs a box of rows a frame.

use crate::app::{App, InputRows};
use ratatui::text::Span;

/// Cut rows `window` of `spans` out at the char offsets in `starts`, one
/// row per start even when the text ends in a newline, and one per row of
/// `window` past the text's end. Newlines count as chars for the offsets
/// but are not drawn: ratatui would show them as a blank cell at best.
/// Spans before the window are only counted, so the cost is the text's
/// length in chars plus the window's width in pushes.
pub(in crate::renderer) fn split_rows(
    spans: &[Span<'_>],
    starts: &[usize],
    window: std::ops::Range<usize>,
) -> Vec<Vec<Span<'static>>> {
    let count = window.len();
    let mut rows: Vec<Vec<Span<'static>>> = Vec::with_capacity(count);
    let Some(&from) = starts.get(window.start) else {
        rows.resize_with(count, Vec::new);
        return rows;
    };
    let to = starts.get(window.end).copied().unwrap_or(usize::MAX);
    rows.push(Vec::new());
    let mut next = starts
        .iter()
        .skip(window.start + 1)
        .take(count.saturating_sub(1))
        .peekable();
    let mut index = 0;
    for span in spans {
        let len = span.content.chars().count();
        if index + len <= from {
            index += len;
            continue;
        }
        for c in span.content.chars() {
            if index >= to {
                break;
            }
            if index < from {
                index += 1;
                continue;
            }
            if next.peek().is_some_and(|&&start| start == index) {
                next.next();
                rows.push(Vec::new());
            }
            index += 1;
            if c == '\n' {
                continue;
            }
            if let Some(row) = rows.last_mut() {
                match row.last_mut() {
                    Some(last) if last.style == span.style => last.content.to_mut().push(c),
                    _ => row.push(Span::styled(c.to_string(), span.style)),
                }
            }
        }
        if index >= to {
            break;
        }
    }
    rows.resize_with(count, Vec::new);
    rows
}

/// Which of the `total` rows of text are shown `visible` at a time.
pub(super) fn shown_rows(
    app: &App,
    cursor: (usize, usize),
    total: usize,
    visible: usize,
) -> InputRows {
    // Once the input outgrows its rows, show where the wheel scrolled it
    // to, or else keep the cursor's row in view.
    let first = app.input_scroll_top().map_or_else(
        || (cursor.0 + 1).saturating_sub(visible),
        |top| top.min(total.saturating_sub(visible)),
    );
    InputRows {
        top: first,
        total,
        visible,
    }
}
