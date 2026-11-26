//! The projects list as rows: projects, their agents and the agents they
//! spawned, and folding them.

use crate::app::{App, ProjectListRow};
use crate::events::{empty_projects, starts_project, AgentSummary};

impl App {
    /// Get the list of agents.
    #[must_use]
    pub fn agents(&self) -> &[AgentSummary] {
        &self.agents
    }

    /// Visible projects-list rows. Every project contributes a header and,
    /// while expanded, its agents with the agents they spawned.
    /// A gap row separates each project from the one above it.
    #[must_use]
    pub(crate) fn project_rows(&self) -> Vec<ProjectListRow> {
        let gap = |rows: &mut Vec<ProjectListRow>| {
            if !rows.is_empty() {
                rows.push(ProjectListRow::Gap);
            }
        };
        let mut empty = empty_projects(&self.projects, &self.agents)
            .into_iter()
            .peekable();
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.agents.len() {
            if starts_project(&self.agents, index) {
                while let Some((_, project)) = empty.next_if(|&(before, _)| before <= index) {
                    gap(&mut rows);
                    rows.push(ProjectListRow::EmptyProject { project });
                }
                let root = index;
                gap(&mut rows);
                let end = self.project_end(root);
                let expanded = !self
                    .collapsed_projects
                    .contains(self.agents[root].project_key());
                rows.push(ProjectListRow::Project { root, expanded });
                if expanded {
                    let mut agent_index = root;
                    while agent_index < end {
                        rows.push(ProjectListRow::Agent { index: agent_index });
                        agent_index = self.next_shown_agent(agent_index);
                    }
                }
                index = end;
            } else {
                rows.push(ProjectListRow::Agent { index });
                index = self.next_shown_agent(index);
            }
        }
        for (_, project) in empty {
            gap(&mut rows);
            rows.push(ProjectListRow::EmptyProject { project });
        }
        rows.push(ProjectListRow::NewProject);
        rows
    }

    /// Index just past the agents of the project whose first agent is
    /// `root`.
    pub(in crate::app) fn project_end(&self, root: usize) -> usize {
        (root + 1..self.agents.len())
            .find(|&i| starts_project(&self.agents, i))
            .unwrap_or(self.agents.len())
    }

    /// First agent of the project that contains `index`.
    pub(in crate::app) fn project_root(&self, index: usize) -> Option<usize> {
        (0..=index).rev().find(|&i| starts_project(&self.agents, i))
    }

    /// Key of the project whose first agent is `root`.
    pub(in crate::app) fn project_key(&self, root: usize) -> Option<String> {
        self.agents
            .get(root)
            .map(|agent| agent.project_key().to_string())
    }

    /// First agent of the project with key `key`.
    pub(in crate::app) fn project_root_of_key(&self, key: &str) -> Option<usize> {
        (0..self.agents.len())
            .find(|&i| starts_project(&self.agents, i) && self.agents[i].project_key() == key)
    }

    /// Open a folded project, and any folded agent above it, so `index`
    /// has a visible row.
    pub(in crate::app) fn reveal_agent(&mut self, index: usize) {
        if let Some(key) = self
            .project_root(index)
            .and_then(|root| self.project_key(root))
        {
            self.collapsed_projects.remove(&key);
        }
        self.open_ancestors(index);
    }

    /// Open every folded agent that agent `index` is spawned under.
    pub(in crate::app) fn open_ancestors(&mut self, index: usize) {
        let ancestors: Vec<usize> =
            std::iter::successors(self.parent_of(index), |&i| self.parent_of(i)).collect();
        for ancestor in ancestors {
            self.collapsed_agents.remove(&self.agents[ancestor].id);
        }
    }

    /// The agent that spawned agent `index`: the nearest one above it in
    /// its project with a smaller depth.
    pub(in crate::app) fn parent_of(&self, index: usize) -> Option<usize> {
        let depth = self.agents.get(index)?.depth;
        if depth == 0 || starts_project(&self.agents, index) {
            return None;
        }
        let root = self.project_root(index)?;
        (root..index).rev().find(|&i| self.agents[i].depth < depth)
    }

    /// Index just past agent `index` and the agents spawned under it. Its
    /// spawns stop at the next agent no deeper than it, or at the next
    /// project.
    pub(crate) fn spawn_end(&self, index: usize) -> usize {
        let Some(depth) = self.agents.get(index).map(|agent| agent.depth) else {
            return self.agents.len();
        };
        (index + 1..self.agents.len())
            .find(|&i| self.agents[i].depth <= depth || starts_project(&self.agents, i))
            .unwrap_or(self.agents.len())
    }

    /// Whether agent `index` has spawned agents listed under it, so it
    /// can fold.
    pub(crate) fn agent_has_spawns(&self, index: usize) -> bool {
        let next = index + 1;
        match (self.agents.get(index), self.agents.get(next)) {
            (Some(agent), Some(after)) => {
                after.depth > agent.depth && !starts_project(&self.agents, next)
            }
            _ => false,
        }
    }

    /// Whether agent `index` is folded shut over the agents it spawned.
    pub(crate) fn agent_folded(&self, index: usize) -> bool {
        self.agents
            .get(index)
            .is_some_and(|agent| self.collapsed_agents.contains(&agent.id))
            && self.agent_has_spawns(index)
    }

    /// The agent listed after agent `index`, past its spawns when folded.
    pub(in crate::app) fn next_shown_agent(&self, index: usize) -> usize {
        if self.agent_folded(index) {
            self.spawn_end(index)
        } else {
            index + 1
        }
    }

