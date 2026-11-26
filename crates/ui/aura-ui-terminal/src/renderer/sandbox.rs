//! The panel below the Record panel: a SANDBOX | PROCESSES | TRACE | TASKS
//! header bar over the file tree of the active agent's workspace, the
//! Processes list, the trace feed, or its tasks grouped under their plans.
//! While a channel is open the bar lists only the channel's MEMBERS and
//! INFO (see `channel_side`), and the panel fills the right column.
//! Outside a channel the bar ends in a `*` button that lists the whole
//! project's processes, steps, plans and tasks instead of the active
//! agent's, then a ⤢ button that maximizes the panel over the Record
//! panel, or restores it.

use super::chrome::{render_body_block, Body, ScrollView};
use super::header::{self, PanelHeader};
use super::side_list::{name_style, render_empty, Tone, SANDBOX_HINT};
use crate::app::sandbox::{SandboxEntryKind, SandboxRow};
use crate::app::{HoverTarget, Pane, PanelFocus, SidePanelTab, SideScope};
use crate::components::{cell_style, list_row, CellState, StatusIcon};
use crate::{App, Theme};
use ratatui::{layout::Rect, text::Line, Frame};

/// Render the panel: its header bar, then the open tab.
pub(super) fn render_sandbox_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let hosted = app.host_chrome();
    let panel = area;
    let (bar, area) = header::split(area, hosted);
    // An open channel's panel fills the column until it closes, and lists
    // only the channel's own tabs, so the expand and scope buttons, agent
    // features, are left out.
    let channel_open = app.channel_open();
    let layout = |width| SidePanelTab::layout(width, channel_open);
    let end = header::bar_end(bar, (!channel_open, !channel_open), layout);
    if !hosted {
        app.set_side_tabs_area(Rect {
            width: end.tabs,
            ..bar
        });
        app.set_side_scope_area(end.scope);
        app.set_side_expand_area(end.expand);
    }
    let hovered = match app.hover() {
        Some(HoverTarget::SideTab(tab)) => Some(tab),
        _ => None,
    };
    let header = PanelHeader {
        layout: layout(end.tabs),
        active: Some(app.side_tab()),
        hovered,
        title: None,
        info: None,
        close: None,
        expand: (!channel_open).then(|| header::ExpandButton {
            expanded: app.side_expanded(),
            hovered: app.hover() == Some(HoverTarget::ExpandSide),
            cell: end.expand,
        }),
        scope: (!channel_open).then(|| header::ScopeButton {
            on: app.side_scope() == SideScope::Project,
            hovered: app.hover() == Some(HoverTarget::SideScope),
            cell: end.scope,
        }),
        focused: is_focused,
        hosted,
    };
    header::show(frame, app, (Pane::Side, panel, bar), &header, theme);

    let Some(body) = render_body_block(frame, area, (is_focused, hosted), theme) else {
        return;
    };

    match app.side_tab() {
        SidePanelTab::Sandbox => render_tree(frame, area, body, app, theme),
        SidePanelTab::Processes => {
            super::processes::render_processes(frame, area, body, app, theme);
        }
        SidePanelTab::Host => super::host_tab::render_host(frame, area, body, app, theme),
        SidePanelTab::Trace => {
            super::trace_feed::render_trace_feed(frame, area, body, app, theme);
        }
        SidePanelTab::Tasks => super::tasks::render_tasks(frame, area, body, app, theme),
        SidePanelTab::ChannelMembers => {
            super::channel_side::render_channel_members(frame, area, body, app, theme);
        }
        SidePanelTab::ChannelInfo => {
            super::channel_side::render_channel_info(frame, area, body, app, theme);
        }
    }
}

