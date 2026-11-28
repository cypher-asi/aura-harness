//! The agents panel's AGENTS tab: the roster of permanent agents, with
//! "+ New agent" pinned to the bottom.

use super::{draw_list, split_pinned};
use crate::components::{cell_style, list_row, list_row_ending, CellState, StatusIcon};
use crate::renderer::chrome::ScrollView;
use crate::renderer::text::display_width;
use crate::{
    app::{AgentRowEnd, HoverTarget, PanelFocus},
    App, Theme,
};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

/// Draw the AGENTS tab into `body`, the inside of the panel bordered by
/// `area`, with "+ New agent" pinned to its bottom row.
pub(super) fn render_roster(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let (rows, pinned) = split_pinned(app, area, body);
    let visible = usize::from(rows.height);
    app.set_definition_rows_area(rows);
    app.set_new_agent_area(pinned);
    app.sync_definitions_scroll(visible);
    let mut lines = build_definition_lines(app, body, theme);
    let new_agent = lines.pop().unwrap_or_default();
    let view = ScrollView {
        total: lines.len(),
        visible,
        first: app.definitions_scroll(),
    };
    let lines = lines.into_iter().skip(view.first).take(visible).collect();
    let focused = app.focus() == PanelFocus::Swarm;
    draw_list(
        frame,
        (area, rows),
        lines,
        Some((pinned, new_agent)),
        view,
        theme,
        focused,
    );
}

/// Roster rows: a gray `@`, the name and, at the far right, the muted
/// abbreviation for each permanent agent, then "+  New agent", which the
/// panel pins to its bottom row and which has no abbreviation. Markers and
/// names share columns with the projects list, and abbreviations are laid
/// out as its codes are ([`AgentRowEnd::definition`]): left-aligned in a
/// column as wide as the widest one, one space from the edge, in the time
/// color on the row's fill. A long name is cut first; a row that would
/// leave the name fewer than four columns drops its abbreviation. A
/// selected row still paints the `@` in the selection text color.
pub(in crate::renderer) fn build_definition_lines<'a>(
    app: &App,
    inner: Rect,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_definition();
    let width = inner.width as usize;
    let definitions = app.definitions();
    let abbreviation_width = definitions
        .iter()
        .filter_map(|definition| definition.abbreviation.as_deref())
        .map(display_width)
        .max()
        .unwrap_or(0);
    let names = definitions
        .iter()
        .map(|definition| {
            (
                "@",
                definition.name.as_str(),
                theme.colors.secondary,
                theme.colors.muted,
                Some(definition.abbreviation.as_deref()),
            )
        })
        .chain(std::iter::once((
            "+",
            "New agent",
            theme.colors.muted,
            theme.colors.muted,
            None,
        )));

    names
        .enumerate()
        .map(|(i, (marker, name, text, icon_color, abbreviation))| {
            let is_selected = i == selected && is_focused;
            let is_hovered =
                matches!(app.hover(), Some(HoverTarget::DefinitionRow(row)) if row == i);
            let state = CellState::of(is_selected, is_hovered);
            let cell = cell_style(theme, state, Style::default().fg(text));
            let icon = StatusIcon::new(marker, icon_color);
            match abbreviation {
                Some(abbreviation) => list_row_ending(
                    width,
                    0,
                    icon,
                    (name, None),
                    cell,
                    &AgentRowEnd::definition(width, (abbreviation, abbreviation_width)),
                ),
                None => list_row(width, 0, icon, name, cell, None),
            }
        })
        .collect()
}