    /// Fold or open agent `index` over the agents it spawned. Folding it
    /// parks the highlight on it when the highlight was on one of the
    /// rows it hides; opening it again while the highlight is parked there
    /// brings the highlight back to the agent shown in the chat if it is
    /// one of the spawns. Otherwise the highlight stays on the same row as
    /// rows above it come and go.
    pub(crate) fn toggle_agent(&mut self, index: usize) {
        if !self.agent_has_spawns(index) {
            return;
        }
        let id = self.agents[index].id.clone();
        let spawns = index + 1..self.spawn_end(index);
        let before = self.project_rows().get(self.selected_agent).copied();
        let opened = self.collapsed_agents.remove(&id);
        if !opened {
            self.collapsed_agents.insert(id);
        }
        let on_agent = before == Some(ProjectListRow::Agent { index });
        let hidden = matches!(before, Some(ProjectListRow::Agent { index: found }) if spawns.contains(&found))
            && self.agent_folded(index);
        let active_row = || {
            spawns
                .clone()
                .find(|&i| self.agents[i].id == self.active_agent_id)
                .and_then(|i| self.row_of_agent(i))
        };
        let kept = if hidden {
            self.row_of_agent(index)
        } else if opened && on_agent {
            active_row().or_else(|| self.row_of_agent(index))
        } else {
            let rows = self.project_rows();
            before.and_then(|before| rows.iter().position(|row| *row == before))
        };
        if let Some(pos) = kept {
            self.selected_agent = pos;
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
    }

    /// Whether `agent` is thinking: it runs, or it is the agent on screen
    /// and a turn is in flight.
    pub(crate) fn agent_thinking(&self, agent: &crate::events::AgentSummary) -> bool {
        agent.running || (agent.id == self.active_agent_id && self.is_processing())
    }

    /// Whether the row of agent `index` shows the aura ring: the agent is
    /// thinking, or it is folded over a spawn that is.
    pub(crate) fn agent_row_busy(&self, index: usize) -> bool {
        let Some(agent) = self.agents.get(index) else {
            return false;
        };
        let hidden = if self.agent_folded(index) {
            self.agents.get(index + 1..self.spawn_end(index))
        } else {
            None
        };
        self.agent_thinking(agent)
            || hidden.is_some_and(|hidden| hidden.iter().any(|a| self.agent_thinking(a)))
    }

    /// The agent whose row stands for the agent shown in the chat: that
    /// agent when it has a row, else the nearest agent above it folded
    /// over it. `None` when neither is shown, as under a folded project.
    pub(crate) fn shown_active_agent(&self) -> Option<usize> {
        let index = self
            .agents
            .iter()
            .position(|agent| agent.id == self.active_agent_id)?;
        let rows = self.project_rows();
        let shown = |i: usize| rows.contains(&ProjectListRow::Agent { index: i });
        std::iter::successors(Some(index), |&i| self.parent_of(i)).find(|&i| shown(i))
    }

    /// Visible row of agent `index`, when its project is expanded.
    pub(in crate::app) fn row_of_agent(&self, index: usize) -> Option<usize> {
        self.project_rows().iter().position(
            |row| matches!(row, ProjectListRow::Agent { index: found } if *found == index),
        )
    }

    /// Fold or open the project rooted at `root`. Folding it parks the
    /// highlight on the header, where it is not drawn, when the highlight
    /// was inside that project; opening it again brings the highlight back
    /// to the agent shown in the chat if it is in there. Otherwise the
    /// highlight stays on the same row as rows above it come and go.
    pub(in crate::app) fn toggle_project(&mut self, root: usize) {
        let Some(id) = self.project_key(root) else {
            return;
        };
        let span = root..self.project_end(root);
        let before = self.project_rows().get(self.selected_agent).copied();
        let selected_inside = before.is_some_and(|row| match row {
            ProjectListRow::Project { root: found, .. } => found == root,
            ProjectListRow::Agent { index } => span.contains(&index),
            ProjectListRow::EmptyProject { .. }
            | ProjectListRow::NewProject
            | ProjectListRow::Gap => false,
        });
        let opened = self.collapsed_projects.remove(&id);
        if !opened {
            self.collapsed_projects.insert(id);
        }
        let rows = self.project_rows();
        let active_row = || {
            span.clone()
                .find(|&index| self.agents[index].id == self.active_agent_id)
                .and_then(|index| self.row_of_agent(index))
        };
        let kept = if selected_inside {
            (opened.then(active_row).flatten()).or_else(|| {
                rows.iter().position(
                    |row| matches!(row, ProjectListRow::Project { root: found, .. } if *found == root),
                )
            })
        } else {
            before.and_then(|before| rows.iter().position(|row| *row == before))
        };
        if let Some(pos) = kept {
            self.selected_agent = pos;
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
    }

    /// Move the highlight to the active agent, to the folded agent it is
    /// under, or to its project when folded.
    pub(in crate::app) fn select_visible_active(&mut self) {
        if let Some(row) = self
            .shown_active_agent()
            .and_then(|index| self.row_of_agent(index))
        {
            self.selected_agent = row;
            return;
        }
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.id == self.active_agent_id)
        {
            if let Some(root) = self.project_root(index) {
                if let Some(row) = self.project_rows().iter().position(|row| {
                    matches!(row, ProjectListRow::Project { root: found, .. } if *found == root)
                }) {
                    self.selected_agent = row;
                    return;
                }
            }
        }
        let len = self.project_rows().len();
        self.selected_agent = self.selected_agent.min(len.saturating_sub(1));
    }
}
