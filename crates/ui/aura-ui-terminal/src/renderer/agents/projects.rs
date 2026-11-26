//! The agents panel's PROJECTS tab: projects, each over its agents and
//! the agents they spawned, with "+ New project" pinned to the bottom.

use super::{draw_list, split_pinned, working_icon};
use crate::components::{
    cell_style, format_ago, list_row, now_ms, tree_row, CellState, RowBadge, StatusIcon, PLAN_BADGE,
};
use crate::renderer::chrome::ScrollView;
use crate::{
    animation::AURA_WIDTH,
    app::{AgentButton, AgentRowEnd, HoverTarget, MenuTarget, PanelFocus, ProjectListRow},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};
use std::ops::Range;

/// Draw the PROJECTS tab into `body`, the inside of the panel bordered
/// by `area`, with "+ New project" pinned to its bottom row.
pub(super) fn render_projects(
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let (rows, pinned) = split_pinned(body);
    let visible = usize::from(rows.height);
    app.set_agent_rows_area(rows);
    app.set_new_project_area(pinned);
    app.sync_agents_scroll(visible);
    let first = app.agents_scroll();
    let (lines, new_project, total) = build_swarm_lines(app, body, first..first + visible, theme);
    let view = ScrollView {
        total,
        visible,
        first,
    };
    let focused = app.focus() == PanelFocus::Swarm;
    draw_list(
        frame,
        (area, rows),
        lines,
        Some((pinned, new_project)),
        view,
        theme,
        focused,
    );
}

