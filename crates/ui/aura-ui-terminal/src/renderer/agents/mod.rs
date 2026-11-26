//! The agents panel: a CHANNELS | PROJECTS | AGENTS | BENCH | ARCHIVE
//! header bar over the list of the app in front, one module per app.

mod archive;
mod bench;
pub(super) mod channels;
pub(super) mod projects;
pub(super) mod roster;

use super::chrome::{render_body_block, render_scrollbar, ScrollView};
use super::header::{self, PanelHeader};
use crate::components::StatusIcon;
use crate::{
    app::{AgentsTab, HoverTarget, Pane, PanelFocus},
    App, Theme,
};
use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

/// Render the agents panel: a CHANNELS | PROJECTS | AGENTS | BENCH |
/// ARCHIVE header bar over the channels, the live agents, the roster of
/// permanent agents, the benchmarks or the archived agents.
pub(super) fn render_agents_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let hosted = app.host_chrome();
    let panel = area;
    let (bar, area) = header::split(area, hosted);
    if !hosted {
        app.set_agent_tabs_area(bar);
    }
    let hovered = match app.hover() {
        Some(HoverTarget::AgentTab(tab)) => Some(tab),
        _ => None,
    };
    let header = PanelHeader {
        layout: AgentsTab::layout(bar.width),
        active: Some(app.agents_tab()),
        hovered,
        title: None,
        info: None,
        close: None,
        expand: None,
        scope: None,
        focused: is_focused,
        hosted,
    };
    header::show(frame, app, (Pane::Agents, panel, bar), &header, theme);

    let Some(body) = render_body_block(frame, area, (is_focused, hosted), theme) else {
        return;
    };
    match app.agents_tab() {
        AgentsTab::Channels => channels::render_channels(frame, area, body, app, theme),
        AgentsTab::Projects => projects::render_projects(frame, area, body, app, theme),
        AgentsTab::Roster => roster::render_roster(frame, area, body, app, theme),
        AgentsTab::Bench => bench::render_bench(frame, area, body, app, theme),
        AgentsTab::Archive => archive::render_archive(frame, area, body, app, theme),
    }
}

/// `body` split into a list's rows and the row pinned under them, which
/// holds the list's "+ New …".
fn split_pinned(body: Rect) -> (Rect, Rect) {
    let rows = Rect {
        height: body.height.saturating_sub(1),
        ..body
    };
    let pinned = Rect {
        y: body.y + rows.height,
        height: body.height.min(1),
        ..body
    };
    (rows, pinned)
}

/// Draw a list's visible `lines` into `rows`, the pinned line on its row,
/// and the list's scrollbar on the border of `area`, the panel around them.
fn draw_list(
    frame: &mut Frame,
    (area, rows): (Rect, Rect),
    lines: Vec<Line<'_>>,
    pinned: Option<(Rect, Line<'_>)>,
    view: ScrollView,
    theme: &Theme,
    focused: bool,
) {
    frame.render_widget(Paragraph::new(lines), rows);
    if let Some((row, line)) = pinned {
        frame.render_widget(Paragraph::new(line), row);
    }
    render_scrollbar(frame, area, rows, view, focused, theme);
}

pub(super) fn aura_icon(app: &App) -> StatusIcon {
    let (glyph, color) = app.aura_frame();
    StatusIcon::new(&glyph, color)
}

/// The braille spinner the chat shows beside work in flight, for a
/// working agent or folded project in the projects list.
pub(super) fn working_icon(app: &App, theme: &Theme) -> StatusIcon {
    StatusIcon::new(app.spinner_char(), theme.colors.muted)
}
