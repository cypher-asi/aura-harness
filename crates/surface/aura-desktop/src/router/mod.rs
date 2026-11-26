//! The window's tabs, and how one agent session serves all of them.
//!
//! A tab is a workspace: it holds some of the session's projects (by
//! project key) and shows only those, in its own copy
//! of the terminal UI. The session knows nothing of tabs. It has one
//! event loop, one command stream and one "active" agent, whose Record
//! panel, sandbox and notifications it sends without an address, so the
//! agent ties a command to a tab:
//!
//! - The agent list is cut down to each tab's projects, and commands
//!   addressed to an agent go to the tab that holds it.
//! - Lists that aren't about projects (models, definitions, the Archive,
//!   channels) go to every tab. Archived agents and channel members are
//!   in no tab's projects: they show in the tab that opened them.
//! - Anything else goes to the tab holding the session's active agent.
//! - Before a tab's events are sent, the session is asked to make that
//!   tab's agent the active one, so the answers come back to it.
//!
//! Invariant: every project is in exactly one tab. A new one joins the
//! tab that asked for it; `+` asks for one in a tab of its own. A project
//! moves whole from one tab to another (a tab's right-click menu asks,
//! see [`Tabs::forward_events`]), never split, and a tab left with no
//! project closes.
//!
//! Each tab's UI is told the tabs' titles and its own place among them
//! (`set_workspaces`) whenever they may have changed, so the indices its
//! menu offers are those of the tabs as they are.

mod arrangement;
mod events;
mod placement;
mod routing;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
mod workspace;

pub use arrangement::{arrangement_file, load_arrangement, save_arrangement, Arrangement};

use crate::tab::{Settings, Tab};
use aura_ui::events::{
    starts_project, AgentSummary, ProcessSummary, ProjectSummary, UiCommand, UiEvent,
};
use tokio::sync::mpsc;

/// Most commands kept back while waiting for an agent to be listed.
const MAX_HELD: usize = 4096;

pub struct Tabs {
    /// In the order they were opened; never empty.
    tabs: Vec<Tab>,
    front: usize,
    session: mpsc::Sender<UiEvent>,
    settings: Settings,
    next_id: u64,
    /// Every agent of the session, as last listed.
    agents: Vec<AgentSummary>,
    /// Every listed project of the session, as last listed. Those with
    /// every agent archived are in no run of `agents`.
    projects: Vec<ProjectSummary>,
    /// Every process of the session, as last listed.
    processes: Vec<ProcessSummary>,
    /// The latest of each list all tabs show whole, for tabs opened later.
    lists: [Option<UiCommand>; 5],
    /// The session's active agent, as it last reported.
    session_active: String,
    /// Agent the session was asked to make active and hasn't confirmed.
    focus_sent: Option<String>,
    /// `+` asked for a project and it is yet to be listed.
    opening: bool,
    /// Tab, by id, whose folder browser `+` opened: the project picked
    /// there gets a tab of its own.
    picking: Option<u64>,
    /// Tab, by id, whose UI last asked for a new project or agent.
    asker: Option<u64>,
    /// An agent the session switched to before listing it, and what it
    /// sent since: where that goes depends on the tab the agent joins.
    unlisted: Option<(String, Vec<UiCommand>)>,
    /// The tabs of the last run, until the first list brings them back.
    restore: Option<Arrangement>,
}

impl Tabs {
    /// One tab, sending what the tabs ask for to `session`. The first
    /// agent list splits the projects over tabs as in `restore`.
    pub fn new(session: mpsc::Sender<UiEvent>, settings: Settings, restore: Arrangement) -> Self {
        Self {
            tabs: vec![Tab::new(1, settings)],
            front: 0,
            session,
            settings,
            next_id: 2,
            agents: Vec::new(),
            projects: Vec::new(),
            processes: Vec::new(),
            lists: [None, None, None, None, None],
            session_active: String::new(),
            focus_sent: None,
            opening: false,
            picking: None,
            asker: None,
            unlisted: None,
            restore: Some(restore),
        }
    }

    pub const fn settings(&self) -> Settings {
        self.settings
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub const fn front_index(&self) -> usize {
        self.front
    }

    pub fn front(&self) -> &Tab {
        &self.tabs[self.front]
    }

    pub fn front_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.front]
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    pub fn by_id_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    /// Which projects are in which tab.
    pub fn arrangement(&self) -> Arrangement {
        self.tabs.iter().map(|tab| tab.projects.clone()).collect()
    }

    /// Key of the project `agent` is in.
    fn root_of(&self, agent: &str) -> Option<&str> {
        let mut root = None;
        for (index, listed) in self.agents.iter().enumerate() {
            if starts_project(&self.agents, index) {
                root = Some(listed.project_key());
            }
            if listed.id == agent {
                return root;
            }
        }
        None
    }

    /// The tab holding `agent`, once the session has listed it.
    fn owner(&self, agent: &str) -> Option<usize> {
        let root = self.root_of(agent)?;
        self.tabs
            .iter()
            .position(|tab| tab.projects.iter().any(|project| project == root))
    }

    /// Whether `agent` is in the Archive, as last listed.
    fn is_archived(&self, agent: &str) -> bool {
        matches!(
            &self.lists[3],
            Some(UiCommand::SetArchivedAgents(rows)) if rows.iter().any(|row| row.id == agent)
        )
    }

    /// Whether `agent` coordinates a channel or is a member of one, as
    /// last listed. The agent list leaves those out.
    fn is_channel_member(&self, agent: &str) -> bool {
        matches!(
            &self.lists[4],
            Some(UiCommand::SetChannels(channels)) if channels.iter().any(|channel| {
                channel.coordinator_id == agent
                    || channel.members.iter().any(|member| member.id == agent)
            })
        )
    }

    /// The tab that gets what the session sends without an address: the
    /// one showing the session's active agent. That is the tab holding it
    /// once it shows it; until then, as right after a project moved, the
    /// tab still showing it, so what the session sends lands under that
    /// agent's chat rather than another's.
    fn active_index(&self) -> usize {
        let agent = self.session_active.as_str();
        let owner = self.owner(agent);
        let shows = |index: &usize| {
            self.tabs
                .get(*index)
                .is_some_and(|tab| tab.agent() == agent)
        };
        let showing = || {
            // Tabs that have shown nothing are not showing "no agent".
            (!agent.is_empty())
                .then(|| self.tabs.iter().position(|tab| tab.agent() == agent))
                .flatten()
        };
        owner
            .filter(shows)
            .or_else(showing)
            .or(owner)
            .unwrap_or(self.front)
    }

    /// The agent the tab at `index` speaks for: the one it shows, or none
    /// (empty) when it shows none or one whose project moved to another
    /// tab. An agent not listed, new or archived, still counts.
    fn shown_by(&self, index: usize) -> &str {
        let Some(tab) = self.tabs.get(index) else {
            return "";
        };
        let agent = tab.agent();
        match self.owner(agent) {
            Some(owner) if owner != index => "",
            _ => agent,
        }
    }
}