/// A project is its own row under a folder icon, with a "+" on the right
/// that adds an agent to it, and its agents below it, each followed by
/// the agents it spawned indented further. Each agent shows how long ago
/// it was last active on the right, then its code, padded so the codes
/// line up; a row too narrow to leave its name four columns beside the
/// code drops it. A ring spins in the
/// folder's column beside agents that are thinking; idle agents leave it
/// blank. An agent with spawns shows ▾ there, ▸ when folded over them;
/// a busy one shows the ring alone, which still folds it.
/// Space folds an agent over its spawns, or the project of a leaf, which
/// hides every agent under it. Every row is one
/// line, and only the highlighted one has a background: the keyboard cursor
/// while the panel is focused, otherwise the agent shown in the chat, or
/// the folded agent it is hidden under.
/// Projects are never highlighted. The
/// last line is "+ New project", which the panel pins to its bottom row:
///
/// ```text
///  🗁  WEBSITE                 +
///  ⢎⡱ Agent            1m AGE1
///      reviewer #1     2d REV1
///     Agent 2          5m AGE2
///
///  🗀  RESEARCH                +
///  …
///  +  New project
/// ```
///
/// Returns the rows in `window` of the ones that scroll, the pinned last
/// row, and how many rows scroll. Only those rows are formatted, so a
/// long list costs what fits on screen.
fn build_swarm_lines<'a>(
    app: &App,
    inner: Rect,
    window: Range<usize>,
    theme: &Theme,
) -> (Vec<Line<'a>>, Line<'a>, usize) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let selected = app.selected_agent();
    // Unfocused, the agent in the chat is lit, or the folded agent over it.
    let shown_active = app.shown_active_agent();
    let width = inner.width as usize;
    let now_ms = now_ms();
    let is_thinking = |agent: &crate::events::AgentSummary| app.agent_thinking(agent);
    let project_name = Style::default().fg(theme.colors.muted);
    let agent_name = Style::default().fg(theme.colors.foreground);
    let subagent_name = Style::default().fg(theme.colors.muted);
    let menu_target = app.context_menu().map(|menu| &menu.target);
    // While a project or agent is dragged its rows stay lit, and a line
    // in the accent color between rows marks where they go.
    let drag = app.project_drag_hit();
    let rows = app.project_rows();
    let drop_line = drag
        .as_ref()
        .and_then(|drag| drag.edge)
        .and_then(|edge| drop_line_at(&rows, edge));
    let listed = rows.len().saturating_sub(1);
    // A project's agents run up to the next project's root.
    let roots: Vec<usize> = rows
        .iter()
        .filter_map(|row| match row {
            ProjectListRow::Project { root, .. } => Some(*root),
            _ => None,
        })
        .collect();
    let mut projects_seen = 0;
    let mut lines = Vec::with_capacity(window.len());
    let mut new_project = Line::default();

    for (row_index, row) in rows.into_iter().enumerate() {
        if matches!(row, ProjectListRow::Project { .. }) {
            projects_seen += 1;
        }
        let pinned = matches!(row, ProjectListRow::NewProject);
        if !pinned && (row_index >= listed || !window.contains(&row_index)) {
            continue;
        }
        let is_selected = match row {
            ProjectListRow::Project { .. }
            | ProjectListRow::EmptyProject { .. }
            | ProjectListRow::Gap => false,
            _ if is_focused => row_index == selected,
            ProjectListRow::Agent { index } => shown_active == Some(index),
            ProjectListRow::NewProject => false,
        };
        let add_hovered =
            matches!(app.hover(), Some(HoverTarget::AddToProject(index)) if index == row_index);
        let button_hovered = |button: AgentButton| app.hover() == Some(button.target(row_index));
        let pointer_on_row = AgentButton::ALL.into_iter().any(button_hovered)
            || matches!(app.hover(), Some(HoverTarget::ProjectRow(index)) if index == row_index);
        let menu_on = match (row, menu_target) {
            (ProjectListRow::Project { root, .. }, Some(MenuTarget::Project(key))) => {
                app.agents()[root].project_key() == key
            }
            (ProjectListRow::EmptyProject { project }, Some(MenuTarget::Project(key))) => {
                app.projects().get(project).is_some_and(|p| p.id == *key)
            }
            (ProjectListRow::Agent { index }, Some(MenuTarget::Agent(id))) => {
                app.agents()[index].id == *id
            }
            _ => false,
        };
        let is_hovered = add_hovered
            || pointer_on_row
            || menu_on
            || drag
                .as_ref()
                .is_some_and(|drag| drag.dragged.contains(&row_index));
        match row {
            ProjectListRow::Project { root, expanded } => {
                let end = roots
                    .get(projects_seen)
                    .copied()
                    .unwrap_or(app.agents().len());
                let agents = &app.agents()[root..end];
                let icon = if !expanded && agents.iter().any(is_thinking) {
                    working_icon(app, theme)
                } else {
                    let folder = if expanded { "🗁" } else { "🗀" };
                    StatusIcon::new(folder, theme.colors.muted)
                };
                let mut cell =
                    cell_style(theme, CellState::of(is_selected, is_hovered), project_name);
                if add_hovered {
                    cell.time = theme.colors.foreground;
                }
                lines.push(list_row(
                    width,
                    0,
                    icon,
                    &project_label(&app.agents()[root]),
                    cell,
                    Some("+"),
                ));
            }
            ProjectListRow::EmptyProject { project } => {
                let name = app
                    .projects()
                    .get(project)
                    .map(|p| p.name.to_uppercase())
                    .unwrap_or_default();
                let mut cell =
                    cell_style(theme, CellState::of(is_selected, is_hovered), project_name);
                if add_hovered {
                    cell.time = theme.colors.foreground;
                }
                let icon = StatusIcon::new("🗀", theme.colors.muted);
                lines.push(list_row(width, 0, icon, &name, cell, Some("+")));
            }
            ProjectListRow::Agent { index } => {
                let agent = &app.agents()[index];
                let marker = app.agent_has_spawns(index).then(|| {
                    if app.agent_folded(index) {
                        "▸"
                    } else {
                        "▾"
                    }
                });
                // A busy agent's spinner sits in the icon column and stands in
                // for its fold marker, open or folded; clicking it still
                // folds, as `fold_marker_at` says.
                let icon = if app.agent_row_busy(index) {
                    working_icon(app, theme)
                } else {
                    StatusIcon::new(marker.unwrap_or(""), theme.colors.muted)
                };
                let ago = agent.last_active_ms.map(|ms| format_ago(ms, now_ms));
                let state = CellState::of(is_selected, is_hovered);
                let name = if agent.depth == 0 {
                    agent_name
                } else {
                    subagent_name
                };
                let cell = cell_style(theme, state, name);
                // The buttons show only under the pointer, muted like the
                // time, and the one that is hovered lights up.
                let buttons = AgentButton::ALL.map(|button| {
                    let color = if button_hovered(button) && !is_selected {
                        theme.colors.foreground
                    } else {
                        cell.time
                    };
                    (button, color)
                });
                let buttons: &[_] = if pointer_on_row { &buttons } else { &[] };
                // Plan mode wins the badge; otherwise a cloud agent shows
                // a cloud, dimmed with its owner's name for another user's.
                let cloud = app.cloud_badge(&agent.id);
                let badge = if agent.plan_mode {
                    Some(RowBadge {
                        text: PLAN_BADGE,
                        color: theme.colors.success,
                    })
                } else {
                    cloud.as_ref().map(|(text, foreign)| RowBadge {
                        text,
                        color: if *foreign {
                            theme.colors.faint
                        } else {
                            theme.colors.primary
                        },
                    })
                };
                let end = AgentRowEnd::agent(ago.as_deref());
                lines.push(tree_row(
                    width,
                    (&spawn_guides(app.agents(), index), theme.colors.muted),
                    icon,
                    (&agent.name, badge),
                    cell,
                    &end,
                    buttons,
                ));
            }
            ProjectListRow::NewProject => {
                let cell = cell_style(
                    theme,
                    CellState::of(is_selected, is_hovered),
                    Style::default().fg(theme.colors.muted),
                );
                let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
                new_project = list_row(width, 0, icon, "New project", cell, None);
            }
            ProjectListRow::Gap => lines.push(Line::default()),
        }
        if !pinned {
            if let Some(line) = lines.last_mut() {
                match drop_line {
                    Some(DropLine::Rule(at)) if at == row_index => {
                        *line = drop_rule(width, theme.colors.success);
                    }
                    Some(DropLine::Under(at)) if at == row_index => {
                        underline(line, theme.colors.success);
                    }
                    _ => {}
                }
            }
        }
    }
    (lines, new_project, listed)
}

