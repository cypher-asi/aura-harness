//! Dragging a project up or down the projects list.
//!
//! A press on a project's header does not fold it at once: it waits to see
//! whether the pointer moves. Released on the row it went down on, it is a
//! click and folds or opens the project as before. Moved to another row,
//! it is a drag: the project, with the agents under it, takes the place of
//! the project under the pointer as soon as the pointer is over that
//! project, and the list is drawn again at every step.
//!
//! Invariants: only whole projects move, so the agents of a project stay
//! right after their root; agents listed before the first root stay put.
//! A swap happens only when the pointer lands on the dragged project's
//! new rows. Without that a tall project and a short one would swap back
//! and forth under a still pointer.

use super::{App, ProjectListRow};
use crate::events::AgentSummary;

/// The left button is down on a project's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectPress {
    /// Id of the root agent heading the project
    id: String,
    /// Screen row it went down on
    row: u16,
    /// The pointer has left that row
    moved: bool,
}

impl App {
    /// Take a press on a project's header, leaving what a click does for
    /// the release. Returns whether it was one.
    pub(super) fn press_project(&mut self, column: u16, row: u16) -> bool {
        if self.agents_tab != super::AgentsTab::Projects {
            return false;
        }
        let Some(index) = self.project_row_at(column, row) else {
            return false;
        };
        if self.project_add_at(index, column).is_some() {
            return false;
        }
        let Some(ProjectListRow::Project { root, .. }) = self.project_rows().get(index).copied()
        else {
            return false;
        };
        let Some(id) = self.agents.get(root).map(|agent| agent.id.clone()) else {
            return false;
        };
        self.project_press = Some(ProjectPress {
            id,
            row,
            moved: false,
        });
        true
    }

    /// The pointer moved to `row` with the button down. Returns whether a
    /// project is pressed, which keeps the drag from selecting text.
    pub(super) fn drag_project(&mut self, row: u16) -> bool {
        let Some(press) = self.project_press.as_mut() else {
            return false;
        };
        press.moved |= row != press.row;
        if press.moved {
            let id = press.id.clone();
            self.move_project_under(&id, row);
        }
        true
    }

    /// The button came up. Returns whether a project was pressed.
    pub(super) fn release_project(&mut self) -> bool {
        let Some(press) = self.project_press.take() else {
            return false;
        };
        if !press.moved {
            if let Some(root) = self.agents.iter().position(|agent| agent.id == press.id) {
                self.toggle_project(root);
            }
        }
        true
    }

    /// Whether a project is being dragged, not just pressed.
    #[must_use]
    pub fn project_dragging(&self) -> bool {
        self.project_press.as_ref().is_some_and(|press| press.moved)
    }

    /// Ids of the root agents heading the projects, in the order listed.
    #[must_use]
    pub fn project_order(&self) -> Vec<String> {
        self.agents
            .iter()
            .filter(|agent| agent.heads_project())
            .map(|agent| agent.id.clone())
            .collect()
    }

    /// Swap the project `id` into the place of the project drawn at screen
    /// `row`, if the pointer would still be over it afterwards.
    fn move_project_under(&mut self, id: &str, row: u16) {
        let Some(list) = self.areas.agent_rows else {
            return;
        };
        let Some(from) = self.agents.iter().position(|agent| agent.id == id) else {
            return;
        };
        let row = row.clamp(list.y, list.y.saturating_add(list.height.saturating_sub(1)));
        let at = self.agents_scroll + usize::from(row - list.y);
        let Some(target) = self.project_at_row(&self.project_rows(), at) else {
            return;
        };
        if target == from {
            return;
        }
        let before: Vec<AgentSummary> = self.agents.clone();
        let selected = self.selected_row_key();
        self.move_project(from, target);
        let lands = self.project_at_row(&self.project_rows(), at) == self.agents_position(id);
        if lands {
            self.restore_selected_row(selected);
            self.project_moves += 1;
        } else {
            self.agents = before;
        }
    }

    fn agents_position(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|agent| agent.id == id)
    }

    /// Root index of the project drawn at list row `at`. A gap belongs to
    /// the project below it; below the last project is the last project.
    fn project_at_row(&self, rows: &[ProjectListRow], at: usize) -> Option<usize> {
        let mut at = at;
        loop {
            match rows.get(at) {
                Some(ProjectListRow::Project { root, .. }) => return Some(*root),
                Some(ProjectListRow::Agent { index }) => return self.project_root(*index),
                Some(ProjectListRow::Gap) => at += 1,
                Some(ProjectListRow::NewProject) | None => {
                    return rows.iter().rev().find_map(|row| match row {
                        ProjectListRow::Project { root, .. } => Some(*root),
                        _ => None,
                    });
                }
            }
        }
    }

    /// Put the project with root `from` where the one with root `to` is:
    /// after it when moving down, before it when moving up.
    fn move_project(&mut self, from: usize, to: usize) {
        let from_end = self.project_end(from);
        let to_end = self.project_end(to);
        let len = from_end - from;
        if from < to {
            self.agents[from..to_end].rotate_left(len);
        } else {
            self.agents[to..from_end].rotate_right(len);
        }
    }

    /// What the highlighted row is, to find it again after a move.
    fn selected_row_key(&self) -> Option<(String, bool)> {
        match self.project_rows().get(self.selected_agent)? {
            ProjectListRow::Project { root, .. } => Some((self.agents[*root].id.clone(), true)),
            ProjectListRow::Agent { index } => Some((self.agents[*index].id.clone(), false)),
            ProjectListRow::NewProject | ProjectListRow::Gap => None,
        }
    }

    fn restore_selected_row(&mut self, key: Option<(String, bool)>) {
        let Some((id, header)) = key else {
            return;
        };
        let found = self.project_rows().iter().position(|row| match *row {
            ProjectListRow::Project { root, .. } => header && self.agents[root].id == id,
            ProjectListRow::Agent { index } => !header && self.agents[index].id == id,
            ProjectListRow::NewProject | ProjectListRow::Gap => false,
        });
        if let Some(row) = found {
            self.selected_agent = row;
        }
    }
}
