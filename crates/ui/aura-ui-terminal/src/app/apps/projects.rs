//! The PROJECTS app: the listed agents under their projects, each with the
//! agents it spawned below it. Rows fold, open a project's info, start a
//! project or an agent in one, and put an agent on screen.

use crate::app::folder_picker::FolderPicker;
use crate::app::{
    agent_button_at, contains, follow_selection, AgentButton, AgentRowEnd, App, AppState,
    NotificationType, ProjectListRow, PROJECT_ADD_WIDTH,
};
use crate::events::{empty_projects, starts_project, AgentSummary, ProjectSummary, UiEvent};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;

impl App {
    /// Highlight projects-list row `row`. Landing on an agent switches to it.
    /// Only agents and "+ New project" take the highlight: project headers
    /// and gaps are stepped over in the direction of travel, or the other
    /// way at the end of the list.
    pub(in crate::app) fn select_agent_row(&mut self, row: usize) {
        self.agents_scroll_detached = false;
        let rows = self.project_rows();
        if rows.is_empty() {
            self.selected_agent = 0;
            return;
        }
        let target = row.min(rows.len() - 1);
        let takes_highlight = |&row: &usize| {
            matches!(
                rows[row],
                ProjectListRow::Agent { .. } | ProjectListRow::NewProject
            )
        };
        let found = if target < self.selected_agent {
            (0..=target)
                .rev()
                .find(takes_highlight)
                .or_else(|| (target..rows.len()).find(takes_highlight))
        } else {
            (target..rows.len())
                .find(takes_highlight)
                .or_else(|| (0..=target).rev().find(takes_highlight))
        };
        let Some(row) = found else {
            return;
        };
        self.selected_agent = row;
        if let Some(ProjectListRow::Agent { index }) = rows.get(self.selected_agent) {
            let agent_id = self.agents[*index].id.clone();
            self.request_agent_switch(agent_id);
        }
    }

