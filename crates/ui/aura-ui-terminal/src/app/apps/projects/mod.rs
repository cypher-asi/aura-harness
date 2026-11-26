//! The PROJECTS app: the listed agents under their projects, each with the
//! agents it spawned below it. Rows fold, open a project's info, start a
//! project or an agent in one, and put an agent on screen.

mod folders;
mod mouse;
mod tree;

use crate::app::{follow_selection, App, ProjectListRow};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

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

    /// Space folds the highlighted agent, or with Shift its project;
    /// Delete archives the highlighted agent, or with Shift removes its
    /// project. Returns whether `key` was one of them.
    pub(in crate::app) fn projects_shortcut(&mut self, key: KeyEvent, word_motion: bool) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char(' ') if !word_motion && shift => self.toggle_selected_project(),
            KeyCode::Char(' ') if !word_motion => self.toggle_selected_fold(),
            KeyCode::Delete if shift => self.remove_selected_project(),
            KeyCode::Delete => self.archive_selected_agent(),
            _ => return false,
        }
        true
    }

    /// Scroll the projects list `delta` rows, leaving the highlight where
    /// it is; down stops at the last row.
    pub(in crate::app) fn scroll_project_rows(&mut self, delta: isize) {
        let scroll = self.agents_scroll.saturating_add_signed(delta);
        self.agents_scroll = if delta > 0 {
            let visible = self
                .areas
                .agent_rows
                .map_or(1, |area| usize::from(area.height));
            scroll.min(self.listed_project_rows().saturating_sub(visible))
        } else {
            scroll
        };
        self.agents_scroll_detached = true;
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
                self.input.text.insert(self.input.cursor_pos, c);
                self.input.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.input.cursor_pos > 0 {
                    self.input.cursor_pos -= 1;
                    self.input.text.remove(self.input.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }
}
