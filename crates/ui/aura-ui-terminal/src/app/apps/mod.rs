//! The agents panel's tabs are apps of their own. CHANNELS, PROJECTS,
//! AGENTS, BENCH and ARCHIVE each put their own thing on screen, and
//! remember it while another app is in front, so coming back to one shows
//! what it was left on, with its panels:
//!
//! - PROJECTS shows the listed agent it was left on; failing that the
//!   agent on screen if listed, the one under its highlight, or the first.
//! - AGENTS shows the latest instance of the permanent agent picked in it
//!   (a click; Enter still starts a new one), or the highlighted one when
//!   none was picked, or says it has none.
//! - BENCH shows the benchmark, the new-benchmark form or the agent of a
//!   benchmark it was left on, else the highlighted benchmark, else the
//!   newest (see `bench`).
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
//! CHANNELS shows none of the agent's chat tabs, nor does BENCH but for
//! an agent of a benchmark: coming to one of them turns the chat panel to
//! CHAT, and going back to an app with the tabs, or to an agent in BENCH,
//! gives the agent that was on screen the tab it was on.
//!
//! Each app lives in its own module; what the agents panel does with a
//! key, a click, a hover or the wheel is handed to the app in front here.

mod archive;
mod bench;
mod channels;
mod projects;
mod roster;

pub use archive::ArchiveRow;
pub use bench::{
    BenchAgent, BenchMode, BenchRow, BenchState, FormField, WorkChoice, AUTO_CLASSIFIER,
    BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};
pub(super) use channels::ChannelState;

use super::{contains, tab_at, tab_bar::TabLayout, App, ChatTab, HoverTarget, NotificationType};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent};
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
    /// Benchmarks: the same work on two arms (Gate or a model), their
    /// cost and how well they did.
    Bench,
    /// Agents moved out of the projects list, until restored.
    Archive,
}