    /// Act on the highlighted projects-list row: fold a project, switch to
    /// an agent, or create one from "+ New project".
    pub(in crate::app) fn activate_agent_row(&mut self) {
        self.agents_scroll_detached = false;
        match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => self.toggle_project(root),
            Some(ProjectListRow::Agent { index }) => {
                let agent_id = self.agents[index].id.clone();
                self.request_agent_switch(agent_id);
            }
            Some(ProjectListRow::NewProject) | None => self.prompt_new_project(),
            Some(ProjectListRow::EmptyProject { .. } | ProjectListRow::Gap) => {}
        }
    }

    /// Highlight the last agent, opening its project if it was folded.
    pub(in crate::app) fn select_last_agent_row(&mut self) {
        let Some(index) = self.agents.len().checked_sub(1) else {
            self.select_agent_row(0);
            return;
        };
        self.reveal_agent(index);
        let row = self.row_of_agent(index).unwrap_or(0);
        self.select_agent_row(row);
    }

    /// Ask for a folder, then open it as a project named after it.
    /// Browsing starts in the launch directory.
    pub(in crate::app) fn prompt_new_project(&mut self) {
        let start = std::env::current_dir()
            .ok()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        match FolderPicker::open(start) {
            Ok(picker) => {
                self.folder_picker = Some(picker);
                self.state = AppState::SelectingFolder;
                self.status = "Project folder".to_string();
            }
            Err(e) => {
                self.notification = Some((
                    format!("Cannot browse folders: {e}"),
                    NotificationType::Error,
                ));
            }
        }
    }

    /// Open the folder browser for a new project, as Ctrl+N does.
    pub fn ask_for_project_folder(&mut self) {
        self.prompt_new_project();
    }

    /// Close the folder browser and open the folder as a project.
    pub(in crate::app) fn choose_project_folder(&mut self, folder: PathBuf) {
        self.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::CreateProject { folder });
        }
    }

    /// Close the folder browser without creating a project.
    pub(in crate::app) fn cancel_folder_picker(&mut self) {
        self.folder_picker = None;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.notification = Some(("Cancelled".to_string(), NotificationType::Warning));
    }

    /// A path was dropped on the window. A folder becomes a project named
    /// after it, with the same event the folder browser sends; anything
    /// else is refused with a notice. Returns whether a project was asked
    /// for.
    pub fn drop_path(&mut self, path: &std::path::Path) -> bool {
        if !path.is_dir() {
            self.notification = Some((
                "Drop a folder to start a project".to_string(),
                NotificationType::Warning,
            ));
            return false;
        }
        let Some(tx) = &self.event_tx else {
            return false;
        };
        let event = UiEvent::CreateProject {
            folder: path.to_path_buf(),
        };
        if tx.try_send(event).is_err() {
            self.notification = Some((
                "Cannot add the project right now".to_string(),
                NotificationType::Error,
            ));
            return false;
        }
        true
    }

    /// Folder browser for a new project, while it is open.
    #[must_use]
    pub const fn folder_picker(&self) -> Option<&FolderPicker> {
        self.folder_picker.as_ref()
    }

    /// Keep the folder browser's highlight inside a list `visible_rows`
    /// tall, returning the first row to show.
    pub fn sync_folder_picker_scroll(&mut self, visible_rows: usize) -> usize {
        self.folder_picker
            .as_mut()
            .map_or(0, |picker| picker.sync_scroll(visible_rows))
    }

    /// Listed projects, in list order.
    #[must_use]
    pub fn projects(&self) -> &[ProjectSummary] {
        &self.projects
    }

    /// The project whose details are shown, while they are.
    #[must_use]
    pub fn project_info(&self) -> Option<&ProjectSummary> {
        if self.state != AppState::ShowingProjectInfo {
            return None;
        }
        let key = self.project_info.as_deref()?;
        self.projects.iter().find(|project| project.id == key)
    }

    /// Show the details of the project with key `key`.
    pub(in crate::app) fn show_project_info(&mut self, key: &str) {
        if !self.projects.iter().any(|project| project.id == key) {
            self.notification = Some((
                "No details for that project".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.project_info = Some(key.to_string());
        self.state = AppState::ShowingProjectInfo;
    }

    /// Show the details of the project the active agent is in.
    pub(in crate::app) fn show_active_project_info(&mut self) {
        match self.project_key_of_agent(&self.active_agent_id) {
            Some(key) => self.show_project_info(&key),
            None => {
                self.notification =
                    Some(("No project is open".to_string(), NotificationType::Warning));
            }
        }
    }

    /// Close the project details.
    pub(in crate::app) fn close_project_info(&mut self) {
        self.project_info = None;
        self.state = AppState::Idle;
    }

    /// Fold or open what is under the highlight: an agent with spawns
    /// folds over them, any other agent row folds the project that
    /// contains it, as does a project's header.
    pub(in crate::app) fn toggle_selected_fold(&mut self) {
        self.agents_scroll_detached = false;
        let root = match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => Some(root),
            Some(ProjectListRow::Agent { index }) if self.agent_has_spawns(index) => {
                self.toggle_agent(index);
                return;
            }
            Some(ProjectListRow::Agent { index }) => self.project_root(index),
            Some(
                ProjectListRow::EmptyProject { .. }
                | ProjectListRow::NewProject
                | ProjectListRow::Gap,
            )
            | None => None,
        };
        if let Some(root) = root {
            self.toggle_project(root);
        }
    }

    /// Fold or open the project that contains the highlighted row, from
    /// any agent in it or its header, even an agent that could fold over
    /// its own spawns.
    pub(in crate::app) fn toggle_selected_project(&mut self) {
        self.agents_scroll_detached = false;
        let root = match self.project_rows().get(self.selected_agent).copied() {
            Some(ProjectListRow::Project { root, .. }) => Some(root),
            Some(ProjectListRow::Agent { index }) => self.project_root(index),
            Some(
                ProjectListRow::EmptyProject { .. }
                | ProjectListRow::NewProject
                | ProjectListRow::Gap,
            )
            | None => None,
        };
        if let Some(root) = root {
            self.toggle_project(root);
        }
    }

    /// Switch to the agent `delta` rows away from the active one.
    pub(in crate::app) fn switch_agent_by(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let current = self
            .agents
            .iter()
            .position(|a| a.id == self.active_agent_id)
            .unwrap_or(0);
        let last = self.agents.len() - 1;
        let target = current.saturating_add_signed(delta).min(last);
        self.reveal_agent(target);
        let row = self.row_of_agent(target).unwrap_or(0);
        self.select_agent_row(row);
    }

    /// First visible line of the agents list.
    #[must_use]
    pub const fn agents_scroll(&self) -> usize {
        self.agents_scroll
    }

    /// Keep the highlighted projects-list row inside a list `visible_rows`
    /// tall. "+ New project" is pinned below the list, so it never scrolls.
    /// After the wheel scrolled the list, it stays put until the highlight
    /// moves again.
    pub fn sync_agents_scroll(&mut self, visible_rows: usize) {
        let total = self.listed_project_rows();
        let visible = visible_rows.max(1);
        self.agents_scroll = if self.agents_scroll_detached {
            self.agents_scroll.min(total.saturating_sub(visible))
        } else {
            follow_selection(self.agents_scroll, self.selected_agent, visible, total)
        };
    }

    /// Projects-list rows that scroll: all but the pinned "+ New project".
    pub(in crate::app) fn listed_project_rows(&self) -> usize {
        self.project_rows().len().saturating_sub(1)
    }

    /// Record where the agents list rows were drawn this frame.
    pub fn set_agent_rows_area(&mut self, area: Rect) {
        self.areas.agent_rows = Some(area);
    }

    /// Record where "+ New project" was drawn this frame.
    pub fn set_new_project_area(&mut self, area: Rect) {
        self.areas.new_project_row = Some(area);
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
        let index = self.agents_scroll + usize::from(row - list.y);
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
            ProjectListRow::EmptyProject { project } => {
                self.projects.get(project).map(|p| p.id.clone())
            }
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
        let now = crate::renderer::now_ms();
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
        self.collapsed_projects.remove(key);
        if let Some(tx) = &self.event_tx {
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
            self.selected_agent = index;
            self.activate_agent_row();
        }
    }

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

    /// Get the selected agent index.
    #[must_use]
    pub const fn selected_agent(&self) -> usize {
        self.selected_agent
    }

    /// Handle key when the agents panel shows Projects. Moving the
    /// highlight onto an agent switches to it; project headers are skipped.
    /// Space folds the highlighted agent over its spawns, or its project
    /// when it has none; Shift+Space folds its project either way. Enter
    /// on "+ New project" creates one.
    pub(in crate::app) fn handle_swarm_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            KeyCode::Up => self.select_agent_row(self.selected_agent.saturating_sub(1)),
            KeyCode::Down => self.select_agent_row(self.selected_agent + 1),
            KeyCode::Enter => self.activate_agent_row(),
            KeyCode::PageUp => self.select_agent_row(self.selected_agent.saturating_sub(5)),
            KeyCode::PageDown => self.select_agent_row(self.selected_agent + 5),
            KeyCode::Home => self.select_agent_row(0),
            KeyCode::End => self.select_last_agent_row(),
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }
}
