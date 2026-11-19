//! Dragging a project up or down the projects list, or an agent up or
//! down its project.
//!
//! A press on a project's header does not fold it at once: it waits to see
//! whether the pointer moves. Released on the row it went down on, it is a
//! click and folds or opens the project as before. Moved to another row,
//! it is a drag: the project, with the agents under it, takes the place of
//! the project under the pointer as soon as the pointer is over that
//! project, and the list is drawn again at every step.
//!
//! A press on a top-level agent opens it at once, as a click does; moving
//! the pointer with the button down then drags it, with the agents it
//! spawned, over the other top-level agents of its project. It never
//! leaves its project.
//!
//! When the button comes up after a drag, the new order goes to the
//! session, which keeps it.
//!
//! Invariants: only whole projects move, so the agents of a project stay
//! together; an agent moves with everything under it. A swap happens only
//! when the pointer lands on the dragged rows' new place. Without that a
//! tall block and a short one would swap back and forth under a still
//! pointer.

use super::{App, ProjectListRow};
use crate::events::{starts_project, AgentSummary, UiEvent};

/// What the left button went down on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pressed {
    /// A project's header, by project key
    Project(String),
    /// A top-level agent, by id
    Agent(String),
}

/// The left button is down on a project's header or a top-level agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectPress {
    pressed: Pressed,
    /// Screen row it went down on
    row: u16,
    /// The pointer has left that row
    moved: bool,
}

