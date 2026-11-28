//! The HISTORY tab of an open channel's side panel: every message the user
//! posted in the channel, newest first, two rows each.
//!
//! ```text
//! Your messages · 3
//!  Make the footer links open in a new tab
//!  20:31
//!  Plan the parser rewrite and split it…
//!  11-27 09:12
//! ```
//!
//! A message longer than [`POST_SUMMARY_MIN_CHARS`] shows its one-line
//! summary once the session sent one, else the message cut at a word.
//! Line breaks fold into spaces. The time is local: the minute for today,
//! the month, day and minute before. The highlight lights while the panel
//! has the focus; the pointer lights the item under it.

use super::chrome::{Body, ScrollView};
use super::side_list::render_empty;
use super::text::display_width;
use crate::app::{HistoryItem, HoverTarget, PanelFocus, HISTORY_ROWS};
use crate::components::{cell_style, CellState};
use crate::events::{wants_summary, POST_SUMMARY_MIN_CHARS};
use crate::{App, Theme};
use chrono::{DateTime, Local};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// What HISTORY says before the user posts in the channel.
pub(super) const NO_MESSAGES: &str = "No messages yet. What you post in this channel is listed here, newest first; pick one to jump to it.";
/// Columns a row leaves at its left before the text.
const INSET: usize = 1;

/// Draw the open channel's history into `inner`, the body below the tab
/// bar of the panel bordered by `area`.
pub(super) fn render_channel_history(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    app.set_channel_history_rows(None);
    if app.open_channel().is_none() {
        return;
    }
    let items = app.channel_history();
    if items.is_empty() || inner.height < 2 {
        render_empty(frame, inner, NO_MESSAGES, theme);
        return;
    }
    let heading = Rect { height: 1, ..inner };
    let rows = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    frame.render_widget(
        ratatui::widgets::Paragraph::new(heading_line(items.len(), theme)),
        heading,
    );
    let body = Body::new(area, rows, app.focus() == PanelFocus::Sandbox);
    app.set_channel_history_rows(Some(rows));
    let visible = (body.visible() / HISTORY_ROWS).max(1);
    let first = app.sync_channel_history_scroll(items.len(), visible);
    let width = body.width();
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .skip(first)
        .flat_map(|(index, item)| item_rows(app, (index, item), width, theme))
        .take(body.visible())
        .collect();
    let view = ScrollView {
        total: items.len() * HISTORY_ROWS,
        visible: body.visible(),
        first: first * HISTORY_ROWS,
    };
    body.draw(frame, lines, view, theme);
}

/// `Your messages · N`, in `muted`, the count `faint`.
fn heading_line(count: usize, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            " Your messages",
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {count}"),
            Style::default().fg(theme.colors.faint),
        ),
    ])
}

/// Item `index`'s two rows: what it says, then when it was sent.
fn item_rows(
    app: &App,
    (index, item): (usize, &HistoryItem),
    width: usize,
    theme: &Theme,
) -> [Line<'static>; HISTORY_ROWS] {
    let selected = app.focus() == PanelFocus::Sandbox && app.selected_history_item() == index;
    let hovered = app.hover() == Some(HoverTarget::ChannelHistoryRow(index));
    let state = CellState::of(selected, hovered);
    let text = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    let time = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let room = width.saturating_sub(INSET + 1);
    [
        row(&shown_text(item, room), text.bg, text.text, width),
        row(&compact_time(item.at_ms), time.bg, time.text, width),
    ]
}

/// One row: `text` after the inset, padded to `width` on `bg`.
fn row(text: &str, bg: Option<Color>, style: Style, width: usize) -> Line<'static> {
    let fill = bg.map_or_else(Style::default, |bg| Style::default().bg(bg));
    let lead = format!("{:INSET$}", "");
    let pad = width.saturating_sub(INSET + display_width(text));
    Line::from(vec![
        Span::styled(lead, fill),
        Span::styled(text.to_string(), fill.patch(style)),
        Span::styled(" ".repeat(pad), fill),
    ])
}

/// What an item shows in `room` columns: its summary when it has one and
/// is long enough to want one, else its text, cut at a word to fit.
pub(super) fn shown_text(item: &HistoryItem, room: usize) -> String {
    let text = item
        .summary
        .as_deref()
        .filter(|_| wants_summary(&item.text))
        .unwrap_or(&item.text);
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Without a summary, a long message is cut by the threshold too, so
    // a wide panel doesn't show what the summary would replace.
    let room = if item.summary.is_none() && wants_summary(&item.text) {
        room.min(POST_SUMMARY_MIN_CHARS)
    } else {
        room
    };
    cut_at_word(&folded, room)
}

/// `text` when it fits in `room` columns, else cut back to its last word
/// that fits (or mid-word when the first doesn't), ending in `…`.
pub(super) fn cut_at_word(text: &str, room: usize) -> String {
    if display_width(text) <= room {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut used = 0;
    let mut at_break = false;
    for c in text.chars() {
        let w = display_width(c.encode_utf8(&mut [0; 4]));
        if used + w + 1 > room {
            // Cut where a word ends: what was kept is whole words.
            at_break = c == ' ';
            break;
        }
        used += w;
        kept.push(c);
    }
    if !at_break {
        if let Some(space) = kept.rfind(' ').filter(|&at| at > 0) {
            kept.truncate(space);
        }
    }
    let kept = kept.trim_end_matches([' ', ',', ';', ':', '.']);
    format!("{kept}…")
}

/// `at_ms` in local time: `HH:MM` today, `MM-DD HH:MM` before.
pub(super) fn compact_time(at_ms: u64) -> String {
    let Some(at) = i64::try_from(at_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
    else {
        return String::new();
    };
    let local = at.with_timezone(&Local);
    if local.date_naive() == Local::now().date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%m-%d %H:%M").to_string()
    }
}
