//! Whose items the side panel's PROCESSES, TRACE and TASKS tabs list: the
//! active agent's, with the agents it spawned, or every agent's in its
//! project. The TASKS tab lists the plans those agents' tasks work on.
//! The `*` button at the right end of the panel's header bar switches
//! every tab at once. An open channel's TASKS lists the whole project's,
//! whatever the button says.

use std::collections::HashSet;

use super::tasks::RowKey;
use super::trace_feed::FeedPlace;
use super::{contains, App};
use crate::events::{starts_project, ProcessSummary};
use ratatui::layout::Rect;

/// Whose items the side panel's lists show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideScope {
    /// The active agent and the agents it spawned
    #[default]
    Agent,
    /// Every agent in the active agent's project
    Project,
}

impl SideScope {
    /// Label of the button that switches to [`Self::Project`] and back.
    pub const BUTTON: &'static str = "*";

    const fn toggled(self) -> Self {
        match self {
            Self::Agent => Self::Project,
            Self::Project => Self::Agent,
        }
    }
}

/// Hex ids of the agents whose items a list shows; `None` shows every item.
pub type AgentFilter = Option<HashSet<String>>;

/// Where the PROCESSES, TRACE and TASKS lists were while an agent was on
/// screen: the row each highlighted, and the first row each showed. They
/// are filtered for another agent while it is away, so its rows come back
/// by what they are, not by index.
#[derive(Debug, Clone)]
pub(super) struct ListPlaces {
    process: Option<String>,
    processes_scroll: usize,
    step: FeedPlace,
    trace_scroll: usize,
    task: Option<RowKey>,
    tasks_scroll: usize,
}

/// Whether `process` belongs to an agent `agents` lets through: the one
/// that started it, the one it runs, or one above them. Rows of no agent
/// are always shown.
pub(super) fn process_in_scope(process: &ProcessSummary, agents: &HashSet<String>) -> bool {
    if process.owner_agent_id.is_empty() && process.jump_agent_id.is_empty() {
        return true;
    }
    [&process.owner_agent_id, &process.jump_agent_id]
        .into_iter()
        .chain(&process.parent_chain)
        .any(|id| agents.contains(id))
}

impl App {
    /// Whose items the side panel's lists show.
    #[must_use]
    pub const fn side_scope(&self) -> SideScope {
        self.side.scope
    }

    /// Show the whole project's items in the side panel's lists, or only
    /// the active agent's again.
    pub fn toggle_side_scope(&mut self) {
        self.side.scope = self.side.scope.toggled();
        self.refresh_side_scope();
    }

    /// Record where the side panel's `*` button was drawn this frame.
    pub fn set_side_scope_area(&mut self, area: Option<Rect>) {
        self.areas.side_scope = area;
    }

    pub(super) fn side_scope_at(&self, column: u16, row: u16) -> bool {
        contains(self.areas.side_scope, column, row)
    }

    /// The agents whose items the lists show: the active agent and the
    /// agents under it in the projects list, or every agent listed with
    /// it in its project. With no active agent everything is shown.
    fn scope_agents(&self) -> AgentFilter {
        if self.active_agent_id.is_empty() {
            return None;
        }
        let Some(index) = self
            .agents
            .iter()
            .position(|a| a.id == self.active_agent_id)
        else {
            return match self.side.scope {
                SideScope::Agent => Some(HashSet::from([self.active_agent_id.clone()])),
                SideScope::Project => None,
            };
        };
        let agents = &self.agents;
        let range = match self.side.scope {
            SideScope::Agent => {
                let depth = agents[index].depth;
                let end = agents[index + 1..]
                    .iter()
                    .position(|agent| agent.depth <= depth)
                    .map_or(agents.len(), |offset| index + 1 + offset);
                index..end
            }
            SideScope::Project => {
                let start = (0..=index)
                    .rev()
                    .find(|&i| starts_project(agents, i))
                    .unwrap_or(0);
                let end = (index + 1..agents.len())
                    .find(|&i| starts_project(agents, i))
                    .unwrap_or(agents.len());
                start..end
            }
        };
        Some(agents[range].iter().map(|agent| agent.id.clone()).collect())
    }

    /// Filter every side-panel list for the scope, the active agent and
    /// the agents listed now, and list the members of the active agent's
    /// run again. While a channel is open its TASKS lists every plan and
    /// task the session sends, which are the whole project's: the agent
    /// on screen is one of the channel's, working in its folder.
    pub(super) fn refresh_side_scope(&mut self) {
        let agents = self.scope_agents();
        self.trace_feed.set_filter(agents.clone());
        let tasks = if self.channel_open() {
            None
        } else {
            agents.clone()
        };
        self.task_list.set_filter(tasks);
        self.side.process_filter = agents;
        self.reshow_processes();
        self.refresh_members();
    }

    /// Where the side panel's lists are now, for the agent on screen.
    pub(super) fn list_places(&self) -> ListPlaces {
        ListPlaces {
            process: self.selected_process_data().map(|p| p.id.clone()),
            processes_scroll: self.side.processes.scroll,
            step: self.trace_feed.place(),
            trace_scroll: self.side.trace_scroll,
            task: self.task_list.place(),
            tasks_scroll: self.side.tasks_scroll,
        }
    }

    /// Put the side panel's lists back where `places` has them, once they
    /// are filtered for the agent they were left with. A row gone since
    /// leaves its list's highlight where the filter put it.
    pub(super) fn put_back_list_places(&mut self, places: ListPlaces) {
        if let Some(at) = places
            .process
            .and_then(|id| self.side.processes.items.iter().position(|p| p.id == id))
        {
            self.side.processes.selected = at;
        }
        self.side.processes.scroll = places.processes_scroll;
        self.trace_feed.put_back(&places.step);
        self.side.trace_scroll = places.trace_scroll;
        self.task_list.put_back(places.task.as_ref());
        self.side.tasks_scroll = places.tasks_scroll;
    }
}

#[cfg(test)]
mod tests;