/// How the edge a drag's rows go to is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropLine {
    /// A rule across this gap row, between two projects
    Rule(usize),
    /// An underline along this row, the one just above the edge
    Under(usize),
}

/// How to draw the edge just above list row `edge`: across the gap row
/// beside it when there is one, else under the row above it. `None` above
/// the first row, where there is no row to draw on.
fn drop_line_at(rows: &[ProjectListRow], edge: usize) -> Option<DropLine> {
    let above = edge.checked_sub(1);
    if let Some(above) = above.filter(|&above| rows.get(above) == Some(&ProjectListRow::Gap)) {
        return Some(DropLine::Rule(above));
    }
    if rows.get(edge) == Some(&ProjectListRow::Gap) {
        return Some(DropLine::Rule(edge));
    }
    above.map(DropLine::Under)
}

/// A horizontal rule `width` columns wide, padded like the list's rows.
fn drop_rule(width: usize, color: Color) -> Line<'static> {
    let rule = DROP_RULE.repeat(width.saturating_sub(2));
    Line::from(vec![
        Span::raw(" "),
        Span::styled(rule, Style::default().fg(color)),
        Span::raw(" "),
    ])
}

/// Underline all of `line` in `color`, keeping its text colors.
fn underline(line: &mut Line<'_>, color: Color) {
    for span in &mut line.spans {
        span.style = span
            .style
            .add_modifier(Modifier::UNDERLINED)
            .underline_color(color);
    }
}

/// Glyph of the rule drawn where a dragged project or agent goes.
pub(in crate::renderer) const DROP_RULE: &str = "─";

/// Glyph of the rail beside a drag's hit area.
pub(in crate::renderer) const DROP_RAIL: &str = "▎";

/// `   │   ├`: the tree lines before the agent at `index` in `agents`.
/// Each level's line drops from the first column of its parent's name.
/// Each level the agent is under has a line down while the agent at that
/// level has more spawns below; the agent's own level ends in an elbow
/// onto it, `└` for the last of its parent's spawns. Agents at the top of
/// the tree have none.
pub(in crate::renderer) fn spawn_guides(
    agents: &[crate::events::AgentSummary],
    index: usize,
) -> String {
    spawn_guides_below(agents, index, 0)
}

/// [`spawn_guides`] for a tree drawn from an agent at depth `top`, which
/// has none: the agents under it get the first level's.
pub(in crate::renderer) fn spawn_guides_below(
    agents: &[crate::events::AgentSummary],
    index: usize,
    top: usize,
) -> String {
    let depth = agents[index].depth;
    let continues = |level: usize| {
        agents[index + 1..]
            .iter()
            .map(|agent| agent.depth)
            .take_while(|&depth| depth >= level)
            .any(|depth| depth == level)
    };
    let lead = " ".repeat(AURA_WIDTH + 1);
    let mut guides = String::with_capacity(depth.saturating_sub(top) * (lead.len() + 3));
    for level in top + 1..=depth {
        let more = continues(level);
        guides.push_str(&lead);
        guides.push_str(match (level == depth, more) {
            (false, true) => "│",
            (false, false) => " ",
            (true, true) => "├",
            (true, false) => "└",
        });
    }
    guides
}

fn project_label(agent: &crate::events::AgentSummary) -> String {
    agent
        .project
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(|| agent.name.clone(), ToString::to_string)
        .to_uppercase()
}
