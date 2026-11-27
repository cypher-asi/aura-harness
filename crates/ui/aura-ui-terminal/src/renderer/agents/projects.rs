//! The agents panel's PROJECTS tab: projects, each over its agents and
//! the agents they spawned, with "+ New project" pinned to the bottom.

pub(in crate::renderer) use super::drop_marks::DROP_RAIL;
use super::drop_marks::{drop_line_at, mark_drop};
use super::{draw_list, split_pinned, working_icon};
use crate::components::{
    cell_style, format_ago, list_row, tree_row, CellState, RowBadge, StatusIcon, PLAN_BADGE,
};
use crate::renderer::chrome::ScrollView;
use crate::{
    animation::AURA_WIDTH,
    app::{AgentButton, AgentRowEnd, HoverTarget, MenuTarget, PanelFocus, ProjectListRow},
    App, Theme,
};
use aura_core_types::time::now_ms;
use ratatui::{layout::Rect, style::Style, text::Line, Frame};
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
    // While a project or agent is dragged its rows stay lit, and a line
    // in the accent color between rows marks where they go.
    let drag = app.project_drag_hit();
    let rows = app.project_rows();
    let drop_line = drag
        .as_ref()
        .and_then(|drag| drag.edge)
        .and_then(|edge| drop_line_at(&rows, edge));
    let listed = rows.len().saturating_sub(1);
    let roots = project_roots(&rows);
    let swarm = Swarm::new(app, theme, inner, &roots, drag.map(|drag| drag.dragged));
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
        let state = swarm.row_state(row, row_index);
        match row {
            ProjectListRow::Project { root, expanded } => {
                lines.push(swarm.project_line(root, expanded, projects_seen, state));
            }
            ProjectListRow::EmptyProject { project } => {
                lines.push(swarm.empty_project_line(project, state));
            }
            ProjectListRow::Agent { index } => {
                lines.push(swarm.agent_line(index, row_index, state));
            }
            ProjectListRow::NewProject => new_project = swarm.new_project_line(state),
            ProjectListRow::Gap => lines.push(Line::default()),
        }
        if !pinned {
            if let Some(line) = lines.last_mut() {
                mark_drop(
                    line,
                    drop_line,
                    row_index,
                    swarm.width,
                    theme.colors.success,
                );
            }
        }
    }
    (lines, new_project, listed)
}

/// The root agent of every project row; a project's agents run up to the
/// next project's root.
fn project_roots(rows: &[ProjectListRow]) -> Vec<usize> {
    rows.iter()
        .filter_map(|row| match row {
            ProjectListRow::Project { root, .. } => Some(*root),
            _ => None,
        })
        .collect()
}

/// What every row of the PROJECTS tab is drawn with, worked out once a
/// frame.
struct Swarm<'s> {
    app: &'s App,
    theme: &'s Theme,
    width: usize,
    now_ms: u64,
    is_focused: bool,
    selected: usize,
    /// Unfocused, the agent in the chat is lit, or the folded agent over it.
    shown_active: Option<usize>,
    menu_target: Option<&'s MenuTarget>,
    /// Rows of the project or agent being dragged, while one is
    dragged: Option<Range<usize>>,
    /// The root agent of each project, in order
    roots: &'s [usize],
}

/// How one row stands to the cursor and the pointer.
#[derive(Clone, Copy)]
struct RowState {
    is_selected: bool,
    is_hovered: bool,
    /// The pointer is on the row's "+"
    add_hovered: bool,
    /// The pointer is on the row or one of its buttons
    pointer_on_row: bool,
}

impl<'s> Swarm<'s> {
    fn new(
        app: &'s App,
        theme: &'s Theme,
        inner: Rect,
        roots: &'s [usize],
        dragged: Option<Range<usize>>,
    ) -> Self {
        Self {
            app,
            theme,
            width: inner.width as usize,
            now_ms: now_ms(),
            is_focused: app.focus() == PanelFocus::Swarm,
            selected: app.selected_agent(),
            shown_active: app.shown_active_agent(),
            menu_target: app.context_menu().map(|menu| &menu.target),
            dragged,
            roots,
        }
    }

    fn button_hovered(&self, button: AgentButton, row_index: usize) -> bool {
        self.app.hover() == Some(HoverTarget::agent_button(button, row_index))
    }

    fn row_state(&self, row: ProjectListRow, row_index: usize) -> RowState {
        let hover = self.app.hover();
        let is_selected = match row {
            ProjectListRow::Project { .. }
            | ProjectListRow::EmptyProject { .. }
            | ProjectListRow::Gap => false,
            _ if self.is_focused => row_index == self.selected,
            ProjectListRow::Agent { index } => self.shown_active == Some(index),
            ProjectListRow::NewProject => false,
        };
        let add_hovered =
            matches!(hover, Some(HoverTarget::AddToProject(index)) if index == row_index);
        let pointer_on_row = AgentButton::ALL
            .into_iter()
            .any(|button| self.button_hovered(button, row_index))
            || matches!(hover, Some(HoverTarget::ProjectRow(index)) if index == row_index);
        let is_hovered = add_hovered
            || pointer_on_row
            || self.menu_on(row)
            || self
                .dragged
                .as_ref()
                .is_some_and(|dragged| dragged.contains(&row_index));
        RowState {
            is_selected,
            is_hovered,
            add_hovered,
            pointer_on_row,
        }
    }

