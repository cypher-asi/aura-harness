//! The agents panel's tabs are apps of their own. CHANNELS, PROJECTS,
//! AGENTS and ARCHIVE each put their own thing on screen, and remember it
//! while another app is in front, so coming back to one shows what it
//! was left on, with its panels:
//!
//! - PROJECTS shows the listed agent it was left on; failing that the
//!   agent on screen if listed, the one under its highlight, or the first.
//! - AGENTS shows the latest instance of the permanent agent picked in it
//!   (a click; Enter still starts a new one), or the highlighted one when
//!   none was picked, or says it has none.
//! - ARCHIVE shows the archived agent it was left on, else the one under
//!   its highlight.
//! - CHANNELS opens the channel it was left on, else the highlighted or
//!   first one (see `channels`).
//!
//! An app with nothing to show says so in the chat, and the right column
//! hides ([`App::app_empty`]). The session putting on screen an agent the
//! app in front does not show (a notification jumping to it, say) is the
//! user going to that agent: PROJECTS comes to the front with it, or
//! ARCHIVE for an archived one.
//!
//! Each app lives in its own module; what the agents panel does with a
//! key, a click, a hover or the wheel is handed to the app in front here.

mod archive;
mod channels;
mod projects;
mod roster;

pub use archive::ArchiveRow;
pub(super) use channels::ChannelState;

use super::{contains, tab_at, tab_bar::TabLayout, App};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;

/// Views of the agents panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsTab {
    /// Channels: shared conversations with a coordinator and its agents.
    Channels,
    /// Live agents: the ones the user talks to and the instances they spawned.
    #[default]
    Projects,
    /// The roster of permanent agents that can be spawned by name.
    Roster,
    /// Agents moved out of the projects list, until restored.
    Archive,
}

impl AgentsTab {
    pub const ALL: [Self; 4] = [Self::Channels, Self::Projects, Self::Roster, Self::Archive];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Channels => "CHANNELS",
            Self::Projects => "PROJECTS",
            Self::Roster => "AGENTS",
            Self::Archive => "ARCHIVE",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Channels => "CHN",
            Self::Projects => "PRJ",
            Self::Roster => "AGT",
            Self::Archive => "ARC",
        }
    }

    /// The next tab (`forward`) or the one before, wrapping, for Left / Right.
    const fn shifted(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::Channels, true) | (Self::Roster, false) => Self::Projects,
            (Self::Projects, true) | (Self::Archive, false) => Self::Roster,
            (Self::Roster, true) | (Self::Channels, false) => Self::Archive,
            (Self::Archive, true) | (Self::Projects, false) => Self::Channels,
        }
    }

    /// The tabs' cells in a header bar `width` columns wide. The renderer
    /// draws the bar with this layout and clicks are mapped back through it.
    #[must_use]
    pub fn layout(width: u16) -> TabLayout<Self> {
        TabLayout::new(Self::ALL, Self::label, Self::short_label, width)
    }
}

/// What each agent app showed, to show again when it comes back.
#[derive(Debug, Default)]
pub(super) struct AppMemory {
    /// The listed agent PROJECTS was left on
    projects: Option<String>,
    /// The archived agent ARCHIVE was left on
    archive: Option<String>,
    /// The permanent agent whose latest instance AGENTS shows
    roster: Option<String>,
}

impl App {
    /// Which list the agents panel shows.
    #[must_use]
    pub const fn agents_tab(&self) -> AgentsTab {
        self.agents_tab
    }

    /// Record where the agents panel's header bar was drawn this frame.
    pub fn set_agent_tabs_area(&mut self, area: Rect) {
        self.areas.agent_tabs = Some(area);
    }

    /// Left shows the tab before, Right the one after.
    pub(super) fn shift_agents_tab(&mut self, key: KeyCode) {
        self.show_agents_tab(self.agents_tab.shifted(key == KeyCode::Right));
    }

    /// Clicking a tab shows it; clicking a row acts on the visible list.
    pub(super) fn click_agents_panel(&mut self, column: u16, row: u16) {
        if contains(self.areas.agent_tabs, column, row) {
            if let Some(tab) = tab_at(self.areas.agent_tabs, column, row, AgentsTab::layout) {
                self.show_agents_tab(tab);
            }
            return;
        }
        match self.agents_tab {
            AgentsTab::Channels => self.click_channel_row(column, row),
            AgentsTab::Projects => self.click_agent_row(column, row),
            AgentsTab::Roster => self.click_definition_row(column, row),
            AgentsTab::Archive => self.click_archived_row(column, row),
        }
    }

