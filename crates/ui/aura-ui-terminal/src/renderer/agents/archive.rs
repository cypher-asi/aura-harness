//! The agents panel's ARCHIVE tab: archived agents under the day they
//! were archived on.

use super::draw_list;
use crate::components::{cell_style, format_ago, list_row, now_ms, CellState, StatusIcon};
use crate::renderer::chrome::ScrollView;
use crate::renderer::side_list::empty_line;
use crate::{
    app::{ArchiveRow, HoverTarget, MenuTarget, PanelFocus},
    App, Theme,
};
use ratatui::Frame;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
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
/// each archived agent, with the agents under it indented below while it
/// is open, each showing how long ago it was last active on the right.
/// An agent with
/// agents under it shows ▸ in the icon column while folded over them,
/// as it starts, and ▾ once opened. Empty, a single muted line. Only
/// the rows in `window` are drawn; the total comes with them. Unfocused,
/// the agent on screen keeps the highlight, or the folded agent over it.
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
    let is_focused = app.focus() == PanelFocus::Swarm;
    let shown_active = app.shown_active_archived();
    let width = inner.width as usize;
    let now_ms = now_ms();
    let menu_target = app.context_menu().map(|menu| &menu.target);
    let rows = app.archive_rows();
    let total = rows.len();
    let shown = rows
        .into_iter()
        .skip(window.start)
        .take(window.len())
        .filter_map(|row| {
            let index = match row {
                ArchiveRow::Day(label) => {
                    return Some(
                        empty_line(&label, inner.width, theme)
                            .patch_style(Style::default().add_modifier(Modifier::BOLD)),
                    );
                }
                ArchiveRow::Agent { index } => index,
            };
            let agent = archived.get(index)?;
            let selected = if is_focused {
                index == app.selected_archived()
            } else {
                shown_active == Some(index)
            };
            let hovered = matches!(app.hover(), Some(HoverTarget::ArchivedRow(i)) if i == index)
                || matches!(menu_target, Some(MenuTarget::Archived(id)) if *id == agent.id);
            // Top-level rows leave the right side blank; agents under them
            // show how long ago they were last active.
            let side = if agent.depth == 0 {
                None
            } else {
                agent.last_active_ms.map(|ms| format_ago(ms, now_ms))
            };
            // ▸ over folded agents under it, ▾ once opened.
            let marker = match (app.archived_has_spawns(index), app.archived_folded(index)) {
                (false, _) => "",
                (true, true) => "▸",
                (true, false) => "▾",
            };
            Some(list_row(
                width,
                agent.depth,
                StatusIcon::new(marker, theme.colors.muted),
                &agent.name,
                cell_style(
                    theme,
                    CellState::of(selected, hovered),
                    Style::default().fg(if agent.depth == 0 {
                        theme.colors.foreground
                    } else {
                        theme.colors.muted
                    }),
                ),
                side.as_deref(),
            ))
        })
        .collect();
    (shown, total)
}
