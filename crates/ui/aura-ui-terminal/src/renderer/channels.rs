//! The agents panel's CHANNELS tab: the list of channels, or the channel
//! open in it.
//!
//! The list is one row per channel, `#slug` with its title muted after
//! it, and "+ New channel" pinned to the bottom row:
//!
//! ```text
//!    #zode  Zode release
//!    #docs  Docs rewrite
//!
//!  +  New channel
//! ```
//!
//! An open channel shows a back row, its title, its description wrapped
//! to the panel, then its members: the coordinator, and the agents it
//! spawned in a tree below it, drawn as nested agents are under PROJECTS.
//!
//! ```text
//!    ‹ #zode
//!  Zode release
//!  Ship the zode crate and its docs.
//!
//!  MEMBERS
//!     Coordinator               CO
//!      └─ Worker                WK
//! ```

use super::panels::{
    cell_style, list_row, list_row_with_detail, render_scrollbar, spawn_guides, tree_row,
    truncate_record_info, working_icon, CellState, ScrollView, StatusIcon,
};
use super::policies::heading;
use super::side_list::empty_line;
use super::text::{display_width, wrap_words};
use crate::app::{AgentRowEnd, HoverTarget, PanelFocus};
use crate::events::{AgentSummary, ChannelSummary};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// What the list says when there are no channels.
const NO_CHANNELS: &str = "No channels yet.";

/// Draw the CHANNELS tab into `body`, the inside of the panel bordered by
/// `area`: the open channel if there is one, else the list.
pub(super) fn render_channels(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    match app.open_channel().cloned() {
        Some(channel) => render_open_channel(frame, (area, body), app, &channel, theme),
        None => render_channel_list(frame, (area, body), app, theme),
    }
}

/// Whether row `index` of the view showing is lit by the keyboard or
/// under the pointer.
fn row_state(app: &App, index: usize, selected: bool) -> CellState {
    let hovered = app.hover() == Some(HoverTarget::ChannelRow(index));
    CellState::of(selected, hovered)
}

fn render_channel_list(
    frame: &mut Frame,
    (area, body): (Rect, Rect),
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let rows_area = Rect {
        height: body.height.saturating_sub(1),
        ..body
    };
    let bottom_row = Rect {
        y: body.y + rows_area.height,
        height: body.height.min(1),
        ..body
    };
    let visible = usize::from(rows_area.height);
    app.set_channel_rows_area(rows_area);
    app.set_new_channel_area(bottom_row);
    app.sync_channels_scroll(visible);

    let width = usize::from(body.width);
    let selected = app.selected_channel();
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
                let state = row_state(app, index, is_focused && index == selected);
                channel_row(app, width, channel, state, theme)
            })
            .collect()
    };

    let new_index = app.channels().len();
    let state = row_state(app, new_index, is_focused && selected == new_index);
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
    let new_channel = list_row(width, 0, icon, "New channel", cell, None);

    let view = ScrollView {
        total: app.channels().len(),
        visible,
        first,
    };
    frame.render_widget(Paragraph::new(lines), rows_area);
    frame.render_widget(Paragraph::new(new_channel), bottom_row);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
}

/// `#slug  Title`: the title muted, dropped when the row is too narrow.
/// A channel with a member at work shows the spinner in the icon column.
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
        StatusIcon::new("", theme.colors.muted)
    };
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.foreground));
    let slug = format!("#{}", channel.slug);
    let title = Some(channel.title.as_str()).filter(|title| !title.is_empty());
    list_row_with_detail(width, 0, icon, (&slug, title), cell, None)
}

fn render_open_channel(
    frame: &mut Frame,
    (area, body): (Rect, Rect),
    app: &mut App,
    channel: &ChannelSummary,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let width = usize::from(body.width);
    let back_row = Rect {
        height: body.height.min(1),
        ..body
    };
    app.set_channel_back_area(back_row);
    let state = row_state(app, 0, is_focused && app.selected_channel_row() == 0);
    let cell = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let back = list_row(
        width,
        0,
        StatusIcon::new("", theme.colors.muted),
        &format!("‹ #{}", channel.slug),
        cell,
        None,
    );
    frame.render_widget(Paragraph::new(back), back_row);

    let about = about_lines(channel, width, theme);
    let about_height = u16::try_from(about.len())
        .unwrap_or(u16::MAX)
        .min(body.height.saturating_sub(back_row.height));
    let about_area = Rect {
        y: back_row.y + back_row.height,
        height: about_height,
        ..body
    };
    frame.render_widget(Paragraph::new(about), about_area);

    let rows_top = about_area.y + about_area.height;
    let rows_area = Rect {
        y: rows_top,
        height: (body.y + body.height).saturating_sub(rows_top),
        ..body
    };
    if rows_area.height == 0 {
        return;
    }
    let visible = usize::from(rows_area.height);
    app.set_channel_member_rows_area(rows_area);
    app.sync_channel_members_scroll(visible);
    let first = app.channel_members_scroll();
    let code_width = channel
        .members
        .iter()
        .filter_map(|member| member_code(app, member))
        .map(display_width)
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = (first..channel.members.len())
        .take(visible)
        .map(|index| member_row(app, (width, code_width), &channel.members, index, theme))
        .collect();
    let view = ScrollView {
        total: channel.members.len(),
        visible,
        first,
    };
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
}

/// The lines between the back row and the members: the title in bold,
/// the description wrapped and muted, a blank line and the `MEMBERS`
/// heading.
fn about_lines(channel: &ChannelSummary, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let room = width.saturating_sub(2);
    let title = truncate_record_info(&channel.title, room);
    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if !channel.description.trim().is_empty() {
        let muted = Style::default().fg(theme.colors.muted);
        lines.extend(
            wrap_words(&channel.description, room)
                .into_iter()
                .map(|line| Line::from(Span::styled(format!(" {line}"), muted))),
        );
    }
    lines.push(Line::default());
    lines.push(heading("MEMBERS", None, theme));
    lines
}

/// Member `index` of `members`: under the guides from the agent that
/// spawned it, with the spinner while it works and its code at the
/// right, in a column `code_width` wide. Lit by the keyboard while the
/// panel is focused, else when it is the agent in the chat.
fn member_row<'a>(
    app: &App,
    (width, code_width): (usize, usize),
    members: &[AgentSummary],
    index: usize,
    theme: &Theme,
) -> Line<'a> {
    let member = &members[index];
    let row = 1 + index;
    let selected = if app.focus() == PanelFocus::Swarm {
        app.selected_channel_row() == row
    } else {
        member.id == app.active_agent_id()
    };
    let name = if member.depth == 0 {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let cell = cell_style(
        theme,
        row_state(app, row, selected),
        Style::default().fg(name),
    );
    let icon = if app.agent_thinking(member) {
        working_icon(app, theme)
    } else {
        StatusIcon::new("", theme.colors.muted)
    };
    let end = AgentRowEnd::definition(width, (member_code(app, member), code_width));
    tree_row(
        width,
        (&spawn_guides(members, index), theme.colors.muted),
        icon,
        (&member.name, None),
        cell,
        &end,
        &[],
    )
}

/// The member's own code (`RE36`), or, for an agent without one, the
/// abbreviation of the permanent agent it is an instance of.
fn member_code<'a>(app: &'a App, member: &'a AgentSummary) -> Option<&'a str> {
    if let Some(code) = member.code.as_deref() {
        return Some(code);
    }
    let definition = member.definition.as_deref()?;
    app.definitions()
        .iter()
        .find(|known| known.name == definition)
        .and_then(|known| known.abbreviation.as_deref())
}