    /// Show `tab`, as the user picking it: the app it replaces is
    /// remembered, and `tab` puts back what it showed.
    pub(super) fn show_agents_tab(&mut self, tab: AgentsTab) {
        if !self.switch_app(tab) {
            return;
        }
        match tab {
            AgentsTab::Channels => {
                if let Some(id) = self.channel_to_show() {
                    self.open_channel_by_id(&id);
                }
            }
            AgentsTab::Projects => {
                if let Some(id) = self.projects_app_agent() {
                    self.switch_on_screen(&id);
                }
            }
            AgentsTab::Roster => {
                if let Some(name) = self.roster_app_definition() {
                    self.open_definition(&name);
                }
            }
            AgentsTab::Archive => {
                if let Some(id) = self.archive_app_agent() {
                    self.switch_on_screen(&id);
                }
            }
        }
    }

    /// Bring `tab` to the front, remembering what the app it replaces
    /// showed, without putting anything on screen: for callers whose next
    /// agent is on its way from the session. Returns whether the app
    /// changed.
    pub(super) fn switch_app(&mut self, tab: AgentsTab) -> bool {
        let was = self.agents_tab;
        self.set_agents_tab(tab);
        if was == tab {
            return false;
        }
        self.remember_app(was);
        if was == AgentsTab::Channels {
            self.leave_channel_app();
        }
        if tab == AgentsTab::Channels {
            self.enter_channel_app();
        }
        true
    }

    /// Make `tab` the one shown, without entering or leaving any app: for
    /// callers that do that part themselves.
    pub(super) fn set_agents_tab(&mut self, tab: AgentsTab) {
        self.agents_tab = tab;
        self.pending_delete = None;
        self.pending_remove_project = None;
        self.pending_archive_channel = None;
    }

    /// Note what `app`, being left, has on screen, to show it again.
    fn remember_app(&mut self, app: AgentsTab) {
        let on_screen = self.active_agent_id.clone();
        match app {
            AgentsTab::Projects if self.is_listed(&on_screen) => {
                self.apps.projects = Some(on_screen);
            }
            AgentsTab::Archive if self.is_archived_agent(&on_screen) => {
                self.apps.archive = Some(on_screen);
            }
            _ => {}
        }
    }

    fn is_listed(&self, agent_id: &str) -> bool {
        self.agents.iter().any(|agent| agent.id == agent_id)
    }

    fn is_archived_agent(&self, agent_id: &str) -> bool {
        self.archived.iter().any(|agent| agent.id == agent_id)
    }

    /// The agent PROJECTS shows on coming back.
    fn projects_app_agent(&self) -> Option<String> {
        let listed = |id: &&str| self.is_listed(id);
        self.apps
            .projects
            .as_deref()
            .filter(listed)
            .or_else(|| Some(self.active_agent_id.as_str()).filter(listed))
            .map(str::to_string)
            .or_else(|| self.highlighted_agent())
            .or_else(|| self.agents.first().map(|agent| agent.id.clone()))
    }

    /// The agent ARCHIVE shows on coming back.
    fn archive_app_agent(&self) -> Option<String> {
        let archived = |id: &&str| self.is_archived_agent(id);
        self.apps
            .archive
            .as_deref()
            .filter(archived)
            .or_else(|| Some(self.active_agent_id.as_str()).filter(archived))
            .map(str::to_string)
            .or_else(|| {
                self.archived
                    .get(self.selected_archived)
                    .map(|agent| agent.id.clone())
            })
    }

    /// The permanent agent AGENTS shows on coming back: the one picked
    /// last, while it exists, else the highlighted one.
    fn roster_app_definition(&self) -> Option<String> {
        self.apps
            .roster
            .clone()
            .filter(|name| self.definitions.iter().any(|d| d.name == *name))
            .or_else(|| self.selected_definition_name())
    }

    /// Latest instance of the permanent agent `name`, listed or archived:
    /// the one that last did something, a listed one on a tie.
    #[must_use]
    pub(crate) fn latest_instance(&self, name: &str) -> Option<&str> {
        let listed = self.agents.iter().map(|agent| (agent, true));
        let archived = self.archived.iter().map(|agent| (agent, false));
        listed
            .chain(archived)
            .filter(|(agent, _)| agent.definition.as_deref() == Some(name))
            .max_by_key(|(agent, listed)| (agent.last_active_ms, *listed))
            .map(|(agent, _)| agent.id.as_str())
    }

