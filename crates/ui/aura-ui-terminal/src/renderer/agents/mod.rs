//! The agents panel: a CHANNELS | PROJECTS | AGENTS | ARCHIVE header bar
//! ending in BENCH's wrench, over the list of the app in front, one module
//! per app.
//!
//! Every app lays its rows out alike, so names line up from one app to
//! the next (drawn here without the row's inset):
//!
//! ```text
//! TODAY
//! ▾  name  detail                   5m
//!    ├── name                       2m
//!    └── name                       2d
//! +  New …
//! ```
//!
//! - The row's inset, the icon column (as wide as the aura ring), a space,
//!   then the name.
//! - The icon column holds one thing: the spinner while the row, or a row
//!   folded under it, works; else `▸` over the rows it folds, `▾` once
//!   they show; else the app's mark (`#`, `@`, a folder, `+`); else
//!   nothing.
//! - A muted detail may follow the name, and is cut first. The right end
//!   is muted too: how long ago, a code, or a project's `+`.
//! - Rows under a row hang from its name on tree lines.
//! - A header grouping rows, a project or a day, starts in the icon
//!   column and is written in muted capitals.
//! - The row that makes something new is pinned to the bottom.

mod archive;
mod bench;
pub(super) mod channels;
mod drop_marks;
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

/// Render the agents panel: a CHANNELS | PROJECTS | AGENTS | ARCHIVE
/// header bar ending in BENCH's wrench, over the channels, the live
/// agents, the roster of permanent agents, the benchmarks or the archived
/// agents.
pub(super) fn render_agents_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let panel = area;
    let (bar, area) = header::split(area, app.host_chrome());
    let header = agents_header(app, bar);
    header::show(frame, app, (Pane::Agents, panel, bar), &header, theme);

    let chrome = (header.focused, header.hosted);
    if let Some(body) = render_body_block(frame, area, chrome, theme) {
        match app.agents_tab() {
            AgentsTab::Channels => channels::render_channels(frame, area, body, app, theme),
            AgentsTab::Projects => projects::render_projects(frame, area, body, app, theme),
            AgentsTab::Roster => roster::render_roster(frame, area, body, app, theme),
            AgentsTab::Bench => bench::render_bench(frame, area, body, app, theme),
            AgentsTab::Archive => archive::render_archive(frame, area, body, app, theme),
        }
    }
    header::render_icon_tooltip(frame, bar, &header, theme);
}

/// The agents panel's header in `bar`: the labelled tabs, then BENCH's
/// wrench in a cell of its own at the right end. Where they fell is
/// recorded for the pointer.
fn agents_header(app: &mut App, bar: Rect) -> PanelHeader<'static, AgentsTab> {
    let hosted = app.host_chrome();
    let tab = AgentsTab::ICON_TAB;
    let icon = tab.icon();
    let end = header::bar_end(bar, (icon, false, false), AgentsTab::layout);
    if !hosted {
        app.set_agent_tabs_area(Rect {
            width: end.tabs,
            ..bar
        });
        app.set_agent_icon_tab_area(end.icon);
    }
    let hovered = match app.hover() {
        Some(HoverTarget::AgentTab(tab)) => Some(tab),
        _ => None,
    };
    PanelHeader {
        layout: AgentsTab::layout(end.tabs),
        active: Some(app.agents_tab()),
        hovered,
        title: None,
        info: None,
        close: None,
        expand: None,
        scope: None,
        icon_tab: icon.map(|icon| header::IconTab {
            tab,
            icon,
            tooltip: tab.tooltip(),
            cell: end.icon,
        }),
        focused: app.focus() == PanelFocus::Swarm,
        hosted,
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

/// The icon of an agent whose work failed, in the Stop button's red.
pub(in crate::renderer) const FAILED_ICON: &str = "!";

/// The braille spinner the chat shows beside work in flight, for a
/// working agent or folded project in the projects list.
pub(super) fn working_icon(app: &App, theme: &Theme) -> StatusIcon {
    StatusIcon::new(app.spinner_char(), theme.colors.muted)
}