    /// Whether the open context menu is for `row`.
    fn menu_on(&self, row: ProjectListRow) -> bool {
        let app = self.app;
        match (row, self.menu_target) {
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
        }
    }

    fn project_line<'a>(
        &self,
        root: usize,
        expanded: bool,
        projects_seen: usize,
        state: RowState,
    ) -> Line<'a> {
        let (app, theme) = (self.app, self.theme);
        let end = self
            .roots
            .get(projects_seen)
            .copied()
            .unwrap_or(app.agents().len());
        let agents = &app.agents()[root..end];
        let icon = if !expanded && agents.iter().any(|agent| app.agent_thinking(agent)) {
            working_icon(app, theme)
        } else {
            let folder = if expanded { "🗁" } else { "🗀" };
            StatusIcon::new(folder, theme.colors.muted)
        };
        let project_name = Style::default().fg(theme.colors.muted);
        let mut cell = cell_style(
            theme,
            CellState::of(state.is_selected, state.is_hovered),
            project_name,
        );
        if state.add_hovered {
            cell.time = theme.colors.foreground;
        }
        list_row(
            self.width,
            0,
            icon,
            &project_label(&app.agents()[root]),
            cell,
            Some("+"),
        )
    }

    fn empty_project_line<'a>(&self, project: usize, state: RowState) -> Line<'a> {
        let theme = self.theme;
        let name = self
            .app
            .projects()
            .get(project)
            .map(|p| p.name.to_uppercase())
            .unwrap_or_default();
        let project_name = Style::default().fg(theme.colors.muted);
        let mut cell = cell_style(
            theme,
            CellState::of(state.is_selected, state.is_hovered),
            project_name,
        );
        if state.add_hovered {
            cell.time = theme.colors.foreground;
        }
        let icon = StatusIcon::new("🗀", theme.colors.muted);
        list_row(self.width, 0, icon, &name, cell, Some("+"))
    }

    fn agent_line<'a>(&self, index: usize, row_index: usize, state: RowState) -> Line<'a> {
        let (app, theme) = (self.app, self.theme);
        let agent = &app.agents()[index];
        let icon = self.agent_icon(index);
        let ago = agent.last_active_ms.map(|ms| format_ago(ms, self.now_ms));
        let name = if agent.depth == 0 {
            Style::default().fg(theme.colors.foreground)
        } else {
            Style::default().fg(theme.colors.muted)
        };
        let cell = cell_style(
            theme,
            CellState::of(state.is_selected, state.is_hovered),
            name,
        );
        // The buttons show only under the pointer, muted like the
        // time, and the one that is hovered lights up.
        let buttons = AgentButton::ALL.map(|button| {
            let color = if self.button_hovered(button, row_index) && !state.is_selected {
                theme.colors.foreground
            } else {
                cell.time
            };
            (button, color)
        });
        let buttons: &[_] = if state.pointer_on_row { &buttons } else { &[] };
        let cloud = app.cloud_badge(&agent.id);
        let badge = self.agent_badge(agent.plan_mode, cloud.as_ref());
        let end = AgentRowEnd::agent(ago.as_deref());
        tree_row(
            self.width,
            (&spawn_guides(app.agents(), index), theme.colors.muted),
            icon,
            (&agent.name, badge),
            cell,
            &end,
            buttons,
        )
    }

    /// The icon column of the agent at `index`: its fold marker, ▾ over
    /// its spawns or ▸ folded over them, blank without any.
    fn agent_icon(&self, index: usize) -> StatusIcon {
        let app = self.app;
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
        if app.agent_row_busy(index) {
            working_icon(app, self.theme)
        } else {
            StatusIcon::new(marker.unwrap_or(""), self.theme.colors.muted)
        }
    }

    /// Plan mode wins the badge; otherwise a cloud agent shows
    /// a cloud, dimmed with its owner's name for another user's.
    fn agent_badge<'b>(
        &self,
        plan_mode: bool,
        cloud: Option<&'b (String, bool)>,
    ) -> Option<RowBadge<'b>> {
        let colors = &self.theme.colors;
        if plan_mode {
            Some(RowBadge {
                text: PLAN_BADGE,
                color: colors.success,
            })
        } else {
            cloud.map(|(text, foreign)| RowBadge {
                text,
                color: if *foreign { colors.faint } else { colors.muted },
            })
        }
    }

    fn new_project_line<'a>(&self, state: RowState) -> Line<'a> {
        let theme = self.theme;
        let cell = cell_style(
            theme,
            CellState::of(state.is_selected, state.is_hovered),
            Style::default().fg(theme.colors.muted),
        );
        let icon = StatusIcon::new("+", cell.text.fg.unwrap_or(theme.colors.muted));
        list_row(self.width, 0, icon, "New project", cell, None)
    }
}

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
