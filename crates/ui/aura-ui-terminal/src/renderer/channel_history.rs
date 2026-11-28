//! The HISTORY tab of an open channel's side panel: every message the
//! user sent in the channel, oldest first, newest at the bottom, one row
//! each: what it says (a summary when long, see `app::apps::channels`)
//! and, at the right, when it was sent, `HH:MM` today and `MM-DD HH:MM`
//! before.
//!
//! ```text
//!  ›  Fix the parser escapes                     09:12
//!  ›  Add a history tab to the channel panel     10:40
//!  ›  @RE36 can you review it?                   10:52
//! ```
//!
//! The rows drawn ask the session for the summaries they still lack.

use super::chrome::{Body, ScrollView};
use super::side_list::{name_style, render_empty, Tone};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{cell_style, list_row, CellState, StatusIcon};
use crate::{App, Theme};
use chrono::{DateTime, Local, NaiveDate};
use ratatui::{layout::Rect, text::Line, Frame};

/// What HISTORY says while the user has sent nothing in the channel.
pub(super) const NO_MESSAGES: &str = "No messages from you yet.";

/// The glyph in each row's icon column: a sent message, as the prompt
/// marks one.
const SENT: &str = "›";

/// Draw the open channel's history into `inner`, the body below the tab
/// bar of the panel bordered by `area`. Without an open channel there is
/// nothing to draw.
pub(super) fn render_channel_history(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    if app.open_channel().is_none() {
        return;
    }
    app.refresh_channel_history();
    if app.channel_history().is_empty() {
        app.set_channel_history_rows_area(None);
        render_empty(frame, inner, NO_MESSAGES, theme);
        return;
    }
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let body = Body::new(area, inner, is_focused);
    let visible = body.visible();
    let first = app.sync_channel_history_scroll(visible);
    app.set_channel_history_rows_area(Some(body.rows()));
    app.request_channel_history_summaries(first..first + visible);
    let hovered = match app.hover() {
        Some(HoverTarget::ChannelHistoryRow(index)) => Some(index),
        _ => None,
    };
    let today = Local::now().date_naive();
    let selected = app.selected_channel_history();
    let lines: Vec<Line> = app
        .channel_history()
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(i, entry)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            let cell = cell_style(theme, state, name_style(Tone::Active, theme));
            let icon = StatusIcon::new(SENT, theme.colors.muted);
            let time = history_time(entry.at_ms, today);
            list_row(
                body.width(),
                0,
                icon,
                &app.channel_history_line(entry),
                cell,
                Some(&time),
            )
        })
        .collect();
    let view = ScrollView {
        total: app.channel_history().len(),
        visible,
        first,
    };
    body.draw(frame, lines, view, theme);
}

/// When a message sent at `at_ms` was sent, in local time: `HH:MM` on
/// `today`, `MM-DD HH:MM` before.
pub(super) fn history_time(at_ms: u64, today: NaiveDate) -> String {
    let Some(at) = i64::try_from(at_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map(|at| at.with_timezone(&Local))
    else {
        return String::new();
    };
    if at.date_naive() == today {
        at.format("%H:%M").to_string()
    } else {
        at.format("%m-%d %H:%M").to_string()
    }
}
