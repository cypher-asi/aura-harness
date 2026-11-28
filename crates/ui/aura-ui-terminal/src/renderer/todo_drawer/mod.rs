//! The to-do drawer, between the Chat body and the input, in the same
//! column of borders: the Chat body's bottom border is the line above it,
//! its own bottom border the line above whatever comes next.
//!
//! ```text
//! ├──────────────────────────────────────────────┤
//! │  ⬢ Working on 2 active · 1 done           —  │
//! │ ◐ Write the lexer                            │
//! │   FEN6 · working 4m                          │
//! │ ☐ Run the tests                              │
//! │   REV6 · idle                                │
//! │ ▸ Finished to-dos  1                         │
//! ├──────────────────────────────────────────────┤
//! │  → Add a follow-up                           │
//! ```
//!
//! The header keeps the side padding of a panel's headings, the items a
//! list's column at the left, and neither a blank row on top: the drawer
//! is as tall as its rows.
//!
//! The header counts the items still open and those done, then a toggle,
//! `—` to fold the drawer to the header, `+` to open it. Folded, the item
//! in progress follows the count, so the current step stays in sight.
//! Open, each item still open takes two rows, as a member does on the
//! side panel: its text, then in gray the agent it was given to and how
//! that agent is doing ([`rows`]). Closed items only show under the
//! "Finished to-dos" row, once it is opened. Past [`MAX_TODO_ROWS`] rows,
//! or past the rows the screen leaves it, the items on screen start at
//! the item in progress so it stays in view, keep the history row, and a
//! `+N more` row counts the rest. An opened history starts them at its
//! own row instead, and the wheel scrolls them from there.
//!
//! The drawer records where it drew its header, its history row, each
//! item and the rows the wheel scrolls on the [`App`] for the mouse.

mod rows;

use super::chrome::{border_color, PAD_SIDE};
use super::hud::{truncate, MAX_TODO_ROWS};
use super::input::join_divider;
use super::text::display_width;
use crate::app::{AppState, PanelFocus};
use crate::events::TodoState;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Rows the border adds: only the bottom one, since the Chat body's
/// bottom border sits on top.
const BORDER_ROWS: u16 = 1;
/// Narrowest the item in progress is shown in a folded header; narrower
/// room leaves it out.
const MIN_FOLDED_TEXT: usize = 8;

/// Rows the drawer wants: none when it has nothing to show, the header
/// alone when folded, else the header, the rows of the items on screen
/// and a `+N more` row when some are hidden.
#[must_use]
pub(super) fn height(app: &App) -> u16 {
    if !app.todo_drawer_shown() {
        return 0;
    }
    if app.todo_drawer_folded() {
        return BORDER_ROWS + 1;
    }
    let entries = rows::entries(app);
    let start = rows::history_start(app, &entries);
    let (shown, more) = rows::fit(&entries, MAX_TODO_ROWS, start);
    u16::try_from(rows::rows_of(&entries, &shown) + usize::from(more))
        .unwrap_or(u16::MAX)
        .saturating_add(BORDER_ROWS + 1)
}

/// Render the drawer into `area`, which [`height`] sized, and record
/// where its header went.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if !app.todo_drawer_shown() || area.height < BORDER_ROWS + 1 || area.width < 6 {
        forget_rows(app);
        app.set_todo_drawer_header(None);
        return;
    }
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color(app.focus() == PanelFocus::Chat, theme)));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    join_divider(frame, area);

    let header = Rect { height: 1, ..inner };
    frame.render_widget(Paragraph::new(header_line(app, inner.width, theme)), header);
    app.set_todo_drawer_header(Some(header));

    let room = usize::from(inner.height.saturating_sub(1));
    if app.todo_drawer_folded() || room == 0 {
        forget_rows(app);
        return;
    }
    draw_rows(frame, app, (header, inner.bottom(), room), theme);
}

/// Nothing under the header was drawn: no history row, items or rows to
/// scroll for the mouse.
fn forget_rows(app: &mut App) {
    app.set_todo_history_row(None);
    app.set_todo_item_hits(Vec::new());
    let first = app.todo_history_scroll();
    app.set_todo_history_scroll(first, None);
}

