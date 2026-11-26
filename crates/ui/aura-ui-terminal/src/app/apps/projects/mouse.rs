//! Where the pointer is in the projects list, and what a click there does.

use crate::app::{
    agent_button_at, contains, AgentButton, AgentRowEnd, App, HoverTarget, ProjectListRow,
    PROJECT_ADD_WIDTH,
};
use crate::events::UiEvent;

impl App {
    /// What the pointer is over in the projects list.
    pub(in crate::app) fn hovered_project_row(&self, column: u16, row: u16) -> Option<HoverTarget> {
        let index = self.project_row_at(column, row)?;
        if self.project_add_at(index, column).is_some() {
            return Some(HoverTarget::AddToProject(index));
        }
        if let Some((button, _)) = self.agent_button_hit(index, column) {
            return Some(HoverTarget::agent_button(button, index));
        }
        Some(HoverTarget::ProjectRow(index))
    }

    /// Projects-list row drawn at a screen cell: a listed project or agent,
    /// or the pinned "+ New project". Gaps are not rows to act on.
    pub(in crate::app) fn project_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self.project_rows();
        let new_project = rows.len().checked_sub(1)?;
        if contains(self.areas.new_project_row, column, row) {
            return Some(new_project);
        }
        let list = self.areas.agent_rows?;
        if !contains(Some(list), column, row) {
            return None;
        }
        let index = self.project_list.scroll + usize::from(row - list.y);
        matches!(
            rows[..new_project].get(index)?,
            ProjectListRow::Project { .. }
                | ProjectListRow::EmptyProject { .. }
                | ProjectListRow::Agent { .. }
        )
        .then_some(index)
    }

    /// Key of the project whose header is `row`.
    pub(in crate::app) fn header_key(&self, row: ProjectListRow) -> Option<String> {
        match row {
            ProjectListRow::Project { root, .. } => self.project_key(root),
            ProjectListRow::EmptyProject { project } => self
                .project_list
                .projects
                .get(project)
                .map(|p| p.id.clone()),
            ProjectListRow::Agent { .. } | ProjectListRow::NewProject | ProjectListRow::Gap => None,
        }
    }

    /// Key of the project whose "+" is at `column` on projects-list row
    /// `index`.
    pub(in crate::app) fn project_add_at(&self, index: usize, column: u16) -> Option<String> {
        let list = self.areas.agent_rows?;
        let key = self.header_key(*self.project_rows().get(index)?)?;
        (column + PROJECT_ADD_WIDTH >= list.x + list.width).then_some(key)
    }

    /// The button at `column` on projects-list row `index`, if the row is
    /// an agent's, and the agent's id. Where the buttons sit follows the
    /// row's time, which is formatted against the clock just as the
    /// renderer does it, and the code column.
    pub(in crate::app) fn agent_button_hit(
        &self,
        index: usize,
        column: u16,
    ) -> Option<(AgentButton, String)> {
        let list = self.areas.agent_rows?;
        let ProjectListRow::Agent { index: agent } = *self.project_rows().get(index)? else {
            return None;
        };
        let agent = self.agents.get(agent)?;
        let now = aura_core_types::time::now_ms();
        let ago = agent
            .last_active_ms
            .map(|ms| crate::renderer::format_ago(ms, now));
        let end = AgentRowEnd::agent(ago.as_deref());
        let offset = column.checked_sub(list.x)?;
        let button = agent_button_at(list.width, &end, offset)?;
        Some((button, agent.id.clone()))
    }

    /// Unfold the project with key `key` and ask for a new agent in it.
    pub(in crate::app) fn add_agent_to_project(&mut self, key: &str) {
        self.project_list.collapsed_projects.remove(key);
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::AddAgentToProject(key.to_string()));
        }
    }

    /// Clicking a project folds or opens it and leaves the highlight where
    /// it was, as does clicking the fold marker of an agent with spawns;
    /// clicking an agent elsewhere or "+ New project" highlights and acts
    /// on it.
    pub(in crate::app) fn click_agent_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.project_row_at(column, row) else {
            return;
        };
        if let Some(key) = self.project_add_at(index, column) {
            self.add_agent_to_project(&key);
        } else if let Some((button, id)) = self.agent_button_hit(index, column) {
            match button {
                AgentButton::Archive => self.archive_agent(id),
            }
        } else if let Some(agent) = self.fold_marker_at(index, column) {
            // Like a project's header, the marker leaves a list the wheel
            // scrolled where it is.
            self.toggle_agent(agent);
        } else if let Some(ProjectListRow::Project { root, .. }) = self.project_rows().get(index) {
            self.toggle_project(*root);
        } else if let Some(ProjectListRow::EmptyProject { .. }) = self.project_rows().get(index) {
            // Nothing to fold or open.
        } else {
            self.project_list.selected = index;
            self.activate_agent_row();
        }
    }

    /// Agent whose fold marker is at `column` on projects-list row `row`.
    /// An agent with spawns draws its marker as `tree_row` lays the icon
    /// column out: after one leading space and the spawn guides,
    /// `AURA_WIDTH + 2` columns per level. An idle agent's marker opens the
    /// icon column, and the blank rest of that column counts too, as it
    /// does for [`processes::on_row_icon`]. A busy agent's ring stands in
    /// for its marker, open or folded, so the ring and the spare column
    /// after it are the button just the same.
    pub(in crate::app) fn fold_marker_at(&self, row: usize, column: u16) -> Option<usize> {
        use crate::animation::AURA_WIDTH;
        let list = self.areas.agent_rows?;
        let ProjectListRow::Agent { index } = *self.project_rows().get(row)? else {
            return None;
        };
        if !self.agent_has_spawns(index) {
            return None;
        }
        let start = 1 + self.agents[index].depth * (AURA_WIDTH + 2);
        let offset = usize::from(column.checked_sub(list.x)?);
        (start..=start + AURA_WIDTH)
            .contains(&offset)
            .then_some(index)
    }
}