    /// Show the latest instance of the permanent agent `name` in AGENTS,
    /// or that it has none.
    pub(super) fn open_definition(&mut self, name: &str) {
        self.apps.roster = Some(name.to_string());
        if let Some(id) = self.latest_instance(name).map(str::to_string) {
            self.switch_on_screen(&id);
        }
    }

    /// Whether the agent on screen is an instance of the permanent agent
    /// AGENTS shows.
    fn roster_shows_instance(&self) -> bool {
        let Some(name) = self.apps.roster.as_deref() else {
            return false;
        };
        self.agents.iter().chain(self.archived.iter()).any(|agent| {
            agent.id == self.active_agent_id && agent.definition.as_deref() == Some(name)
        })
    }

    /// Whether the app in front has nothing to show: no channel open, no
    /// instance of the permanent agent picked, no archived agent opened.
    /// The chat says so, and the right column hides.
    #[must_use]
    pub(crate) fn app_empty(&self) -> bool {
        match self.agents_tab {
            AgentsTab::Channels => !self.channel_open(),
            AgentsTab::Projects => false,
            AgentsTab::Roster => !self.roster_shows_instance(),
            AgentsTab::Archive => !self.is_archived_agent(&self.active_agent_id),
        }
    }

    /// What the chat says while [`Self::app_empty`].
    #[must_use]
    pub(crate) fn app_empty_message(&self) -> String {
        match self.agents_tab {
            AgentsTab::Channels if self.channels().is_empty() => {
                "No channels yet. Start one with + New channel on the left.".to_string()
            }
            AgentsTab::Channels => "Pick a channel on the left to open its chat.".to_string(),
            AgentsTab::Roster if self.definitions.is_empty() => {
                "No permanent agents yet. Make one with + New agent on the left.".to_string()
            }
            AgentsTab::Roster => match self.apps.roster.as_deref() {
                Some(name) if self.latest_instance(name).is_none() => {
                    format!("No {name} agent has run yet. Enter on it starts one.")
                }
                _ => "Pick a permanent agent on the left to open its latest chat.".to_string(),
            },
            AgentsTab::Archive if self.archived.is_empty() => "Nothing is archived.".to_string(),
            AgentsTab::Archive => {
                "Pick an archived agent on the left to open its chat.".to_string()
            }
            AgentsTab::Projects => String::new(),
        }
    }

    /// What there is to pick in the app in front, as "Pick …" says it.
    fn app_pick(&self) -> &'static str {
        match self.agents_tab {
            AgentsTab::Channels => "a channel",
            AgentsTab::Roster => "a permanent agent",
            AgentsTab::Archive => "an archived agent",
            AgentsTab::Projects => "an agent",
        }
    }

    /// What the empty input says: who a message goes to in the channel
    /// app, or what to pick while the app in front shows nothing. `None`
    /// otherwise.
    #[must_use]
    pub(crate) fn app_placeholder(&self) -> Option<String> {
        if self.channel_app() {
            return self.channel_placeholder();
        }
        self.app_empty()
            .then(|| format!("Pick {}", self.app_pick()))
    }

    /// Why a message cannot be sent while the app shows nothing.
    pub(super) fn app_send_refusal(&self) -> String {
        format!("Pick {} to send a message", self.app_pick())
    }

    /// The session put the agent on screen; `switched` when it was not on
    /// screen before. See the module docs.
    pub(super) fn follow_active_agent(&mut self, switched: bool) {
        let shown = match self.agents_tab {
            AgentsTab::Channels => {
                self.follow_active_agent_out_of_channel();
                return;
            }
            AgentsTab::Projects => return,
            AgentsTab::Roster => self.roster_shows_instance(),
            AgentsTab::Archive => self.is_archived_agent(&self.active_agent_id),
        };
        if switched && !shown {
            self.follow_into_agent_app();
        }
    }

    /// Bring to the front the app that lists the agent on screen: ARCHIVE
    /// for an archived one, else PROJECTS. It stays on screen.
    pub(super) fn follow_into_agent_app(&mut self) {
        let tab = if self.is_archived_agent(&self.active_agent_id) {
            AgentsTab::Archive
        } else {
            AgentsTab::Projects
        };
        self.switch_app(tab);
    }
}
