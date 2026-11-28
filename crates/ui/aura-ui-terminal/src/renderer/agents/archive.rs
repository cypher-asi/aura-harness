//! The agents panel's ARCHIVE tab: archived agents under the day they
//! were archived on, the agents under one hanging from its name as the
//! projects list hangs spawns.
//!
//! ```text
//! YESTERDAY
//! ▾  Agent                    2d
//!    └── reviewer #1          2d
//!    Agent 2                  3d
//! ```

use super::draw_list;
use super::projects::spawn_guides;
use crate::components::{
    cell_style, format_ago, tree_row, truncate_record_info, CellState, StatusIcon, ROW_INSET,
};
use crate::renderer::chrome::ScrollView;
use crate::renderer::side_list::empty_line;
use crate::{
    app::{AgentRowEnd, ArchiveRow, HoverTarget, MenuTarget, PanelFocus},
    App, Theme,
};
use aura_core_types::time::now_ms;
use ratatui::Frame;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use std::ops::Range;

/// Draw the ARCHIVE tab into `body`, the inside of the panel bordered by
/// `area`.
pub(super) fn render_archive(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let body = app.list_rows(area, body);
    let visible = usize::from(body.height);
    app.set_archived_rows_area(body);
    app.sync_archived_scroll(visible);
    let first = app.archived_scroll();
    let (lines, total) = build_archived_lines(app, body, first..first + visible, theme);
    let view = ScrollView {
        total,
        visible,
        first,
    };
    let focused = app.focus() == PanelFocus::Swarm;
    draw_list(frame, (area, body), lines, None, view, theme, focused);
}

/// Archive rows: a muted header for each day agents were archived on, then
/// each archived agent, with the agents under it on tree lines below while
/// it is open, each showing how long ago it was last active on the right.
/// An agent with agents under it shows ▸ in the icon column while folded
/// over them, as it starts, and ▾ once opened. Empty, a single muted line.
/// Only the rows in `window` are drawn; the total comes with them.
/// Unfocused, the agent on screen keeps the highlight, or the folded agent
/// over it.
fn build_archived_lines<'a>(
    app: &App,
    inner: Rect,
    window: Range<usize>,
    theme: &Theme,
) -> (Vec<Line<'a>>, usize) {
    let archived = app.archived();
    if archived.is_empty() {
        let line = empty_line("No archived agents.", inner.width, theme);
        return if window.start == 0 && !window.is_empty() {
            (vec![line], 1)
        } else {
            (Vec::new(), 1)
        };
    }
    let archive = Archive {
        app,
        theme,
        inner,
        is_focused: app.focus() == PanelFocus::Swarm,
        shown_active: app.shown_active_archived(),
        now_ms: now_ms(),
        menu_target: app.context_menu().map(|menu| &menu.target),
    };
    let rows = app.archive_rows();
    let total = rows.len();
    let shown = rows
        .into_iter()
        .skip(window.start)
        .take(window.len())
        .filter_map(|row| match row {
            ArchiveRow::Day(label) => Some(archive.day_line(&label)),
            ArchiveRow::Agent { index } => archive.agent_line(index),
        })
        .collect();
    (shown, total)
}

/// What every row of the ARCHIVE tab is drawn with, worked out once a
/// frame.
struct Archive<'s> {
    app: &'s App,
    theme: &'s Theme,
    inner: Rect,
    is_focused: bool,
    shown_active: Option<usize>,
    now_ms: u64,
    menu_target: Option<&'s MenuTarget>,
}

impl Archive<'_> {
    /// The header of the day `label`, from the icon column on, in muted
    /// capitals like a project's.
    fn day_line<'a>(&self, label: &str) -> Line<'a> {
        let room = usize::from(self.inner.width).saturating_sub(2 * ROW_INSET);
        let label = truncate_record_info(&label.to_uppercase(), room);
        Line::from(Span::styled(
            format!("{:ROW_INSET$}{label}", ""),
            Style::default().fg(self.theme.colors.muted),
        ))
    }

    /// The row of archived agent `index`, `None` when there is none.
    fn agent_line<'a>(&self, index: usize) -> Option<Line<'a>> {
        let (app, theme) = (self.app, self.theme);
        let agent = app.archived().get(index)?;
        let selected = if self.is_focused {
            index == app.selected_archived()
        } else {
            self.shown_active == Some(index)
        };
        let hovered = matches!(app.hover(), Some(HoverTarget::ArchivedRow(i)) if i == index)
            || matches!(self.menu_target, Some(MenuTarget::Archived(id)) if *id == agent.id);
        let ago = agent.last_active_ms.map(|ms| format_ago(ms, self.now_ms));
        // ▸ over folded agents under it, ▾ once opened.
        let marker = match (app.archived_has_spawns(index), app.archived_folded(index)) {
            (false, _) => "",
            (true, true) => "▸",
            (true, false) => "▾",
        };
        let name = if agent.depth == 0 {
            theme.colors.foreground
        } else {
            theme.colors.muted
        };
        Some(tree_row(
            usize::from(self.inner.width),
            (&spawn_guides(app.archived(), index), theme.colors.muted),
            StatusIcon::new(marker, theme.colors.muted),
            (&agent.name, None),
            cell_style(
                theme,
                CellState::of(selected, hovered),
                Style::default().fg(name),
            ),
            &AgentRowEnd::time(ago.as_deref()),
            &[],
        ))
    }
}
