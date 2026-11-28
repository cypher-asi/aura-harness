//! The HISTORY tab of an open channel's side panel: every message the user
//! sent in the channel, newest first, drawn as MEMBERS draws its members,
//! two rows each.
//!
//! ```text
//!  #  Make the footer links open in a new tab
//!     20:31 · to members
//!  →  Plan the parser rewrite and split it…
//!     11-27 09:12 · to the coordinator
//!  P  Split the lexer from the parser
//!     11-27 09:05 · plan · RUS3
//! ```
//!
//! The first row is a list row: the kind of message in the icon column
//! (`#` a post to the members, `→` a message typed to the coordinator,
//! `P` a `/plan` request), then what it says. The second row is the
//! muted detail row MEMBERS draws under each member: when it was sent and
//! where it went, a `/plan` request with the code of the member planning
//! it. Nothing is bold and only the icon column is gray.
//!
//! A message longer than [`POST_SUMMARY_MIN_CHARS`] shows its one-line
//! summary once the session sent one, else the message cut at a word.
//! Line breaks fold into spaces. The time is local: the minute for today,
//! the month, day and minute before. The highlight lights while the panel
//! has the focus; the pointer lights the item under it.

use super::chrome::{Body, ScrollView};
use super::side_list::render_empty;
use super::text::display_width;
use crate::animation::AURA_WIDTH;
use crate::app::{HistoryItem, HistoryKind, HoverTarget, PanelFocus, HISTORY_ROWS};
use crate::components::{cell_style, detail_row, list_row, CellState, StatusIcon, ROW_INSET};
use crate::events::{wants_summary, ChannelSummary, POST_SUMMARY_MIN_CHARS};
use crate::{App, Theme};
use chrono::{DateTime, Local};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

/// What HISTORY says before the user posts in the channel.
pub(super) const NO_MESSAGES: &str = "No messages yet. What you post in this channel is listed here, newest first; pick one to jump to it.";

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
    let Some(channel) = app.open_channel().cloned() else {
        return;
    };
    let items = app.channel_history();
    if items.is_empty() {
        render_empty(frame, inner, NO_MESSAGES, theme);
        return;
    }
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    app.set_channel_history_rows(Some(body.rows()));
    // A panel too short for an item's two rows still shows its text.
    let visible = (body.visible() / HISTORY_ROWS).max(1);
    let first = app.sync_channel_history_scroll(items.len(), visible);
    let width = body.width();
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .skip(first)
        .flat_map(|(index, item)| item_rows(app, &channel, (index, item), width, theme))
        .take(body.visible())
        .collect();
    let view = ScrollView {
        total: items.len() * HISTORY_ROWS,
        visible: body.visible(),
        first: first * HISTORY_ROWS,
    };
    body.draw(frame, lines, view, theme);
}

/// Item `index`'s two rows: its kind and what it says, then, muted, when
/// it was sent and where it went.
fn item_rows(
    app: &App,
    channel: &ChannelSummary,
    (index, item): (usize, &HistoryItem),
    width: usize,
    theme: &Theme,
) -> [Line<'static>; HISTORY_ROWS] {
    let selected = app.focus() == PanelFocus::Sandbox && app.selected_history_item() == index;
    let hovered = app.hover() == Some(HoverTarget::ChannelHistoryRow(index));
    let state = CellState::of(selected, hovered);
    let text = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    let room = width.saturating_sub(2 * ROW_INSET + AURA_WIDTH + 1);
    let icon = StatusIcon::new(kind_mark(item.kind), theme.colors.muted);
    let first = list_row(width, 0, icon, &shown_text(item, room), text, None);
    let detail = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let second = detail_row(
        width,
        "",
        &detail_text(channel, item),
        (detail.bg, detail.text),
        theme,
    );
    [first, second]
}

/// The icon column of an item: where the message went.
const fn kind_mark(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Post => "#",
        HistoryKind::Typed => "→",
        HistoryKind::Plan => "P",
    }
}

/// An item's second row: `20:31 · to members`, `… · to the
/// coordinator`, or `… · plan · RUS3` with the code of the member
/// planning it, when it is still in the channel.
pub(super) fn detail_text(channel: &ChannelSummary, item: &HistoryItem) -> String {
    let to = match item.kind {
        HistoryKind::Post => "to members".to_string(),
        HistoryKind::Typed => "to the coordinator".to_string(),
        HistoryKind::Plan => {
            let planner = item.planner.as_deref().and_then(|id| {
                let member = channel.members.iter().find(|member| member.id == id)?;
                member.code.clone().or_else(|| Some(member.name.clone()))
            });
            planner.map_or_else(|| "plan".to_string(), |code| format!("plan · {code}"))
        }
    };
    let time = compact_time(item.at_ms);
    if time.is_empty() {
        to
    } else {
        format!("{time} · {to}")
    }
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