/// Draw the sandbox tree into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
fn render_tree(frame: &mut Frame, area: Rect, inner: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    if app.sandbox().rows().is_empty() {
        let message = if app.sandbox().root().is_some() {
            "Sandbox is empty."
        } else {
            "No sandbox yet."
        };
        render_empty(frame, inner, message, theme);
        return;
    }

    let body = Body::new(area, inner, is_focused);
    let visible = body.visible();
    let scroll = app.sync_sandbox_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::SandboxRow(index)) => Some(index),
        _ => None,
    };
    let tree = app.sandbox();
    let selected = tree.selected();
    let width = body.width();

    let lines: Vec<Line> = tree
        .rows()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, row)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            sandbox_line(row, state, width, theme)
        })
        .collect();
    let view = ScrollView {
        total: tree.rows().len(),
        visible,
        first: scroll,
    };
    app.set_sandbox_rows_area(body.rows(), scroll);
    body.draw(frame, lines, view, SANDBOX_HINT, theme);
}

/// A tree row: the fold glyph of a directory (none for files), then the
/// name, `foreground` unless it is a dot entry or a "more" placeholder.
fn sandbox_line<'a>(row: &SandboxRow, state: CellState, width: usize, theme: &Theme) -> Line<'a> {
    let muted = theme.colors.muted;
    let (glyph, name) = match row.kind {
        SandboxEntryKind::Dir => (if row.expanded { "▾" } else { "▸" }, row.name.clone()),
        SandboxEntryKind::File => (" ", row.name.clone()),
        SandboxEntryKind::Symlink => (" ", format!("{}@", row.name)),
        SandboxEntryKind::More(_) => (" ", row.name.clone()),
    };
    let tone = Tone::active_if(
        !row.name.starts_with('.') && !matches!(row.kind, SandboxEntryKind::More(_)),
    );
    list_row(
        width,
        row.depth,
        StatusIcon::new(glyph, muted),
        &name,
        cell_style(theme, state, name_style(tone, theme)),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use std::path::PathBuf;

    fn row(name: &str, kind: SandboxEntryKind) -> SandboxRow {
        SandboxRow {
            path: PathBuf::from(name),
            name: name.to_string(),
            depth: 0,
            kind,
            expanded: false,
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn name_fg(row: &SandboxRow, state: CellState, theme: &Theme) -> Option<Color> {
        let line = sandbox_line(row, state, 40, theme);
        line.spans
            .iter()
            .find(|span| span.content.as_ref() == row.name)
            .and_then(|span| span.style.fg)
    }

    #[test]
    fn dot_entries_are_muted_and_the_rest_foreground() {
        let theme = Theme::cyber();
        let normal = CellState::Normal;
        let fg = Some(theme.colors.foreground);
        let muted = Some(theme.colors.muted);
        assert_eq!(
            name_fg(&row("src", SandboxEntryKind::Dir), normal, &theme),
            fg
        );
        assert_eq!(
            name_fg(&row("main.rs", SandboxEntryKind::File), normal, &theme),
            fg
        );
        assert_eq!(
            name_fg(&row(".git", SandboxEntryKind::Dir), normal, &theme),
            muted
        );
        assert_eq!(
            name_fg(&row(".env", SandboxEntryKind::File), normal, &theme),
            muted
        );
    }

    #[test]
    fn rows_use_the_shared_selection_and_hover_cells() {
        let theme = Theme::cyber();
        let file = row("main.rs", SandboxEntryKind::File);
        let selected = sandbox_line(&file, CellState::Selected, 40, &theme);
        assert!(selected
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.colors.selection)));
        assert_eq!(
            name_fg(&file, CellState::Selected, &theme),
            Some(theme.colors.on_selection)
        );
        let hovered = sandbox_line(&file, CellState::Hovered, 40, &theme);
        assert!(hovered
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.colors.surface_hover)));
    }

    #[test]
    fn directories_show_their_fold_state() {
        let theme = Theme::cyber();
        let mut dir = row("src", SandboxEntryKind::Dir);
        assert!(text(&sandbox_line(&dir, CellState::Normal, 40, &theme)).starts_with(" ▸"));
        dir.expanded = true;
        assert!(text(&sandbox_line(&dir, CellState::Normal, 40, &theme)).starts_with(" ▾"));
    }
}