impl AgentsTab {
    pub const ALL: [Self; 5] = [
        Self::Channels,
        Self::Projects,
        Self::Roster,
        Self::Bench,
        Self::Archive,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Channels => "CHANNELS",
            Self::Projects => "PROJECTS",
            Self::Roster => "AGENTS",
            Self::Bench => "BENCH",
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
            Self::Bench => "BEN",
            Self::Archive => "ARC",
        }
    }

    /// The next tab (`forward`) or the one before, wrapping, for Left / Right.
    const fn shifted(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::Channels, true) | (Self::Roster, false) => Self::Projects,
            (Self::Projects, true) | (Self::Bench, false) => Self::Roster,
            (Self::Roster, true) | (Self::Archive, false) => Self::Bench,
            (Self::Bench, true) | (Self::Channels, false) => Self::Archive,
            (Self::Archive, true) | (Self::Projects, false) => Self::Channels,
        }
    }

    /// The tab Alt+`digit` brings to the front: 1 to 5, in header order.
    #[must_use]
    pub fn numbered(digit: char) -> Option<Self> {
        let index = digit.to_digit(10)?.checked_sub(1)?;
        Self::ALL.get(usize::try_from(index).ok()?).copied()
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
    /// The channel CHANNELS has open, or had when it was left
    channel: Option<String>,
    /// The agent on screen when the user went from an app with the
    /// agent's chat tabs to one without, or in BENCH from an agent to a
    /// benchmark, and the tab it was on, which turned to CHAT: given back
    /// on going to an app or an agent with them
    chat_tab_before: Option<(String, ChatTab)>,
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
            AgentsTab::Bench => self.click_bench_row(column, row),
            AgentsTab::Archive => self.click_archived_row(column, row),
        }
    }

    /// The app in front's own keys, taken before the input sees them
    /// while it is empty: letters, Space, Delete. Ctrl and Alt keys
    /// (`word_motion`) stay the input's, except Delete in PROJECTS.
    /// Returns whether the app took `key`.
    pub(super) fn agents_panel_shortcut(&mut self, key: KeyEvent, word_motion: bool) -> bool {
        match self.agents_tab {
            AgentsTab::Projects => self.projects_shortcut(key, word_motion),
            _ if word_motion => false,
            AgentsTab::Channels => self.handle_channels_key(key),
            AgentsTab::Roster => self.handle_definitions_shortcut(key),
            AgentsTab::Bench => self.handle_bench_list_key(key),
            AgentsTab::Archive => self.handle_archive_key(key),
        }
    }

    /// A key for the agents panel the input did not take.
    pub(super) fn handle_agents_panel_key(&mut self, key: KeyEvent) -> KeyResult {
        match self.agents_tab {
            AgentsTab::Projects => return self.handle_swarm_key(key),
            AgentsTab::Channels => {
                self.handle_channels_key(key);
            }
            AgentsTab::Roster => self.handle_definitions_key(key),
            AgentsTab::Bench => {
                self.handle_bench_list_key(key);
            }
            AgentsTab::Archive => {
                self.handle_archive_key(key);
            }
        }
        KeyResult::continue_running()
    }

    /// The wheel over the agents panel, `delta` rows: PROJECTS scrolls its
    /// list, the others move their highlight.
    pub(super) fn scroll_agents_panel(&mut self, delta: isize) {
        match self.agents_tab {
            AgentsTab::Channels => self.move_channel_highlight(delta),
            AgentsTab::Projects => self.scroll_project_rows(delta),
            AgentsTab::Roster => {
                self.select_definition_row(self.definitions.selected.saturating_add_signed(delta));
            }
            AgentsTab::Bench => self.move_bench_highlight(delta),
            AgentsTab::Archive => self.move_archived_by(delta),
        }
    }

    /// Rows one wheel notch moves in the agents panel: one for the apps
    /// whose wheel moves the highlight, three for scrolling PROJECTS.
    pub(super) const fn agents_panel_wheel_step(&self) -> usize {
        match self.agents_tab {
            AgentsTab::Projects => 3,
            _ => 1,
        }
    }

    /// What the pointer is over in the app in front's list.
    pub(super) fn hovered_agents_panel(&self, column: u16, row: u16) -> Option<HoverTarget> {
        match self.agents_tab {
            AgentsTab::Projects => self.hovered_project_row(column, row),
            AgentsTab::Channels => self
                .channel_row_at(column, row)
                .map(HoverTarget::ChannelRow),
            AgentsTab::Roster => self
                .definition_row_at(column, row)
                .map(HoverTarget::DefinitionRow),
            AgentsTab::Bench => self.bench_row_at(column, row).map(HoverTarget::BenchRow),
            AgentsTab::Archive => self
                .archived_row_at(column, row)
                .map(HoverTarget::ArchivedRow),
        }
    }

    /// A right click on the agents panel: the menu of the row under the
    /// pointer in the app in front, if it has one. Returns whether a menu
    /// opened.
    pub(super) fn open_context_menu(&mut self, column: u16, row: u16) -> bool {
        self.context_menu = None;
        match self.agents_tab {
            AgentsTab::Projects => self.open_project_menu(column, row),
            AgentsTab::Archive => self.open_archive_menu(column, row),
            AgentsTab::Channels => self.open_channel_menu(column, row),
            AgentsTab::Roster | AgentsTab::Bench => false,
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
            AgentsTab::Bench => self.show_bench_app(),
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
        match was {
            AgentsTab::Channels => self.leave_channel_app(),
            AgentsTab::Bench => self.leave_bench_app(),
            _ => {}
        }
        match (self.has_agent_tabs(was), self.has_agent_tabs(tab)) {
            (true, false) => self.put_chat_tabs_away(),
            (false, true) => self.give_chat_tabs_back(),
            _ => {}
        }
        if tab == AgentsTab::Bench {
            self.enter_bench_app();
        }
        true
    }

    /// Whether `tab` shows an agent with all its chat tabs. CHANNELS has
    /// only a chat, and BENCH shows benchmarks, but for an agent of one.
    fn has_agent_tabs(&self, tab: AgentsTab) -> bool {
        match tab {
            AgentsTab::Channels => false,
            AgentsTab::Bench => self.bench.shown_agent().is_some(),
            AgentsTab::Projects | AgentsTab::Roster | AgentsTab::Archive => true,
        }
    }

    /// Put the agent's chat tabs away for an app, or a view of BENCH,
    /// with none: the chat panel turns to CHAT, and the agent on screen
    /// and the tab it was on are kept to give back.
    pub(super) fn put_chat_tabs_away(&mut self) {
        self.apps.chat_tab_before = (!self.active_agent_id.is_empty())
            .then(|| (self.active_agent_id.clone(), self.chat_tab()));
        self.close_detail();
        if self.chat_tab() != ChatTab::Chat {
            self.show_chat_tab(ChatTab::Chat);
        }
    }

    /// Give the tab [`Self::put_chat_tabs_away`] kept back to its agent.
    pub(super) fn give_chat_tabs_back(&mut self) {
        if let Some((agent, chat_tab)) = self.apps.chat_tab_before.take() {
            self.give_chat_tab_back(&agent, chat_tab);
        }
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
            AgentsTab::Channels => {
                if let Some(open) = self.open_channel().map(|channel| channel.id.clone()) {
                    self.apps.channel = Some(open);
                }
            }
            _ => {}
        }
    }

    fn is_listed(&self, agent_id: &str) -> bool {
        self.agents.iter().any(|agent| agent.id == agent_id)
    }

    fn is_archived_agent(&self, agent_id: &str) -> bool {
        self.archived.items.iter().any(|agent| agent.id == agent_id)
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
            .or_else(|| self.archived.selected_item().map(|agent| agent.id.clone()))
    }

    /// The permanent agent AGENTS shows on coming back: the one picked
    /// last, while it exists, else the highlighted one.
    fn roster_app_definition(&self) -> Option<String> {
        self.apps
            .roster
            .clone()
            .filter(|name| self.definitions.items.iter().any(|d| d.name == *name))
            .or_else(|| self.selected_definition_name())
    }

    /// Latest instance of the permanent agent `name`, listed or archived:
    /// the one that last did something, a listed one on a tie.
    #[must_use]
    pub(crate) fn latest_instance(&self, name: &str) -> Option<&str> {
        let listed = self.agents.iter().map(|agent| (agent, true));
        let archived = self.archived.items.iter().map(|agent| (agent, false));
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
        self.agents
            .iter()
            .chain(self.archived.items.iter())
            .any(|agent| {
                agent.id == self.active_agent_id && agent.definition.as_deref() == Some(name)
            })
    }

    /// Whether the app in front has nothing to show: no channel open, no
    /// instance of the permanent agent picked, no benchmark or form, no
    /// archived agent opened. The chat says so, and the right column
    /// hides.
    #[must_use]
    pub(crate) fn app_empty(&self) -> bool {
        match self.agents_tab {
            AgentsTab::Channels => !self.channel_open(),
            AgentsTab::Projects => false,
            AgentsTab::Roster => !self.roster_shows_instance(),
            AgentsTab::Bench => !self.bench.showing(),
            AgentsTab::Archive => !self.is_archived_agent(&self.active_agent_id),
        }
    }

    /// Whether no agent is on screen: BENCH shows a benchmark or the form,
    /// and an app showing nothing shows no agent either. The agent's side
    /// column, queue, stop button, todos and benchmark progress stay away.
    #[must_use]
    pub(crate) fn no_agent_on_screen(&self) -> bool {
        self.bench_draws_center() || self.app_empty()
    }

    /// Whether the app in front shows only CHAT of the chat panel's tabs:
    /// CHANNELS, and BENCH but for an agent of a benchmark.
    #[must_use]
    pub(crate) fn chat_only(&self) -> bool {
        !self.has_agent_tabs(self.agents_tab)
    }

    /// Whether the agent panels (`/kernel`, `/files`, `/inbox`, ...) stay
    /// off screen: CHANNELS has its own, and BENCH shows no agent but for
    /// one of a benchmark's.
    pub(super) fn agent_panels_hidden(&self) -> bool {
        self.channel_app() || self.bench_draws_center()
    }

    /// Say why the agent panels are not shown ([`Self::agent_panels_hidden`]).
    pub(super) fn notify_no_agent_panels(&mut self) {
        if self.bench_app() {
            self.notification = Some((
                "BENCH shows no agent here; pick one of a benchmark's for agent panels".to_string(),
                NotificationType::Warning,
            ));
        } else {
            self.notify_channel_owns_column();
        }
    }

    /// The chat panel's title in an app without the agent's tabs, and
    /// what it says beside it: the open channel's `#slug` and title, or
    /// the benchmark BENCH shows and its title. `None` elsewhere, or with
    /// nothing open.
    #[must_use]
    pub(crate) fn app_chat_title(&self) -> Option<(String, String)> {
        match self.agents_tab {
            AgentsTab::Channels => self.channel_chat_title(),
            AgentsTab::Bench => self.bench_chat_title(),
            _ => None,
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
            AgentsTab::Bench => self.bench_empty_message().to_string(),
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
            AgentsTab::Bench => "a benchmark",
            AgentsTab::Archive => "an archived agent",
            AgentsTab::Projects => "an agent",
        }
    }

    /// What the empty input says: who a message goes to in the channel
    /// app, what BENCH takes while it shows no agent, or what to pick
    /// while the app in front shows nothing. `None` otherwise.
    #[must_use]
    pub(crate) fn app_placeholder(&self) -> Option<String> {
        if self.channel_app() {
            return self.channel_placeholder();
        }
        if self.bench_draws_center() {
            return Some(self.bench_placeholder());
        }
        self.app_empty()
            .then(|| format!("Pick {}", self.app_pick()))
    }

    /// Why a message cannot be sent: a benchmark has no chat, and an app
    /// showing nothing has nobody to send it to.
    pub(super) fn app_send_refusal(&self) -> String {
        if self.bench_draws_center() {
            return "BENCH has no chat here; pick one of a benchmark's agents to message it"
                .to_string();
        }
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
            // BENCH gave way to an agent it does not show before its tab
            // came back (`apply_active_agent`).
            AgentsTab::Projects | AgentsTab::Bench => return,
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
