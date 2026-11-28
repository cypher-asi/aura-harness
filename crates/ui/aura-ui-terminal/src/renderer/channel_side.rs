//! The right column's panel while a channel is open: its MEMBERS and its
//! INFO, and none of the agent app's tabs.
//!
//! MEMBERS is the coordinator, then the agents it started, each with the
//! subagents it spawned in a tree below it, drawn as nested agents are
//! under PROJECTS. Each takes two rows: the permanent agent it is an
//! instance of (`subagent` for a subagent of none), with its code at the
//! right, then in gray what it works on, its title (a subagent's
//! subtask), else `Working…` or `Idle`. A member has the `@` that marks a
//! permanent agent on AGENTS in its icon column, and a subagent the line
//! from the member that spawned it; either shows the spinner there while
//! it works. A member that runs in the cloud has the `☁` PROJECTS shows
//! after its name, with its owner's name when it is another user's:
//!
//! ```text
//!  @  coordinator               CO
//!     Idle
//!  @  rust-engineer ☁         REN3
//!     Fix parser escapes
//!     └── subagent            SB12
//!         Write the escape tests
//! ```
//!
//! INFO is what the channel is, as an info document: its slug, title
//! and description, who coordinates it, how many members it has, the
//! project its agents work in and where the members it starts run. A
//! right click anywhere in it opens the channel's menu, whose "Edit
//! channel…" changes the project and where agents run.
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
//!
//! AGENTS RUN
//!   In the cloud
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
use super::modals::placement_name;
use super::side_list::render_empty;
use super::text::display_width;
use crate::animation::AURA_WIDTH;
use crate::app::{AgentRowEnd, HoverTarget, PanelFocus, CHANNEL_MEMBER_ROWS};
use crate::components::{
    cell_style, tree_row, truncate_record_info, CellState, Info, RowBadge, StatusIcon, Value,
    ROW_INSET,
};
use crate::events::{AgentSummary, ChannelSummary};
use crate::host::PlacementView;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    Frame,
};

/// What a member's second row says while it works with no title.
const WORKING: &str = "Working…";
/// What a member's second row says while it waits with no title.
const IDLE: &str = "Idle";

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
    // A panel too short for one member's two rows still shows its name.
    let first = app.sync_channel_members_scroll((visible / CHANNEL_MEMBER_ROWS).max(1));
    let width = body.width();
    let code_width = channel
        .members
        .iter()
        .filter_map(|member| member_code(app, member))
        .map(display_width)
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = (first..channel.members.len())
        .flat_map(|index| member_rows(app, (width, code_width), &channel.members, index, theme))
        .take(visible)
        .collect();
    let view = ScrollView {
        total: channel.members.len() * CHANNEL_MEMBER_ROWS,
        visible,
        first: first * CHANNEL_MEMBER_ROWS,
    };
    body.draw(frame, lines, view, theme);
}

/// Member `index` of `members`, on two rows: the permanent agent it is an
/// instance of, under the guides from the agent that spawned it, after
/// its `@` (a subagent has none, so its guides run on through the icon
/// column) or the spinner while it works, and its cloud if it runs in
/// the cloud, with its code at the right, in a column `code_width` wide;
/// then, in gray, what it works on. Lit by the keyboard while the panel
/// is focused, else when it is the agent in the chat.
fn member_rows<'a>(
    app: &App,
    (width, code_width): (usize, usize),
    members: &[AgentSummary],
    index: usize,
    theme: &Theme,
) -> [Line<'a>; CHANNEL_MEMBER_ROWS] {
    let member = &members[index];
    let selected = if app.focus() == PanelFocus::Sandbox {
        app.selected_channel_member() == index
    } else {
        member.id == app.active_agent_id()
    };
    let hovered = app.hover() == Some(HoverTarget::ChannelMemberRow(index));
    let state = CellState::of(selected, hovered);
    let name = if member.depth == 0 {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let cell = cell_style(theme, state, Style::default().fg(name));
    let end = AgentRowEnd::definition(width, (member_code(app, member), code_width));
    let guides = spawn_guides(members, index);
    let cloud = app.cloud_badge(&member.id);
    let name_row = tree_row(
        width,
        (&guides, theme.colors.muted),
        member_icon(app, member, theme),
        (member_name(member), member_badge(theme, cloud.as_ref())),
        cell,
        &end,
        &[],
    );
    let status = cell_style(theme, state, Style::default().fg(theme.colors.muted));
    let status_row = status_row(
        width,
        &status_guides(&guides),
        &member_status(app, member),
        (status.bg, status.text),
        theme,
    );
    [name_row, status_row]
}

/// A member's icon column: the spinner while it works, else the `@` of a
/// member, or nothing for a subagent, whose guides run on through it.
fn member_icon(app: &App, member: &AgentSummary, theme: &Theme) -> StatusIcon {
    if app.agent_thinking(member) {
        working_icon(app, theme)
    } else if member.depth == 0 {
        StatusIcon::new("@", theme.colors.muted)
    } else {
        StatusIcon::new("", theme.colors.muted)
    }
}

/// The cloud after a member's name when it runs in the cloud, from
/// [`App::cloud_badge`]: gray as on PROJECTS, dimmer with the owner's
/// name for another user's agent.
fn member_badge<'b>(theme: &Theme, cloud: Option<&'b (String, bool)>) -> Option<RowBadge<'b>> {
    let colors = &theme.colors;
    cloud.map(|(text, foreign)| RowBadge {
        text,
        color: if *foreign { colors.faint } else { colors.muted },
    })
}

