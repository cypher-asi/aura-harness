//! The right column's panel while a channel is open: its MEMBERS and its
//! INFO, and none of the agent app's tabs.
//!
//! MEMBERS is the coordinator, then the agents it spawned in a tree below
//! it, drawn as nested agents are under PROJECTS, each with the spinner
//! while it works and its code at the right:
//!
//! ```text
//!      Coordinator              CO
//!       └─ Worker               WK
//! ```
//!
//! INFO is what the channel is, as an info document: its slug, title
//! and description, who coordinates it, how many members it has, and
//! the project its agents work in. A right click anywhere in it opens
//! the channel's menu, whose "Edit channel…" sets or clears the project.
//!
//! ```text
//! #zode
//! Zode release
//! Ship the zode crate and its docs.
//!
//! COORDINATOR
//!   Coordinator  CO
//!
//! MEMBERS  2 agents
//!
//! PROJECT
//!   Zode  ~/code/zode
//! ```
//!
//! Without a project, PROJECT says so, wrapped to the column:
//!
//! ```text
//! PROJECT
//!   No project — agents work in the channel's folder
//! ```

use super::agents::projects::spawn_guides;
use super::agents::working_icon;
use super::chrome::{Body, ScrollView};
use super::side_list::render_empty;
use super::text::display_width;
use crate::app::{AgentRowEnd, HoverTarget, PanelFocus};
use crate::components::{cell_style, tree_row, CellState, Info, StatusIcon, Value};
use crate::events::{AgentSummary, ChannelSummary};
use crate::{App, Theme};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

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
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let visible = body.visible();
    app.set_channel_member_rows_area(body.rows());
    let first = app.sync_channel_members_scroll(visible);
    let width = body.width();
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
    body.draw(frame, lines, view, theme);
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

/// Draw what the open channel is into `inner`, the body below the tab bar
/// of the panel bordered by `area`. Without an open channel there is
/// nothing to draw.
pub(super) fn render_channel_info(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let Some(channel) = app.open_channel() else {
        return;
    };
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let lines = info_lines(app, channel, body.text_width(), theme);
    let first = app.sync_channel_info_scroll(lines.len(), body.visible());
    body.draw_document(frame, lines, first, theme);
}

/// The INFO tab's lines, `width` columns wide.
fn info_lines(
    app: &App,
    channel: &ChannelSummary,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme);
    info.note(&format!("#{}", channel.slug));
    if !channel.title.trim().is_empty() {
        info.title(&channel.title);
    }
    if !channel.description.trim().is_empty() {
        info.note(&channel.description);
    }

    info.section("COORDINATOR", None);
    match channel
        .members
        .iter()
        .find(|member| member.id == channel.coordinator_id)
    {
        Some(coordinator) => info.item(&coordinator.name, member_code(app, coordinator)),
        None => info.text("Not listed", Value::Quiet),
    }

    let count = channel.members.len();
    let unit = if count == 1 { "agent" } else { "agents" };
    info.section("MEMBERS", Some(&format!("{count} {unit}")));

    info.section("PROJECT", None);
    match &channel.project {
        // A project the app no longer lists comes without a folder.
        Some(project) => info.item(
            &project.name,
            Some(project.folder.as_str()).filter(|folder| !folder.is_empty()),
        ),
        None => info.text(NO_PROJECT, Value::Quiet),
    }
    info.into_lines()
}

/// What INFO says of a channel without a project.
const NO_PROJECT: &str = "No project — agents work in the channel's folder";