impl App {
    /// Take a press on a project's header, leaving what a click does for
    /// the release, or on a top-level agent, which the click handles at
    /// once. Returns whether the press was on a project's header.
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
        let pressed = match self.project_rows().get(index).copied() {
            Some(ProjectListRow::Project { root, .. }) => {
                Pressed::Project(self.agents[root].project_key().to_string())
            }
            Some(ProjectListRow::Agent { index }) if self.agents[index].depth == 0 => {
                Pressed::Agent(self.agents[index].id.clone())
            }
            _ => return false,
        };
        let header = matches!(pressed, Pressed::Project(_));
        self.project_press = Some(ProjectPress {
            pressed,
            row,
            moved: false,
        });
        header
    }

    /// The pointer moved to `row` with the button down. Returns whether a
    /// project or agent is pressed, which keeps the drag from selecting
    /// text.
    pub(super) fn drag_project(&mut self, row: u16) -> bool {
        let Some(press) = self.project_press.as_mut() else {
            return false;
        };
        press.moved |= row != press.row;
        if press.moved {
            match press.pressed.clone() {
                Pressed::Project(key) => self.move_project_under(&key, row),
                Pressed::Agent(id) => self.move_agent_under(&id, row),
            }
        }
        true
    }

    /// The button came up. Returns whether a project or agent was pressed.
    pub(super) fn release_project(&mut self) -> bool {
        let Some(press) = self.project_press.take() else {
            return false;
        };
        match press.pressed {
            Pressed::Project(key) if !press.moved => {
                if let Some(root) = self.project_root_of_key(&key) {
                    self.toggle_project(root);
                }
            }
            Pressed::Project(_) => {
                let order = self.project_order();
                self.send_order(UiEvent::ReorderProjects(order));
            }
            Pressed::Agent(_) if !press.moved => {}
            Pressed::Agent(id) => {
                if let Some(event) = self.agent_order_of(&id) {
                    self.send_order(event);
                }
            }
        }
        true
    }

    fn send_order(&self, event: UiEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }

    /// Whether a project or agent is being dragged, not just pressed.
    #[must_use]
    pub fn project_dragging(&self) -> bool {
        self.project_press.as_ref().is_some_and(|press| press.moved)
    }

    /// Keys of the projects, in the order listed.
    #[must_use]
    pub fn project_order(&self) -> Vec<String> {
        (0..self.agents.len())
            .filter(|&i| starts_project(&self.agents, i))
            .map(|i| self.agents[i].project_key().to_string())
            .collect()
    }

    /// The order of the top-level agents of the project agent `id` is in,
    /// for the session. `None` when the agent is in no project.
    fn agent_order_of(&self, id: &str) -> Option<UiEvent> {
        let index = self.agents_position(id)?;
        let project_id = self.agents[index].project_id.clone()?;
        let root = self.project_root(index)?;
        let order = self.agents[root..self.project_end(root)]
            .iter()
            .filter(|agent| agent.depth == 0)
            .map(|agent| agent.id.clone())
            .collect();
        Some(UiEvent::ReorderAgents { project_id, order })
    }

    /// Swap the project with key `key` into the place of the project drawn
    /// at screen `row`, if the pointer would still be over it afterwards.
    fn move_project_under(&mut self, key: &str, row: u16) {
        let Some(at) = self.list_row_at(row) else {
            return;
        };
        let Some(from) = self.project_root_of_key(key) else {
            return;
        };
        let Some(target) = self.project_at_row(&self.project_rows(), at) else {
            return;
        };
        if target == from {
            return;
        }
        let before: Vec<AgentSummary> = self.agents.clone();
        let selected = self.selected_row_key();
        self.move_project(from, target);
        let lands = self.project_at_row(&self.project_rows(), at) == self.project_root_of_key(key);
        if lands {
            self.restore_selected_row(selected);
            self.project_moves += 1;
        } else {
            self.agents = before;
        }
    }

    /// Swap the top-level agent `id`, with the agents under it, into the
    /// place of the top-level agent of its project drawn at screen `row`,
    /// if the pointer would still be over it afterwards.
    fn move_agent_under(&mut self, id: &str, row: u16) {
        let Some(at) = self.list_row_at(row) else {
            return;
        };
        let Some(from) = self.agents_position(id) else {
            return;
        };
        let Some(target) = self.agent_block_at_row(&self.project_rows(), at) else {
            return;
        };
        if target == from || self.project_root(target) != self.project_root(from) {
            return;
        }
        let before: Vec<AgentSummary> = self.agents.clone();
        let selected = self.selected_row_key();
        self.move_block(from, self.block_end(from), target, self.block_end(target));
        let lands = self.agent_block_at_row(&self.project_rows(), at) == self.agents_position(id);
        if lands {
            self.restore_selected_row(selected);
            self.project_moves += 1;
        } else {
            self.agents = before;
        }
    }

    /// List row under screen `row`, clamped to the list.
    fn list_row_at(&self, row: u16) -> Option<usize> {
        let list = self.areas.agent_rows?;
        let row = row.clamp(list.y, list.y.saturating_add(list.height.saturating_sub(1)));
        Some(self.agents_scroll + usize::from(row - list.y))
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

    /// Index of the top-level agent whose rows include list row `at`.
    fn agent_block_at_row(&self, rows: &[ProjectListRow], at: usize) -> Option<usize> {
        let ProjectListRow::Agent { index } = rows.get(at)? else {
            return None;
        };
        let root = self.project_root(*index)?;
        (root..=*index).rev().find(|&i| self.agents[i].depth == 0)
    }

    /// Index just past top-level agent `start` and the agents under it.
    fn block_end(&self, start: usize) -> usize {
        let end = self
            .project_root(start)
            .map_or(self.agents.len(), |root| self.project_end(root));
        (start + 1..end)
            .find(|&i| self.agents[i].depth == 0)
            .unwrap_or(end)
    }

    /// Put the project with root `from` where the one with root `to` is:
    /// after it when moving down, before it when moving up.
    fn move_project(&mut self, from: usize, to: usize) {
        let from_end = self.project_end(from);
        let to_end = self.project_end(to);
        self.move_block(from, from_end, to, to_end);
    }

    /// Put the agents `from..from_end` where `to..to_end` are: after them
    /// when moving down, before them when moving up.
    fn move_block(&mut self, from: usize, from_end: usize, to: usize, to_end: usize) {
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
            ProjectListRow::Project { root, .. } => {
                Some((self.agents[*root].project_key().to_string(), true))
            }
            ProjectListRow::Agent { index } => Some((self.agents[*index].id.clone(), false)),
            ProjectListRow::NewProject | ProjectListRow::Gap => None,
        }
    }

    fn restore_selected_row(&mut self, key: Option<(String, bool)>) {
        let Some((id, header)) = key else {
            return;
        };
        let found = self.project_rows().iter().position(|row| match *row {
            ProjectListRow::Project { root, .. } => header && self.agents[root].project_key() == id,
            ProjectListRow::Agent { index } => !header && self.agents[index].id == id,
            ProjectListRow::NewProject | ProjectListRow::Gap => false,
        });
        if let Some(row) = found {
            self.selected_agent = row;
        }
    }
}