/// What a member's first row calls it: the permanent agent it is an
/// instance of, `subagent` for a subagent of none, else its name.
fn member_name(member: &AgentSummary) -> &str {
    match member.definition.as_deref() {
        Some(definition) => definition,
        None if member.depth > 0 => "subagent",
        None => &member.name,
    }
}

/// What a member's second row says: its title, else the subtask it was
/// spawned for, else whether it is working.
fn member_status(app: &App, member: &AgentSummary) -> String {
    member
        .title
        .as_deref()
        .or(member.subtitle.as_deref())
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map_or_else(
            || {
                let status = if app.agent_thinking(member) {
                    WORKING
                } else {
                    IDLE
                };
                status.to_string()
            },
            str::to_string,
        )
}

/// The guides of a member's second row, from `guides`, its first row's:
/// each level's line runs on down while more agents follow at it, so an
/// elbow onto the member runs on as a line, or ends.
fn status_guides(guides: &str) -> String {
    guides.replace('├', "│").replace('└', " ")
}

/// A member's second row: `guides`, then `status` where the name above
/// starts, cut to fit, on the row's `bg` in its `text` style.
fn status_row<'a>(
    width: usize,
    guides: &str,
    status: &str,
    (bg, text): (Option<Color>, Style),
    theme: &Theme,
) -> Line<'a> {
    let fill = bg.map_or_else(Style::default, |bg| Style::default().bg(bg));
    let lead = format!("{:ROW_INSET$}{guides}{}", "", " ".repeat(AURA_WIDTH + 1));
    let lead_width = display_width(&lead);
    // The row keeps its blanks at the right end, as the name's row does.
    let status = truncate_record_info(status, width.saturating_sub(lead_width + ROW_INSET));
    let padding = width.saturating_sub(lead_width + display_width(&status));
    Line::from(vec![
        Span::styled(lead, fill.fg(theme.colors.muted)),
        Span::styled(status, fill.patch(text)),
        Span::styled(" ".repeat(padding), fill),
    ])
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
    placement_section(&mut info, channel);
    info.into_lines()
}

/// The AGENTS RUN section: where the members the channel starts run. A
/// cloud channel without a project it can use (none, or one the app no
/// longer lists, which comes without a folder) starts them locally.
fn placement_section(info: &mut Info<'_>, channel: &ChannelSummary) {
    info.section("AGENTS RUN", None);
    info.item(placement_name(channel.placement), None);
    let usable_project = channel
        .project
        .as_ref()
        .is_some_and(|project| !project.folder.is_empty());
    if channel.placement == PlacementView::Cloud && !usable_project {
        info.text(CLOUD_WITHOUT_PROJECT, Value::Quiet);
    }
}

/// What INFO says of a cloud channel without a project it can use.
const CLOUD_WITHOUT_PROJECT: &str = "Without a project new members run locally";

/// What INFO says of a channel without a project.
const NO_PROJECT: &str = "No project — agents work in the channel's folder";
