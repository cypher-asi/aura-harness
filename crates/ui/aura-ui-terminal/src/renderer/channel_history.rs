//! The open channel's HISTORY side tab: every message the user sent in
//! the channel, newest first, one row each, with when it was sent at the
//! right: `HH:MM` today, else the day.
//!
//! ```text
//!  Make parser escapes round trip             14:02
//!  @REN3 can you look at the failing test?    13:47
//!  Ship the zode crate and its docs          Nov 27
//! ```
//!
//! A long message is listed by its summary, or cut at a word with `…`
//! until the summary comes (see `app::apps::channels::history`).

use super::chrome::Body;
use super::side_list::render_empty;
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{cell_style, list_row, CellState, StatusIcon};
use crate::renderer::chrome::ScrollView;
use crate::{App, Theme};
use aura_core_types::time::now_ms;
use chrono::{DateTime, Local};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

/// What HISTORY says before the user has sent anything.
const EMPTY: &str = "No messages sent yet.";

/// Draw HISTORY into `inner`, the body below the tab bar of the panel
/// bordered by `area`.
pub(super) fn render_channel_history(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let entries = app.channel_history().to_vec();
    if entries.is_empty() {
        render_empty(frame, inner, EMPTY, theme);
        return;
    }
    let focused = app.focus() == PanelFocus::Sandbox;
    let body = Body::new(area, inner, focused);
    let visible = body.visible();
    app.set_channel_history_rows_area(body.rows());
    let first = app.sync_channel_history_scroll(visible);
    let width = body.width();
    let now = now_ms();
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, entry)| {
            let selected = focused && app.selected_channel_history() == index;
            let hovered = app.hover() == Some(HoverTarget::ChannelHistoryRow(index));
            let text = Style::default().fg(theme.colors.foreground);
            let cell = cell_style(theme, CellState::of(selected, hovered), text);
            let label = app.channel_history_label(entry);
            let time = sent_at(entry.at_ms, now);
            let icon = StatusIcon::new("", theme.colors.muted);
            list_row(width, 0, icon, &label, cell, Some(&time))
        })
        .collect();
    let view = ScrollView {
        total: entries.len(),
        visible,
        first,
    };
    body.draw(frame, lines, view, theme);
}

/// When a message sent at `at_ms` was sent, said for `now_ms`: `HH:MM`
/// on the same local day, else the day as `Nov 27`.
#[must_use]
pub(crate) fn sent_at(at_ms: u64, now_ms: u64) -> String {
    let local = |ms: u64| {
        i64::try_from(ms)
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .map(|at| at.with_timezone(&Local))
    };
    let (Some(at), Some(now)) = (local(at_ms), local(now_ms)) else {
        return String::new();
    };
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::sent_at;

    #[test]
    fn a_message_of_today_shows_its_time_and_an_older_one_its_day() {
        let now = 1_764_300_000_000;
        let today = sent_at(now - 60_000, now);
        assert_eq!(today.len(), 5, "{today}");
        assert_eq!(today.as_bytes()[2], b':');
        let older = sent_at(now - 3 * 86_400_000, now);
        assert!(
            older.chars().next().is_some_and(char::is_alphabetic),
            "{older}"
        );
    }
}
