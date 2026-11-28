//! The to-do drawer, between the Chat body and the input, in the same
//! column of borders: the Chat body's bottom border is the line above it,
//! its own bottom border the line above whatever comes next.
//!
//! ```text
//! ├──────────────────────────────────────────────┤
//! │  ⬢ Working on 2 active · 1 done           —  │
//! │ ◐ Write the lexer       → FEN6 · working 4m  │
//! │ ☐ Run the tests         → REV6 · idle        │
//! │ ▸ Finished to-dos  1                            │
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
//! Open, each item still open is one row, with the agent it was given to
//! ([`rows`]). Closed items only show under the "Finished to-dos" row, once
//! it is opened. Past [`MAX_TODO_ROWS`] rows, or past the rows the screen
//! leaves it, the rows on screen start at the item in progress so it
//! stays in view, keep the history row, and a `+N more` row counts the
//! rest.
//!
//! The drawer records where it drew its header and its history row on
//! the [`App`] for the mouse.

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
/// alone when folded, else the header, the items on screen and a
/// `+N more` row when some are hidden.
#[must_use]
pub(super) fn height(app: &App) -> u16 {
    if !app.todo_drawer_shown() {
        return 0;
    }
    if app.todo_drawer_folded() {
        return BORDER_ROWS + 1;
    }
    let len = rows::entries(app).len();
    let shown = len.min(MAX_TODO_ROWS);
    let more = usize::from(len > shown);
    u16::try_from(shown + more)
        .unwrap_or(u16::MAX)
        .saturating_add(BORDER_ROWS + 1)
}

/// Render the drawer into `area`, which [`height`] sized, and record
/// where its header went.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if !app.todo_drawer_shown() || area.height < BORDER_ROWS + 1 || area.width < 6 {
        app.set_todo_drawer_header(None);
        app.set_todo_history_row(None);
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
        app.set_todo_history_row(None);
        return;
    }
    draw_rows(frame, app, (header, inner.bottom(), room), theme);
}

/// Draw the rows under `header`, down to `bottom`, at most `room` of
/// them, and record where the history row went.
fn draw_rows(
    frame: &mut Frame,
    app: &mut App,
    (header, bottom, room): (Rect, u16, usize),
    theme: &Theme,
) {
    let entries = rows::entries(app);
    let (shown, more) = rows::fit(&entries, room.min(MAX_TODO_ROWS));
    // A column clear of each border.
    let text_room = usize::from(header.width.saturating_sub(2));
    let mut y = header.y + 1;
    let mut history = None;
    for &index in &shown {
        let entry = entries[index];
        if matches!(entry, rows::Entry::History(_)) {
            history = Some(Rect { y, ..header });
        }
        let mut line = rows::entry_line(app, entry, text_room, theme);
        line.spans.insert(0, Span::raw(" "));
        frame.render_widget(Paragraph::new(line), Rect { y, ..header });
        y += 1;
    }
    let hidden = entries.len() - shown.len();
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
