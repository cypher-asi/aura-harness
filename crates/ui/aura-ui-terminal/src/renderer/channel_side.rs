//! The right column's panel while a channel is open: its MEMBERS and its
//! INFO, and none of the agent app's tabs.
//!
//! MEMBERS is the coordinator, then the agents it spawned in a tree below
//! it, drawn as nested agents are under PROJECTS, each with the spinner
//! while it works and its code at the right:
//!
//! ```text
//!     Coordinator               CO
//!      └─ Worker                WK
//! ```
//!
//! INFO is what the channel is: its slug, title and description, who
//! coordinates it, how many members it has, and the projects linked to
//! it. A right click anywhere in it opens the channel's menu.
//!
//! ```text
//!  #zode
//!  Zode release
//!  Ship the zode crate and its docs.
//!
//!  COORDINATOR
//!     Coordinator  CO
//!
//!  MEMBERS  2 agents
//!
//!  LINKED PROJECTS
//!     Zode  ~/code/zode
//! ```

use super::agents::projects::spawn_guides;
use super::agents::working_icon;
use super::chrome::{render_scrollbar, ScrollView};
use super::policies::heading;
use super::side_list::{empty_line, render_empty, render_hint, split_hint, CHANNEL_MEMBERS_HINT};
use super::text::{display_width, wrap_words};
use crate::app::{AgentRowEnd, HoverTarget, PanelFocus};
use crate::components::{
    cell_style, list_row_with_detail, tree_row, truncate_record_info, CellState, StatusIcon,
};
use crate::events::{AgentSummary, ChannelSummary};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Draw the open channel's members into `inner`, the body below the tab
/// bar of the panel bordered by `area`. Without an open channel there is
/// nothing to draw.
pub(super) fn render_channel_members(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let Some(channel) = app.open_channel().cloned() else {
        return;
    };
    if channel.members.is_empty() {
        render_empty(frame, inner, "No members yet.", theme);
        return;
    }
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = usize::from(rows_area.height);
    app.set_channel_member_rows_area(rows_area);
    let first = app.sync_channel_members_scroll(visible);
    let width = usize::from(rows_area.width);
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
    if let Some(hint) = hint {
        render_hint(frame, hint, CHANNEL_MEMBERS_HINT, theme);
    }
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
    let selected = if app.focus() == PanelFocus::Sandbox {
        app.selected_channel_member() == index
    } else {
        member.id == app.active_agent_id()
    };
    let hovered = app.hover() == Some(HoverTarget::ChannelMemberRow(index));
    let name = if member.depth == 0 {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let cell = cell_style(
        theme,
        CellState::of(selected, hovered),
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

/// Draw what the open channel is into `inner`, cut to its rows. Without
/// an open channel there is nothing to draw.
pub(super) fn render_channel_info(frame: &mut Frame, inner: Rect, app: &App, theme: &Theme) {
    let Some(channel) = app.open_channel() else {
        return;
    };
    let lines = info_lines(app, channel, inner.width, theme);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The INFO tab's lines, `width` columns wide.
fn info_lines(
    app: &App,
    channel: &ChannelSummary,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let columns = usize::from(width);
    let room = columns.saturating_sub(2);
    let muted = Style::default().fg(theme.colors.muted);
    let plain = cell_style(theme, CellState::Normal, Style::default());

    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            truncate_record_info(&format!("#{}", channel.slug), room),
            muted,
        ),
    ])];
    if !channel.title.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                truncate_record_info(&channel.title, room),
                Style::default()
                    .fg(theme.colors.foreground)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !channel.description.trim().is_empty() {
        lines.extend(
            wrap_words(&channel.description, room)
                .into_iter()
                .map(|line| Line::from(Span::styled(format!(" {line}"), muted))),
        );
    }
    lines.push(Line::default());

    lines.push(heading("COORDINATOR", None, theme));
    match channel
        .members
        .iter()
        .find(|member| member.id == channel.coordinator_id)
    {
        Some(coordinator) => lines.push(list_row_with_detail(
            columns,
            0,
            StatusIcon::new("", theme.colors.muted),
            (&coordinator.name, member_code(app, coordinator)),
            plain,
            None,
        )),
        None => lines.push(empty_line("Not listed", width, theme)),
    }
    lines.push(Line::default());

    let count = channel.members.len();
    let unit = if count == 1 { "agent" } else { "agents" };
    lines.push(heading("MEMBERS", Some(format!("{count} {unit}")), theme));
    lines.push(Line::default());

    lines.push(heading("LINKED PROJECTS", None, theme));
    if channel.projects.is_empty() {
        lines.push(empty_line("No linked projects", width, theme));
    }
    lines.extend(channel.projects.iter().map(|project| {
        list_row_with_detail(
            columns,
            0,
            StatusIcon::new("", theme.colors.muted),
            (&project.name, Some(&project.folder)),
            plain,
            None,
        )
    }));
    lines
}