/// Draw the rows under `header`, down to `bottom`, at most `room` of
/// them, and record where the history row, each item and the rows the
/// wheel scrolls went.
fn draw_rows(
    frame: &mut Frame,
    app: &mut App,
    (header, bottom, room): (Rect, u16, usize),
    theme: &Theme,
) {
    let entries = rows::entries(app);
    let start = rows::history_start(app, &entries);
    let (shown, more) = rows::fit(&entries, room.min(MAX_TODO_ROWS), start);
    // A column clear of each border.
    let text_room = usize::from(header.width.saturating_sub(2));
    let mut y = header.y + 1;
    let mut history = None;
    let mut hits = Vec::new();
    for &index in &shown {
        let entry = entries[index];
        if matches!(entry, rows::Entry::History(_)) && y < bottom {
            history = Some(Rect { y, ..header });
        }
        let top = y;
        // An item the screen cuts short keeps its first row; past `bottom`
        // are the border and the input.
        for mut line in rows::entry_lines(app, entry, text_room, theme) {
            if y >= bottom {
                break;
            }
            line.spans.insert(0, Span::raw(" "));
            frame.render_widget(Paragraph::new(line), Rect { y, ..header });
            y += 1;
        }
        if let rows::Entry::Open(todo) | rows::Entry::Closed(todo) = entry {
            let area = Rect {
                y: top,
                height: y - top,
                ..header
            };
            hits.push((area, todo.assignee.clone()));
        }
    }
    let scroll_rows = (start.is_some() && more).then(|| Rect {
        y: header.y + 1,
        height: y - header.y - 1,
        ..header
    });
    // The wheel moves on from where the window was drawn, not from past
    // its end; a history that fits keeps its start for when it no longer
    // does.
    let first = match start {
        Some(_) if more => shown.first().copied().unwrap_or(0),
        _ => app.todo_history_scroll(),
    };
    let hidden = entries.len() - shown.len();
    app.set_todo_history_scroll(first, scroll_rows);
    app.set_todo_item_hits(hits);
    if more && hidden > 0 && y < bottom {
        let more = Line::from(Span::styled(
            format!(" +{hidden} more"),
            Style::default().fg(theme.colors.muted),
        ));
        frame.render_widget(Paragraph::new(more), Rect { y, ..header });
    }
    app.set_todo_history_row(history);
}

/// `  ⬢ Working on 2 active · 5 done` at the left, with the item in
/// progress when folded, and the toggle at the right.
fn header_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let hud = app.hud();
    let working = app.state() == AppState::Processing;
    let shimmer = (working && theme.animate_spinners).then(|| app.animation_frame());
    let pad = " ".repeat(usize::from(PAD_SIDE));
    let mut spans = vec![Span::raw(pad.clone())];
    spans.extend(rows::count_spans(app, theme, working, shimmer));

    let folded = app.todo_drawer_folded();
    let toggle = if folded { "+" } else { "—" };
    let toggle_color = if app.todo_drawer_hovered() {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let width = usize::from(width);
    let used =
        |spans: &[Span<'_>]| -> usize { spans.iter().map(|s| display_width(&s.content)).sum() };
    // The toggle, a space before it and the closing pad.
    let tail = display_width(toggle) + 1 + pad.len();
    let current = hud
        .todos
        .iter()
        .find(|todo| todo.state == TodoState::InProgress)
        .filter(|_| folded);
    if let Some(todo) = current {
        let room = width.saturating_sub(used(&spans) + 2 + tail);
        if room >= MIN_FOLDED_TEXT {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate(&todo.content.replace('\n', " "), room),
                Style::default().fg(theme.colors.foreground),
            ));
        }
    }
    let fill = width.saturating_sub(used(&spans) + display_width(toggle) + pad.len());
    spans.extend([
        Span::raw(" ".repeat(fill)),
        Span::styled(toggle, Style::default().fg(toggle_color)),
        Span::raw(pad),
    ]);
    Line::from(spans)
}

#[cfg(test)]
mod tests;
