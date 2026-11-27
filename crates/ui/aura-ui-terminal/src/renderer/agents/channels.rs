//! The agents panel's CHANNELS tab: the list of channels, one row per
//! channel, its `#` in the icon column as a project has its folder, then
//! its slug and nothing else, and "+ New channel" pinned to the bottom
//! row:
//!
//! ```text
//!  #  zode
//!  #  docs
//!
//!  +  New channel
//! ```
//!
//! The list is all the tab ever shows: the open channel's chat is in the
//! chat panel, and its members and details in the right column. While the
//! panel is focused the keyboard's row is lit; otherwise the open
//! channel's row is, as PROJECTS lights the agent on screen.

use super::{draw_list, split_pinned, working_icon};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{cell_style, list_row, CellState, StatusIcon};
use crate::events::ChannelSummary;
use crate::renderer::chrome::ScrollView;
use crate::renderer::side_list::empty_line;
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

/// What the list says when there are no channels.
const NO_CHANNELS: &str = "No channels yet.";

/// Draw the CHANNELS tab into `body`, the inside of the panel bordered by
/// `area`.
pub(super) fn render_channels(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let (rows_area, bottom_row) = split_pinned(body);
    let visible = usize::from(rows_area.height);
    app.set_channel_rows_area(rows_area);
    app.set_new_channel_area(bottom_row);
    app.sync_channels_scroll(visible);

    let width = usize::from(body.width);
    let lit = if is_focused {
        Some(app.selected_channel())
    } else {
        app.open_channel_row()
    };
    let first = app.channels_scroll();
    let lines: Vec<Line> = if app.channels().is_empty() {
        vec![empty_line(NO_CHANNELS, body.width, theme)]
    } else {
        app.channels()
            .iter()
            .enumerate()
            .skip(first)
            .take(visible)
            .map(|(index, channel)| {
                let state = row_state(app, index, lit == Some(index));
                channel_row(app, width, channel, state, theme)
            })
            .collect()
    };

    let new_index = app.channels().len();
    let state = row_state(app, new_index, is_focused && lit == Some(new_index));
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
    let new_channel = list_row(width, 0, icon, "New channel", cell, None);

    let view = ScrollView {
        total: app.channels().len(),
        visible,
        first,
    };
    let pinned = Some((bottom_row, new_channel));
    draw_list(
        frame,
        (area, rows_area),
        lines,
        pinned,
        view,
        theme,
        is_focused,
    );
}

/// Whether row `index` is lit, by the keyboard or as the open channel,
/// or under the pointer.
fn row_state(app: &App, index: usize, selected: bool) -> CellState {
    let hovered = app.hover() == Some(HoverTarget::ChannelRow(index));
    CellState::of(selected, hovered)
}

/// `#  slug`: a muted `#` in the icon column, then the channel's slug and
/// nothing else. A channel with a member at work shows the spinner in
/// place of its `#`.
fn channel_row<'a>(
    app: &App,
    width: usize,
    channel: &ChannelSummary,
    state: CellState,
    theme: &Theme,
) -> Line<'a> {
    let busy = channel
        .members
        .iter()
        .any(|member| app.agent_thinking(member));
    let icon = if busy {
        working_icon(app, theme)
    } else {
        StatusIcon::new("#", theme.colors.muted)
    };
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    list_row(width, 0, icon, &channel.slug, cell, None)
}
