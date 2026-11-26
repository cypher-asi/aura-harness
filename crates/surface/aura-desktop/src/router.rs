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

use crate::tab::{Settings, Tab};
use aura_ui::events::{
    empty_projects, starts_project, AgentSummary, ProcessSummary, ProjectSummary, UiCommand,
    UiEvent,
};
use aura_ui_terminal::WorkspaceTarget;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// Most commands kept back while waiting for an agent to be listed.
const MAX_HELD: usize = 4096;

/// The root agents of the projects in each tab, in tab order.
pub type Arrangement = Vec<Vec<String>>;

/// Where the arrangement is kept: beside the agent registry, in the data
/// directory the session resolves the same way from `dir`.
pub fn arrangement_file(dir: Option<&Path>) -> PathBuf {
    dir.map(Path::to_path_buf)
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"))
        .join("desktop_tabs")
}

/// The arrangement saved at `path`: a line per tab, of ids separated by
/// spaces. Empty when there is none to read.
pub fn load_arrangement(path: &Path) -> Arrangement {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|line| line.split_whitespace().map(str::to_string).collect())
        .filter(|tab: &Vec<String>| !tab.is_empty())
        .collect()
}

/// Write `arrangement` to `path` for [`load_arrangement`].
///
/// # Errors
///
/// Fails when the file can't be written.
pub fn save_arrangement(path: &Path, arrangement: &Arrangement) -> std::io::Result<()> {
    let lines: Vec<String> = arrangement.iter().map(|tab| tab.join(" ")).collect();
    std::fs::write(path, lines.join("\n") + "\n")
}

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

    /// Apply a command from the session to the tabs it is for.
    pub fn apply(&mut self, command: UiCommand) {
        match command {
            UiCommand::SetAgents(agents) => self.listed(agents),
            command => self.route(command),
        }
    }

    fn route(&mut self, command: UiCommand) {
        match &mut self.unlisted {
            Some((_, held)) if held.len() < MAX_HELD => held.push(command),
            Some(_) => {
                self.release();
                self.deliver(command);
            }
            None => self.deliver(command),
        }
    }

    fn deliver(&mut self, command: UiCommand) {
        if let UiCommand::ForAgent {
            agent_id,
            command: inner,
        } = &command
        {
            if let UiCommand::SetMode(mode) = inner.as_ref() {
                // Lists sent on from this copy keep the agent's plan mark.
                if let Some(agent) = self.agents.iter_mut().find(|a| a.id == *agent_id) {
                    agent.plan_mode = *mode == aura_ui_terminal::PermissionMode::Plan;
                }
            }
            match self.owner(agent_id) {
                Some(index) => self.tabs[index].tui.process_command(command),
                // Not listed yet: a tab's UI keeps it until it is.
                None => self.broadcast(command),
            }
            return;
        }
        match command {
            UiCommand::SetActiveAgent(agent) => {
                // An archived agent opened from the Archive, or a channel's
                // coordinator, is never in a list of `SetAgents`: waiting
                // for one would hold back its chat for good, as the session
                // follows each list with this agent again.
                if self.owner(&agent).is_some()
                    || self.is_archived(&agent)
                    || self.is_channel_member(&agent)
                {
                    self.session_switched(agent);
                } else {
                    // A new agent: the list that names it follows.
                    self.unlisted = Some((agent, Vec::new()));
                }
            }
            UiCommand::SwitchFailed(agent) => {
                // The tab that switched to `agent` goes back; the session
                // still has the agent it had.
                if self.focus_sent.as_deref() == Some(agent.as_str()) {
                    self.focus_sent = None;
                }
                let index = self.owner(&agent).unwrap_or_else(|| self.active_index());
                self.tabs[index]
                    .tui
                    .process_command(UiCommand::SwitchFailed(agent));
            }
            UiCommand::SetProcesses(processes) => {
                self.processes = processes;
                self.send_processes();
            }
            UiCommand::SetProjects(projects) => {
                self.projects = projects;
                self.place_projects();
                self.send_agents();
            }
            // Each tab's GRAPH shows the flow of its own agent and ignores
            // the others'; so does its BENCH with sessions and estimates.
            // Every tab lists every benchmark.
            UiCommand::GraphUpsert { .. }
            | UiCommand::FlowLoad { .. }
            | UiCommand::FlowUpsert { .. }
            | UiCommand::BenchList { .. }
            | UiCommand::BenchUpsert(_)
            | UiCommand::BenchSessions { .. }
            | UiCommand::BenchEstimate { .. }
            | UiCommand::EndLink
            | UiCommand::SetAgentDefinitions(_)
            | UiCommand::SetArchivedAgents(_)
            | UiCommand::SetChannels(_)
            | UiCommand::SetModels { .. }
            | UiCommand::SetApiStatus { .. } => {
                let slot = match command {
                    UiCommand::SetAgentDefinitions(_) => Some(0),
                    UiCommand::SetModels { .. } => Some(1),
                    UiCommand::SetApiStatus { .. } => Some(2),
                    UiCommand::SetArchivedAgents(_) => Some(3),
                    UiCommand::SetChannels(_) => Some(4),
                    _ => None,
                };
                if let Some(slot) = slot {
                    self.lists[slot] = Some(command.clone());
                }
                self.broadcast(command);
            }
            other => {
                if matches!(other, UiCommand::ShowError(_)) {
                    // What a tab or `+` asked for may have failed; a later
                    // project must not be taken for the answer.
                    self.opening = false;
                    self.asker = None;
                }
                let index = self.active_index();
                self.tabs[index].tui.process_command(other);
            }
        }
    }

    fn broadcast(&mut self, command: UiCommand) {
        let Some((last, rest)) = self.tabs.split_last_mut() else {
            return;
        };
        for tab in rest {
            tab.tui.process_command(command.clone());
        }
        last.tui.process_command(command);
    }

    /// The session listed its agents: put new projects in a tab, show
    /// each tab its own, and pass on what waited for an agent to be named.
    fn listed(&mut self, agents: Vec<AgentSummary>) {
        self.agents = agents;
        self.place_projects();
        self.send_agents();
        self.send_processes();
        self.release();
    }

    /// Stop waiting for the session to list the agent it switched to.
    fn release(&mut self) {
        if let Some((agent, held)) = self.unlisted.take() {
            self.session_switched(agent);
            for command in held {
                self.route(command);
            }
        }
    }

    /// Keys of the listed projects in list order, with those that have
    /// no agent listed in their places.
    fn project_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        let mut empty = empty_projects(&self.projects, &self.agents)
            .into_iter()
            .peekable();
        for index in (0..self.agents.len()).filter(|&i| starts_project(&self.agents, i)) {
            while let Some((_, at)) = empty.next_if(|&(before, _)| before <= index) {
                keys.push(self.projects[at].id.clone());
            }
            keys.push(self.agents[index].project_key().to_string());
        }
        keys.extend(empty.map(|(_, at)| self.projects[at].id.clone()));
        keys
    }

    fn place_projects(&mut self) {
        let roots = self.project_keys();
        for tab in &mut self.tabs {
            tab.projects.retain(|project| roots.contains(project));
        }
        if let Some(saved) = self.restore.take() {
            self.bring_back(&saved, &roots);
        }
        for root in roots {
            if self.tabs.iter().any(|tab| tab.projects.contains(&root)) {
                continue;
            }
            let index = if std::mem::take(&mut self.opening) {
                self.front = self.open();
                self.front
            } else {
                let asker = self.asker.take();
                asker
                    .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                    .unwrap_or_else(|| self.active_index())
            };
            self.tabs[index].projects.push(root);
        }
        // A tab whose projects are all gone has nothing to show.
        let mut index = 0;
        while index < self.tabs.len() {
            if self.tabs.len() > 1 && self.tabs[index].projects.is_empty() {
                self.tabs.remove(index);
                if self.front > index || self.front >= self.tabs.len() {
                    self.front = self.front.saturating_sub(1);
                }
            } else {
                index += 1;
            }
        }
    }

    /// Split `roots` over tabs as `saved` had them. Projects it doesn't
    /// name are placed afterwards, like new ones.
    fn bring_back(&mut self, saved: &Arrangement, roots: &[String]) {
        let mut placed: Vec<&String> = Vec::new();
        let mut first = true;
        for group in saved {
            let mut projects = Vec::new();
            for id in group {
                if roots.contains(id) && !placed.contains(&id) {
                    placed.push(id);
                    projects.push(id.clone());
                }
            }
            if projects.is_empty() {
                continue;
            }
            let index = if std::mem::take(&mut first) {
                0
            } else {
                self.open()
            };
            self.tabs[index].projects = projects;
        }
    }

    /// The agents of the projects in `tab`, with the active flag on the
    /// agent the tab shows rather than on the session's. Projects come in
    /// the session's order, which keeps the order dragged into a list.
    fn agents_of(&self, tab: &Tab) -> Vec<AgentSummary> {
        let mut out = Vec::new();
        let mut keep = false;
        for (index, agent) in self.agents.iter().enumerate() {
            if starts_project(&self.agents, index) {
                keep = tab.projects.iter().any(|key| key == agent.project_key());
            }
            if !keep {
                continue;
            }
            let mut agent = agent.clone();
            if !tab.agent().is_empty() {
                agent.is_active = agent.id == tab.agent();
            }
            out.push(agent);
        }
        out
    }

    /// Take the order of projects the front tab's list now has, after the
    /// pointer dragged one, to write with the tabs. The session keeps the
    /// order itself, from the tab's `ReorderProjects`.
    /// Projects with no agent listed keep their places.
    pub fn adopt_project_order(&mut self) {
        let tab = &mut self.tabs[self.front];
        let order = tab.tui.project_order();
        let slots: Vec<usize> = (0..tab.projects.len())
            .filter(|&i| order.contains(&tab.projects[i]))
            .collect();
        if slots.len() == order.len() {
            for (slot, key) in slots.into_iter().zip(order) {
                tab.projects[slot] = key;
            }
        }
    }

    /// The summaries of the projects in `tab`.
    fn projects_of(&self, tab: &Tab) -> Vec<ProjectSummary> {
        self.projects
            .iter()
            .filter(|project| tab.projects.contains(&project.id))
            .cloned()
            .collect()
    }

    fn send_agents(&mut self) {
        let lists: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| (self.projects_of(tab), self.agents_of(tab)))
            .collect();
        for (tab, (projects, agents)) in self.tabs.iter_mut().zip(lists) {
            tab.tui.process_command(UiCommand::SetProjects(projects));
            tab.tui.process_command(UiCommand::SetAgents(agents));
        }
        // Tabs may have opened or closed, and titles follow the lists.
        self.sync_workspaces();
    }

    /// Tell each tab's UI the tabs' titles and which of them it is, for
    /// its menu's "Move to workspace".
    fn sync_workspaces(&mut self) {
        let titles: Vec<String> = self.tabs.iter().map(Tab::title).collect();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.tui.set_workspaces(titles.clone(), index);
        }
    }

    /// Show each tab the processes of its own agents, and those of none.
    fn send_processes(&mut self) {
        let lists: Vec<Vec<ProcessSummary>> = (0..self.tabs.len())
            .map(|index| {
                let here = |process: &&ProcessSummary| {
                    self.owner(&process.jump_agent_id)
                        .is_none_or(|owner| owner == index)
                };
                self.processes.iter().filter(here).cloned().collect()
            })
            .collect();
        for (tab, processes) in self.tabs.iter_mut().zip(lists) {
            tab.tui.process_command(UiCommand::SetProcesses(processes));
        }
    }

    /// The session made `agent` its active agent: the tab holding it
    /// shows it. The window follows the session only while the tab in
    /// front has nothing to show, as at the start.
    fn session_switched(&mut self, agent: String) {
        self.session_active.clone_from(&agent);
        if self.focus_sent.as_deref() == Some(agent.as_str()) {
            self.focus_sent = None;
        }
        // An agent no tab holds, such as an archived one, stays in the tab
        // showing it rather than following the tab in front.
        let index = self
            .owner(&agent)
            .or_else(|| self.tabs.iter().position(|tab| tab.agent() == agent))
            .unwrap_or(self.front);
        if self.focus_sent.is_none() && self.front().agent().is_empty() {
            self.front = index;
        }
        self.tabs[index]
            .tui
            .process_command(UiCommand::SetActiveAgent(agent));
        // The tab's title follows the agent it shows.
        self.sync_workspaces();
    }

    /// Add a tab with the lists the others have. Returns its index.
    fn open(&mut self) -> usize {
        let mut tab = Tab::new(self.next_id, self.settings);
        self.next_id += 1;
        for command in self.lists.iter().flatten() {
            tab.tui.process_command(command.clone());
        }
        self.tabs.push(tab);
        self.tabs.len() - 1
    }

    /// `+`: ask for a folder in the tab in front. The project picked gets
    /// a tab of its own once the session lists it; one already listed
    /// brings its tab forward instead.
    pub fn new_tab(&mut self) {
        // Moves may close tabs here; `front` stays a tab's index, and the
        // folder is asked for in whichever tab is in front after them.
        self.forward_events();
        let tab = &mut self.tabs[self.front];
        tab.tui.ask_for_project_folder();
        self.picking = tab.tui.folder_picker().is_some().then_some(tab.id);
    }

    /// The tab holding the listed project in `folder`, and that project's
    /// first agent when it has one listed.
    fn tab_of_folder(&self, folder: &Path) -> Option<(usize, Option<String>)> {
        let folder = folder.canonicalize().ok()?;
        let project = self
            .projects
            .iter()
            .find(|project| Path::new(&project.folder) == folder)?;
        let tab = self
            .tabs
            .iter()
            .position(|tab| tab.projects.contains(&project.id))?;
        let agent = self
            .agents
            .iter()
            .find(|agent| agent.project_key() == project.id)
            .map(|agent| agent.id.clone());
        Some((tab, agent))
    }

    /// Bring the tab at `index` to the front.
    pub fn focus(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.front = index;
            self.sync_focus();
        }
    }

    /// Id of the tab at `index`, which stays the same while tabs around
    /// it open, close and move.
    pub fn id_at(&self, index: usize) -> Option<u64> {
        self.tabs.get(index).map(|tab| tab.id)
    }

    /// Where the tab with id `id` is now, if it is still open.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    pub fn next(&mut self) {
        self.focus((self.front + 1) % self.tabs.len());
    }

    pub fn previous(&mut self) {
        self.focus((self.front + self.tabs.len() - 1) % self.tabs.len());
    }

    /// Move the tab at `from` to `to`, shifting the ones between. The tab
    /// in front stays in front.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if self.front == from {
            self.front = to;
        } else if from < self.front && self.front <= to {
            self.front -= 1;
        } else if to <= self.front && self.front < from {
            self.front += 1;
        }
        self.sync_workspaces();
    }

    /// Close the tab at `index`. Its projects move to the tab before it
    /// (after it, for the first), so nothing is lost. Returns whether the
    /// tab is gone: `false` for the last tab, which stays, and for no tab.
    ///
    /// The moves the tabs' menus asked for go first, and may close tabs
    /// and shift the rest, so the tab is followed by its id, not `index`.
    pub fn close(&mut self, index: usize) -> bool {
        let Some(id) = self.tabs.get(index).map(|tab| tab.id) else {
            return false;
        };
        self.forward_events();
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            // A move emptied it, and it closed then.
            return true;
        };
        if self.tabs.len() < 2 {
            return false;
        }
        let closed = self.tabs.remove(index);
        let into = index.saturating_sub(1);
        if let Some(tab) = self.tabs.get_mut(into) {
            tab.projects.extend(closed.projects);
        }
        if self.front == index {
            self.front = into;
        } else if self.front > index {
            self.front -= 1;
        }
        self.send_agents();
        self.send_processes();
        self.sync_focus();
        true
    }

    /// Send the session what the tabs' UIs asked for since the last call,
    /// and move the projects their menus asked to move, then make sure the
    /// session follows the tab in front.
    ///
    /// A move names its tab by the index the UI was last told, which is
    /// the tabs' order as this call finds them. An earlier move in the
    /// same call may close a tab and shift the rest, so each is looked up
    /// by id rather than taken as an index of the tabs as they are.
    pub fn forward_events(&mut self) {
        let told: Vec<u64> = self.tabs.iter().map(|tab| tab.id).collect();
        let mut index = 0;
        while index < self.tabs.len() {
            while let Some(event) = self.tabs[index].next_event() {
                self.forward(index, event);
            }
            let id = self.tabs[index].id;
            if let Some(asked) = self.tabs[index].tui.take_workspace_move() {
                if let Some(to) = self.target_now(&told, asked.to) {
                    self.move_project(index, &asked.project, to);
                }
                if self.tabs.get(index).map(|tab| tab.id) != Some(id) {
                    // The tab closed, emptied: the next one took its place.
                    continue;
                }
            }
            index += 1;
        }
        // The browser `+` opened was closed without a pick.
        if let Some(id) = self.picking {
            let open = self
                .tabs
                .iter()
                .any(|tab| tab.id == id && tab.tui.folder_picker().is_some());
            if !open {
                self.picking = None;
            }
        }
        self.sync_workspaces();
        self.sync_focus();
    }

    /// Move the project with key `key` from the tab at `from` to `to`. A
    /// tab left with no project closes, and the one the project went to
    /// takes its place in front if it was there. A tab left without the
    /// agent it showed loads the first agent of the projects it still has,
    /// staying where it is; with no agent left to show (its other projects
    /// have every agent archived), the tab the project went to comes to
    /// the front and shows that agent instead.
    ///
    /// Does nothing when `from` doesn't hold the project, when `to` is
    /// `from` or no tab, or when the project would go to a new tab from
    /// one it is alone in: that is already a tab of its own.
    fn move_project(&mut self, from: usize, key: &str, to: WorkspaceTarget) {
        let Some(tab) = self.tabs.get(from) else {
            return;
        };
        let Some(at) = tab.projects.iter().position(|project| project == key) else {
            return;
        };
        let alone = tab.projects.len() == 1;
        let shown = tab.agent().to_string();
        let shown_moves = self.root_of(&shown) == Some(key);
        let mut into = match to {
            WorkspaceTarget::Existing(index) if index != from && index < self.tabs.len() => index,
            WorkspaceTarget::New if !alone => self.open(),
            WorkspaceTarget::Existing(_) | WorkspaceTarget::New => return,
        };
        let project = self.tabs[from].projects.remove(at);
        self.tabs[into].projects.push(project);
        if alone {
            // `into` is another tab, so this is not the last.
            self.tabs.remove(from);
            if into > from {
                into -= 1;
            }
            if self.front == from {
                self.front = into;
            } else if self.front > from {
                self.front -= 1;
            }
        } else if shown_moves {
            match self.first_agent(from) {
                Some(agent) => self.request(&agent, true),
                None => {
                    self.front = into;
                    self.load_into(into, &shown);
                }
            }
        }
        self.send_agents();
        self.send_processes();
        self.sync_focus();
    }

    /// Have the session show `agent`, of a project the tab at `index`
    /// holds, in that tab with its chat. The session reloads the chat
    /// only of an agent it switches to, so when it has `agent` active
    /// already it is turned to [`Self::stand_in`] first.
    fn load_into(&mut self, index: usize, agent: &str) {
        if self.asked_for(agent) {
            let Some(other) = self.stand_in(index, agent) else {
                // The session's only agent: show it, without its past.
                if let Some(tab) = self.tabs.get_mut(index) {
                    let command = UiCommand::SetActiveAgent(agent.to_string());
                    tab.tui.process_command(command);
                }
                return;
            };
            self.request(&other, false);
        }
        self.request(agent, true);
    }

    /// An agent other than `agent` for the session to turn to on the way.
    /// Best is one a tab shows and holds, the tab at `index` first, as
    /// the switch to it changes what no tab shows. Else another agent of
    /// that tab, which shows it until `agent` comes, or of the session.
    fn stand_in(&self, index: usize, agent: &str) -> Option<String> {
        let shown = std::iter::once(index)
            .chain(0..self.tabs.len())
            .filter_map(|at| {
                let shown = self.tabs.get(at)?.agent();
                (self.owner(shown) == Some(at)).then_some(shown)
            })
            .find(|shown| *shown != agent);
        if let Some(shown) = shown {
            return Some(shown.to_string());
        }
        let projects = self.tabs.get(index).map_or(&[][..], |tab| &tab.projects);
        let in_tab =
            |listed: &&AgentSummary| projects.iter().any(|key| key == listed.project_key());
        self.agents
            .iter()
            .filter(|listed| listed.id != agent)
            .find(in_tab)
            .or_else(|| self.agents.iter().find(|listed| listed.id != agent))
            .map(|listed| listed.id.clone())
    }

    /// `to`, an index into the tabs with ids `told`, as an index into the
    /// tabs as they are now; `None` when that tab is gone or never was.
    fn target_now(&self, told: &[u64], to: WorkspaceTarget) -> Option<WorkspaceTarget> {
        match to {
            WorkspaceTarget::Existing(index) => {
                let id = *told.get(index)?;
                let now = self.tabs.iter().position(|tab| tab.id == id)?;
                Some(WorkspaceTarget::Existing(now))
            }
            WorkspaceTarget::New => Some(WorkspaceTarget::New),
        }
    }

    /// The first agent listed of the projects in the tab at `index`.
    fn first_agent(&self, index: usize) -> Option<String> {
        self.tabs.get(index)?.projects.iter().find_map(|key| {
            self.agents
                .iter()
                .find(|agent| agent.project_key() == key)
                .map(|agent| agent.id.clone())
        })
    }

    fn forward(&mut self, from: usize, event: UiEvent) {
        if let UiEvent::CreateProject { folder } = &event {
            if self.picking == Some(self.tabs[from].id) {
                self.picking = None;
                match self.tab_of_folder(folder) {
                    Some((owner, Some(agent))) => {
                        self.front = owner;
                        let shown = self.shown_by(owner);
                        if self.root_of(shown) != self.root_of(&agent) {
                            self.request(&agent, true);
                        }
                        return;
                    }
                    // Listed with every agent archived: the session adds
                    // one to it, in its tab.
                    Some((owner, None)) => {
                        self.front = owner;
                        self.asker = Some(self.tabs[owner].id);
                        let agent = self.shown_by(owner).to_string();
                        self.request(&agent, false);
                        self.send(event);
                        return;
                    }
                    None => {
                        self.opening = true;
                        self.asker = None;
                        let agent = self.shown_by(from).to_string();
                        self.request(&agent, false);
                        self.send(event);
                        return;
                    }
                }
            }
        }
        match &event {
            UiEvent::SelectAgent(agent) | UiEvent::FocusAgent(agent)
                if self.owner(agent) == Some(from) =>
            {
                let load = matches!(event, UiEvent::SelectAgent(_));
                let agent = agent.clone();
                self.switch_in_tab(from, agent, load);
                return;
            }
            UiEvent::SelectAgent(agent) => {
                // An agent of another tab: go to that tab instead.
                if let Some(owner) = self.owner(agent).filter(|owner| *owner != from) {
                    self.front = owner;
                    if self.tabs[owner].agent() != agent {
                        let agent = agent.clone();
                        self.request(&agent, true);
                    }
                    return;
                }
            }
            UiEvent::CreateProject { .. } | UiEvent::CreateAgent(_) | UiEvent::RestoreAgent(_) => {
                self.asker = Some(self.tabs[from].id);
            }
            _ => {}
        }
        let agent = self.shown_by(from).to_string();
        self.request(&agent, false);
        self.send(event);
    }

    /// Make the session follow the tab in front. A tab that has shown
    /// nothing yet opens on its first project with an agent listed, and so
    /// does one still showing an agent whose project moved to another tab,
    /// until the session shows it its own; with none, it asks for nothing.
    /// Nor does a tab in front waiting for the session to show it one of
    /// its agents: asking for the agent it shows now would undo that.
    fn sync_focus(&mut self) {
        if self.front_awaits_agent() {
            return;
        }
        let shown = self.shown_by(self.front).to_string();
        if shown.is_empty() {
            if let Some(agent) = self.first_agent(self.front) {
                self.request(&agent, true);
            }
        } else {
            self.request(&shown, false);
        }
    }

    /// Whether the session was asked to switch to an agent of the tab in
    /// front other than the one it shows, and hasn't yet.
    fn front_awaits_agent(&self) -> bool {
        self.focus_sent.as_deref().is_some_and(|sent| {
            sent != self.front().agent() && self.owner(sent) == Some(self.front)
        })
    }

    /// Whether the session has `agent` active, or was last asked to.
    fn asked_for(&self, agent: &str) -> bool {
        match &self.focus_sent {
            Some(sent) => sent == agent,
            None => self.session_active == agent,
        }
    }

    /// The tab at `from` switched to `agent`, one of its own, on screen
    /// already (see `App::request_agent_switch`), and asks the session to
    /// follow: with the agent's chat (`load`) or without. Asking for the
    /// agent the tab shows first, as other events do, would turn the
    /// session to `agent` without its chat.
    ///
    /// The tab waits for the session's `SetActiveAgent`. When the session
    /// has `agent` active already and nothing is on its way, it would send
    /// none, so the answer is given here instead.
    fn switch_in_tab(&mut self, from: usize, agent: String, load: bool) {
        if load {
            // Sent even to an agent the session has active: it sends the
            // chat of an agent selected again.
            self.focus_sent = Some(agent.clone());
            self.send(UiEvent::SelectAgent(agent));
        } else if !self.asked_for(&agent) {
            self.request(&agent, false);
        } else if self.focus_sent.is_none() {
            if let Some(tab) = self.tabs.get_mut(from) {
                tab.tui.process_command(UiCommand::SetActiveAgent(agent));
            }
        }
    }

    /// Ask the session to make `agent` its active agent, unless it is or
    /// was already asked to. `load` asks for the agent's chat as well,
    /// for a tab that doesn't hold it yet.
    fn request(&mut self, agent: &str, load: bool) {
        if agent.is_empty() || self.asked_for(agent) {
            return;
        }
        self.focus_sent = Some(agent.to_string());
        self.send(if load {
            UiEvent::SelectAgent(agent.to_string())
        } else {
            UiEvent::FocusAgent(agent.to_string())
        });
    }

    fn send(&self, event: UiEvent) {
        if let Err(e) = self.session.try_send(event) {
            tracing::warn!(error = %e, "the agent session did not take an event");
        }
    }
}

/// Working a tab's UI as the pointer would, for the tests here and the
/// shell's.
#[cfg(test)]
pub(crate) mod testing {
    use aura_ui_terminal::App;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    /// `tui`, drawn 120 columns by 30 rows, a line per row.
    pub fn screen(tui: &mut App) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("never fails");
        let theme = aura_ui_terminal::Theme::cyber();
        terminal
            .draw(|frame| aura_ui_terminal::render(frame, tui, &theme))
            .expect("a test backend draws");
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    /// The cell where `text` first shows in `rows`.
    pub fn cell_of(rows: &[String], text: &str) -> (u16, u16) {
        let row = rows
            .iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no {text}:\n{}", rows.join("\n")));
        let line = &rows[row];
        let column = line[..line.find(text).unwrap_or(0)].chars().count();
        (
            u16::try_from(column).expect("on screen"),
            u16::try_from(row).expect("on screen"),
        )
    }

    pub fn click(tui: &mut App, button: MouseButton, (column, row): (u16, u16)) {
        tui.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(button),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// Right-click `name` in the projects list of `tui`, then pick
    /// `entry` from its "Move to workspace".
    pub fn pick_move(tui: &mut App, name: &str, entry: &str) {
        let rows = screen(tui);
        click(tui, MouseButton::Right, cell_of(&rows, name));
        let rows = screen(tui);
        click(tui, MouseButton::Left, cell_of(&rows, "Move to workspace"));
        let rows = screen(tui);
        click(tui, MouseButton::Left, cell_of(&rows, entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui::events::{MessageData, MessageRole};
    use aura_ui_terminal::AlertSettings;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

    fn tabs_with(restore: &[&[&str]]) -> (Tabs, mpsc::Receiver<UiEvent>) {
        let (session, events) = mpsc::channel(64);
        let settings = Settings {
            verbose: false,
            alerts: AlertSettings::from_env(),
            measure: crate::text::prose_cells,
        };
        let restore = restore
            .iter()
            .map(|group| group.iter().map(ToString::to_string).collect())
            .collect();
        (Tabs::new(session, settings, restore), events)
    }

    fn tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
        tabs_with(&[])
    }

    /// `(id, depth)` of each agent; depth 0 heads a project.
    type Agents<'a> = &'a [(&'a str, usize)];

    fn list(agents: Agents, active: &str) -> UiCommand {
        UiCommand::SetAgents(
            agents
                .iter()
                .map(|(id, depth)| AgentSummary {
                    id: (*id).into(),
                    name: id.to_uppercase(),
                    depth: *depth,
                    is_active: *id == active,
                    ..AgentSummary::default()
                })
                .collect(),
        )
    }

    /// What the session sends at the start.
    fn start(tabs: &mut Tabs, agents: Agents, active: &str) {
        tabs.apply(list(agents, active));
        tabs.apply(UiCommand::SetActiveAgent(active.into()));
    }

    /// What the session sends when it switches to `active`.
    fn switch(tabs: &mut Tabs, agents: Agents, active: &str) {
        tabs.apply(UiCommand::SetActiveAgent(active.into()));
        tabs.apply(list(agents, active));
        tabs.apply(UiCommand::SetActiveAgent(active.into()));
    }

    fn sent(events: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(event) = events.try_recv() {
            out.push(format!("{event:?}"));
        }
        out
    }

    /// The agent each tab shows.
    fn shown(tabs: &Tabs) -> Vec<&str> {
        tabs.iter().map(Tab::agent).collect()
    }

    /// The agents each tab lists.
    fn held(tabs: &Tabs) -> Vec<Vec<&str>> {
        tabs.iter()
            .map(|tab| tab.tui.agents().iter().map(|a| a.id.as_str()).collect())
            .collect()
    }

    /// The listed projects, by id.
    fn projects(ids: &[&str]) -> UiCommand {
        UiCommand::SetProjects(
            ids.iter()
                .map(|id| ProjectSummary {
                    id: (*id).into(),
                    name: (*id).into(),
                    ..ProjectSummary::default()
                })
                .collect(),
        )
    }

    /// The projects `tab` lists, by id.
    fn listed(tab: &Tab) -> Vec<String> {
        tab.tui.projects().iter().map(|p| p.id.clone()).collect()
    }

    fn message(text: &str) -> UiCommand {
        UiCommand::ShowMessage(MessageData::new(MessageRole::System, text))
    }

    fn messages(tabs: &Tabs) -> Vec<usize> {
        tabs.iter().map(|tab| tab.tui.messages().len()).collect()
    }

    const ONE: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0)];
    const TWO: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0)];

    /// `+`, then a folder that is no project yet picked in the browser it
    /// opens in the tab in front.
    fn plus(tabs: &mut Tabs) {
        tabs.new_tab();
        assert!(
            tabs.front().tui.folder_picker().is_some(),
            "+ asks for a folder"
        );
        let event = UiEvent::CreateProject {
            folder: "/new/project".into(),
        };
        tabs.forward(tabs.front_index(), event);
    }

    /// Two tabs: every project of [`ONE`], and `a3` opened with `+` and
    /// in front.
    fn two_tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        plus(&mut tabs);
        switch(&mut tabs, TWO, "a3");
        let _ = sent(&mut events);
        (tabs, events)
    }

    #[test]
    fn an_archived_agent_opens_in_the_tab_that_asked_and_nothing_waits_for_it() {
        let (mut tabs, mut events) = two_tabs();
        let archive = UiCommand::SetArchivedAgents(vec![AgentSummary {
            id: "z9".into(),
            name: "Z9".into(),
            ..AgentSummary::default()
        }]);
        tabs.apply(archive.clone());

        tabs.forward(1, UiEvent::SelectAgent("z9".into()));
        assert!(sent(&mut events).contains(&r#"SelectAgent("z9")"#.to_string()));
        // The session's answer, then a refresh of the lists after a turn.
        tabs.apply(UiCommand::SetActiveAgent("z9".into()));
        tabs.apply(list(TWO, "z9"));
        tabs.apply(archive);
        tabs.apply(UiCommand::SetActiveAgent("z9".into()));
        assert_eq!(shown(&tabs), ["a1", "z9"]);
        assert_eq!(tabs.front_index(), 1);

        // What comes next lands at once, in its tab, without waiting for
        // a list that would name the agent.
        let before = messages(&tabs);
        tabs.apply(message("reply"));
        assert_eq!(messages(&tabs), [before[0], before[1] + 1]);
        assert_eq!(shown(&tabs), ["a1", "z9"]);
    }

    #[test]
    fn a_channel_opens_in_its_tab_and_its_chat_is_not_held_back() {
        let (mut tabs, mut events) = two_tabs();
        let channels = UiCommand::SetChannels(vec![aura_ui::events::ChannelSummary {
            id: "ch".into(),
            slug: "zode".into(),
            coordinator_id: "c1".into(),
            members: vec![AgentSummary {
                id: "c1".into(),
                name: "C1".into(),
                ..AgentSummary::default()
            }],
            ..aura_ui::events::ChannelSummary::default()
        }]);
        tabs.apply(channels.clone());

        // CHANNELS opens the channel there is, its coordinator on screen
        // at once, waiting for its chat.
        tabs.front_mut()
            .tui
            .select_pane_tab(aura_ui_terminal::PaneTab::Agents(
                aura_ui_terminal::AgentsTab::Channels,
            ));
        tabs.forward_events();
        let asked = sent(&mut events);
        assert!(asked.contains(&r#"SelectAgent("c1")"#.to_string()), "{asked:?}");
        assert!(asked.contains(&r#"OpenChannel("ch")"#.to_string()), "{asked:?}");
        assert_eq!(shown(&tabs), ["a1", "c1"]);
        assert!(tabs.front().tui.chat_loading());

        // The session's answer: the coordinator, the lists (which leave it
        // out) and the coordinator again, then its chat.
        tabs.apply(UiCommand::SetActiveAgent("c1".into()));
        tabs.apply(list(TWO, "c1"));
        tabs.apply(channels);
        tabs.apply(UiCommand::SetActiveAgent("c1".into()));
        tabs.apply(UiCommand::ForAgent {
            agent_id: "c1".into(),
            command: Box::new(UiCommand::LoadConversation(vec![MessageData::new(
                MessageRole::Assistant,
                "hello from the coordinator",
            )])),
        });
        assert_eq!(shown(&tabs), ["a1", "c1"]);
        assert!(!tabs.front().tui.chat_loading(), "the chat is not held back");
        assert_eq!(tabs.front().tui.messages().len(), 1);
    }

    #[test]
    fn the_window_starts_with_every_project_in_one_tab() {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.arrangement(), [["a1", "p2"]]);
        assert_eq!(held(&tabs), [["a1", "a1.1", "p2"]]);
        assert_eq!(shown(&tabs), ["a1"]);
        assert_eq!(tabs.front().title(), "A1");

        // Picking another agent of the tab stays in the tab.
        tabs.forward(0, UiEvent::SelectAgent("a1.1".into()));
        switch(&mut tabs, ONE, "a1.1");
        assert_eq!(shown(&tabs), ["a1.1"]);
        assert_eq!(tabs.front().title(), "A1 / A1.1");
        tabs.forward_events();
        assert_eq!(sent(&mut events), [r#"SelectAgent("a1.1")"#]);
    }

    #[test]
    fn plus_opens_a_tab_holding_the_project_picked_only() {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        let _ = sent(&mut events);
        tabs.new_tab();
        assert!(tabs.front().tui.folder_picker().is_some());
        tabs.forward_events();
        assert!(
            sent(&mut events).is_empty(),
            "nothing until a folder is picked"
        );
        let event = UiEvent::CreateProject {
            folder: "/new/project".into(),
        };
        tabs.forward(0, event);
        assert_eq!(
            sent(&mut events),
            [r#"CreateProject { folder: "/new/project" }"#]
        );
        assert_eq!(tabs.len(), 1, "no tab until the project exists");

        // The session switches to the agent before it lists it.
        let before = messages(&tabs)[0];
        tabs.apply(UiCommand::SetActiveAgent("a3".into()));
        tabs.apply(message("for the new agent"));
        assert_eq!(messages(&tabs), [before], "kept for the new tab");
        tabs.apply(list(TWO, "a3"));
        tabs.apply(UiCommand::SetActiveAgent("a3".into()));

        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1", "p2"], vec!["a3"]]);
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(messages(&tabs)[0], before);
        let new = tabs.front().tui.messages();
        assert!(new.iter().any(|m| m.content() == "for the new agent"));
        let active = |tab: &Tab| -> Vec<String> {
            let agents = tab.tui.agents().iter().filter(|a| a.is_active);
            agents.map(|a| a.id.clone()).collect()
        };
        assert_eq!(active(&tabs.tabs[0]), ["a1"]);
        assert_eq!(active(&tabs.tabs[1]), ["a3"]);

        tabs.forward_events();
        assert!(sent(&mut events).is_empty());
    }

    #[test]
    fn a_project_joins_the_tab_that_asked_for_it() {
        let (mut tabs, mut events) = two_tabs();
        tabs.focus(0);
        switch(&mut tabs, TWO, "a1");
        let _ = sent(&mut events);

        let event = UiEvent::CreateProject {
            folder: "/tmp".into(),
        };
        tabs.forward(0, event);
        assert_eq!(sent(&mut events).len(), 1);
        let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
        switch(&mut tabs, more, "p4");
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2", "p4"], vec!["a3"]]);
        assert_eq!(shown(&tabs), ["p4", "a3"]);
        assert_eq!(tabs.front_index(), 0);
    }

    #[test]
    fn an_agent_added_to_a_project_stays_in_its_tab() {
        let (mut tabs, _events) = two_tabs();
        let UiCommand::SetAgents(mut agents) = list(TWO, "a1") else {
            unreachable!("list sends agents");
        };
        for agent in &mut agents[..2] {
            agent.project_id = Some("a1".into());
        }
        agents.insert(
            2,
            AgentSummary {
                id: "a1.2".into(),
                name: "A1.2".into(),
                project_id: Some("a1".into()),
                ..AgentSummary::default()
            },
        );
        tabs.apply(UiCommand::SetAgents(agents));
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1", "a1.2", "p2"], vec!["a3"]]);
    }

    #[test]
    fn a_project_with_every_agent_archived_stays_in_its_tab_until_removed() {
        let (mut tabs, _events) = two_tabs();

        // p2's agent and then a3, the only one of its tab, are archived.
        tabs.apply(projects(&["a1", "p2", "a3"]));
        tabs.apply(list(&[("a1", 0), ("a1.1", 1), ("a3", 0)], "a3"));
        tabs.apply(list(&[("a1", 0), ("a1.1", 1)], "a1"));
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(listed(&tabs.tabs[0]), ["a1", "p2"]);
        assert_eq!(listed(&tabs.tabs[1]), ["a3"]);

        // Removed, they leave their tabs, and the tab left with nothing
        // closes.
        tabs.apply(projects(&["a1"]));
        tabs.apply(list(&[("a1", 0), ("a1.1", 1)], "a1"));
        assert_eq!(tabs.arrangement(), [["a1"]]);
    }

    #[test]
    fn plus_closed_without_a_folder_opens_no_tab() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        tabs.new_tab();
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        tabs.front_mut().tui.handle_key(esc);
        tabs.forward_events();
        assert!(tabs.front().tui.folder_picker().is_none());
        let _ = sent(&mut events);

        // A project asked for later, as with Ctrl+N, joins the tab asking.
        let event = UiEvent::CreateProject {
            folder: "/later".into(),
        };
        tabs.forward(0, event);
        switch(&mut tabs, TWO, "a3");
        assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
    }

    #[test]
    fn plus_on_a_folder_already_a_project_brings_its_tab_forward() {
        let (mut tabs, mut events) = two_tabs();
        let folder = std::env::temp_dir().canonicalize().expect("temp dir");
        let project = |id: &str, folder: &Path| ProjectSummary {
            id: id.into(),
            name: id.into(),
            folder: folder.display().to_string(),
            ..ProjectSummary::default()
        };
        tabs.apply(UiCommand::SetProjects(vec![
            project("a1", Path::new("/site")),
            project("p2", Path::new("/notes")),
            project("a3", &folder),
        ]));
        tabs.focus(0);
        tabs.forward_events();
        let _ = sent(&mut events);

        tabs.new_tab();
        tabs.forward(0, UiEvent::CreateProject { folder });
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(tabs.len(), 2);
        assert!(
            !sent(&mut events)
                .iter()
                .any(|e| e.starts_with("CreateProject")),
            "nothing to create"
        );
        // A later project is not taken for the one `+` asked for.
        tabs.focus(0);
        let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
        switch(&mut tabs, more, "p4");
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn a_failed_create_opens_no_tab() {
        let (mut tabs, _events) = tabs();
        start(&mut tabs, ONE, "a1");
        plus(&mut tabs);
        tabs.apply(UiCommand::ShowError("Could not create agent".into()));
        assert_eq!(tabs.len(), 1);
        assert!(matches!(
            tabs.front().tui.notification(),
            Some((text, _)) if text.contains("Could not create agent")
        ));

        // A later project is not taken for the one `+` asked for.
        switch(&mut tabs, TWO, "a3");
        assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
    }

    #[test]
    fn commands_reach_the_tab_holding_their_agent() {
        let (mut tabs, _events) = two_tabs();
        let before = messages(&tabs);

        // Unaddressed: the tab holding the session's active agent.
        tabs.apply(message("for the active agent"));
        assert_eq!(messages(&tabs), [before[0], before[1] + 1]);

        // Addressed: the tab holding the agent, on screen or not.
        tabs.apply(UiCommand::ForAgent {
            agent_id: "a1.1".into(),
            command: Box::new(UiCommand::RequestApproval {
                id: "r1".into(),
                tool: "run_command".into(),
                description: "cargo test".into(),
                always: String::new(),
            }),
        });
        assert!(tabs.tabs[0].tui.agent_busy("a1.1"));
        assert!(tabs.tabs[0].busy());
        assert!(!tabs.tabs[1].tui.agent_busy("a1.1"));
        assert!(!tabs.tabs[1].busy());

        // Lists that aren't about agents: every tab.
        tabs.apply(UiCommand::SetApiStatus {
            url: Some("http://127.0.0.1:1".into()),
            active: true,
        });
        assert!(tabs.iter().all(|tab| tab.tui.api_active()));

        // Benchmarks: every tab lists every one, whichever tree it is in.
        tabs.apply(UiCommand::BenchList {
            benches: Vec::new(),
        });
        tabs.apply(UiCommand::BenchUpsert(Box::new(
            aura_ui_terminal::events::BenchView {
                id: "c1".into(),
                root: "a1".into(),
                ..Default::default()
            },
        )));
        let listed: Vec<usize> = tabs
            .iter()
            .map(|tab| tab.tui.bench().benches.len())
            .collect();
        assert_eq!(listed, [1, 1]);
    }

    #[test]
    fn a_tab_lists_the_processes_of_its_own_agents() {
        let (mut tabs, _events) = two_tabs();
        let process = |id: &str, agent: &str| ProcessSummary {
            id: id.into(),
            jump_agent_id: agent.into(),
            ..ProcessSummary::default()
        };
        tabs.apply(UiCommand::SetProcesses(vec![
            process("1", "a1.1"),
            process("2", "a3"),
            process("3", ""),
        ]));
        let listed = |tab: &Tab| -> Vec<String> {
            let processes = tab.tui.processes().iter();
            processes.map(|p| p.id.clone()).collect()
        };
        assert_eq!(listed(&tabs.tabs[0]), ["1", "3"]);
        assert_eq!(listed(&tabs.tabs[1]), ["2", "3"]);
    }

    /// Whether tab `tab` lists agent `id` as in plan mode, if it lists it.
    fn plan_mode_in(tabs: &Tabs, tab: usize, id: &str) -> Option<bool> {
        let agents = tabs.tabs[tab].tui.agents().iter();
        agents.filter(|a| a.id == id).map(|a| a.plan_mode).next()
    }

    fn set_mode_for(agent_id: &str, mode: aura_ui_terminal::PermissionMode) -> UiCommand {
        UiCommand::ForAgent {
            agent_id: agent_id.into(),
            command: Box::new(UiCommand::SetMode(mode)),
        }
    }

    #[test]
    fn a_plan_mode_switch_survives_the_lists_sent_on_to_tabs() {
        use aura_ui_terminal::PermissionMode;
        let (mut tabs, _events) = two_tabs();

        // a1.1, off screen in the back tab, goes into plan mode.
        tabs.apply(set_mode_for("a1.1", PermissionMode::Plan));
        assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true));
        assert_eq!(plan_mode_in(&tabs, 0, "a1"), Some(false));
        assert_eq!(plan_mode_in(&tabs, 1, "a1.1"), None, "not its tab's");

        // New projects re-send each tab its agents from the router's copy.
        tabs.apply(projects(&["a1", "p2", "a3"]));
        assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true), "still marked");
        assert_eq!(plan_mode_in(&tabs, 0, "a1"), Some(false));
        assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(false));

        // a3, on screen in the front tab, too.
        tabs.apply(set_mode_for("a3", PermissionMode::Plan));
        tabs.apply(projects(&["a1", "p2", "a3"]));
        assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(true));
        assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true));

        // Out of plan mode, the next re-send keeps it unmarked.
        tabs.apply(set_mode_for("a1.1", PermissionMode::Agent));
        assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(false));
        tabs.apply(projects(&["a1", "p2", "a3"]));
        assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(false));
        assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(true));

        // The session's own list says who is in plan mode from then on.
        tabs.apply(list(TWO, "a3"));
        assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(false));
    }

    #[test]
    fn bringing_a_tab_forward_asks_the_session_once() {
        let (mut tabs, mut events) = two_tabs();
        tabs.focus(0);
        tabs.forward_events();
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

        switch(&mut tabs, TWO, "a1");
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        assert_eq!(tabs.front_index(), 0);
        tabs.forward_events();
        assert!(sent(&mut events).is_empty());

        tabs.next();
        tabs.previous();
        assert_eq!(
            sent(&mut events),
            [r#"FocusAgent("a3")"#, r#"FocusAgent("a1")"#]
        );
    }

    #[test]
    fn the_tabs_of_the_last_run_come_back() {
        let (mut tabs, mut events) = tabs_with(&[&["gone", "p2"], &["a1"], &["missing"]]);
        start(&mut tabs, TWO, "a1");
        // `a3` is new to the arrangement and joins the first tab.
        assert_eq!(tabs.arrangement(), [vec!["p2", "a3"], vec!["a1"]]);
        assert_eq!(held(&tabs), [vec!["p2", "a3"], vec!["a1", "a1.1"]]);
        assert_eq!(tabs.front_index(), 1, "the session's agent is in front");
        assert_eq!(shown(&tabs), ["", "a1"]);
        tabs.forward_events();
        assert!(sent(&mut events).is_empty());

        // A tab that has shown nothing yet loads its first project.
        tabs.focus(0);
        assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
        switch(&mut tabs, TWO, "p2");
        assert_eq!(shown(&tabs), ["p2", "a1"]);
        assert_eq!(tabs.front_index(), 0);
    }

    #[test]
    fn a_project_with_every_agent_archived_comes_back_in_its_tab() {
        let (mut tabs, _events) = tabs_with(&[&["a1", "p2"], &["a3"]]);
        tabs.apply(projects(&["a1", "p2", "a3"]));
        start(&mut tabs, &[("a1", 0), ("a1.1", 1), ("a3", 0)], "a3");
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(listed(&tabs.tabs[0]), ["a1", "p2"]);
        assert_eq!(listed(&tabs.tabs[1]), ["a3"]);
    }

    #[test]
    fn an_agent_of_another_tab_is_shown_in_its_own_tab() {
        let (mut tabs, mut events) = two_tabs();
        tabs.forward(1, UiEvent::SelectAgent("a1".into()));
        tabs.forward_events();
        assert_eq!(tabs.front_index(), 0);
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

        tabs.forward(0, UiEvent::SelectAgent("a3".into()));
        switch(&mut tabs, TWO, "a1");
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(shown(&tabs), ["a1", "a3"]);
    }

    #[test]
    fn a_tab_that_is_not_active_gets_the_session_first() {
        let (mut tabs, mut events) = two_tabs();
        tabs.forward(0, UiEvent::NewSession);
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#, "NewSession"]);
        // The tab in front takes the session back.
        tabs.forward_events();
        assert_eq!(sent(&mut events), [r#"FocusAgent("a3")"#]);
    }

    #[test]
    fn closing_a_tab_moves_its_projects_to_the_one_before() {
        let (mut tabs, mut events) = two_tabs();
        assert!(tabs.close(1));
        assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
        assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);
        assert_eq!(tabs.front_index(), 0);
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);
        assert!(!tabs.close(0), "the last tab stays");

        // The first tab's projects move to the one after it.
        let (mut tabs, _events) = two_tabs();
        assert!(tabs.close(0));
        assert_eq!(tabs.arrangement(), [["a3", "a1", "p2"]]);
        assert_eq!(shown(&tabs), ["a3"]);
    }

    #[test]
    fn moving_a_tab_keeps_its_projects_and_the_tab_in_front() {
        let (mut tabs, mut events) = two_tabs();
        tabs.move_tab(1, 0);
        assert_eq!(tabs.arrangement(), [vec!["a3"], vec!["a1", "p2"]]);
        assert_eq!(held(&tabs), [vec!["a3"], vec!["a1", "a1.1", "p2"]]);
        assert_eq!(tabs.front_index(), 0);
        assert_eq!(tabs.front().agent(), "a3");
        tabs.forward_events();
        assert!(sent(&mut events).is_empty());

        // Another tab moving past the one in front shifts it.
        tabs.move_tab(1, 0);
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(tabs.front().agent(), "a3");

        tabs.move_tab(0, 5);
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    }

    #[test]
    fn a_dragged_order_of_projects_is_listed_and_saved() {
        let (mut tabs, _events) = tabs();
        start(&mut tabs, TWO, "a1");
        assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);

        // The front tab's list was reordered by a drag.
        let dragged: Agents = &[("p2", 0), ("a1", 0), ("a1.1", 1), ("a3", 0)];
        tabs.tabs[0].tui.process_command(list(dragged, "a1"));
        tabs.adopt_project_order();
        assert_eq!(tabs.arrangement(), [["p2", "a1", "a3"]]);

        // The session, told of the drag, lists its agents in that order.
        tabs.apply(list(dragged, "a1"));
        assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3"]]);

        // A new project goes last; the order survives the file.
        let more: Agents = &[("p2", 0), ("a1", 0), ("a1.1", 1), ("a3", 0), ("p4", 0)];
        tabs.apply(list(more, "a1"));
        assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3", "p4"]]);
        let path = std::env::temp_dir().join(format!("aura_order_{}", std::process::id()));
        save_arrangement(&path, &tabs.arrangement()).expect("temp dir is writable");
        let saved = load_arrangement(&path);
        std::fs::remove_file(&path).expect("just written");

        let restore: Vec<Vec<&str>> = saved
            .iter()
            .map(|tab| tab.iter().map(String::as_str).collect())
            .collect();
        let restore: Vec<&[&str]> = restore.iter().map(Vec::as_slice).collect();
        let (mut tabs, _events) = tabs_with(&restore);
        start(&mut tabs, more, "a1");
        assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3", "p4"]]);
    }

    #[test]
    fn the_arrangement_survives_a_round_trip_through_its_file() {
        let path = std::env::temp_dir().join(format!("aura_tabs_{}", std::process::id()));
        let arrangement: Arrangement = vec![vec!["a1".into(), "p2".into()], vec!["a3".into()]];
        save_arrangement(&path, &arrangement).expect("temp dir is writable");
        assert_eq!(load_arrangement(&path), arrangement);
        std::fs::remove_file(&path).expect("just written");
        assert!(load_arrangement(&path).is_empty());
    }

    #[test]
    fn quit_ends_the_session_from_any_tab() {
        let (mut tabs, mut events) = two_tabs();
        tabs.forward(1, UiEvent::Quit);
        assert_eq!(sent(&mut events), ["Quit"]);
    }

    /// Three tabs of one project each, `a3`'s in front.
    fn three_tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
        let (mut tabs, mut events) = tabs_with(&[&["a1"], &["p2"], &["a3"]]);
        start(&mut tabs, TWO, "a3");
        assert_eq!(tabs.front_index(), 2);
        let _ = sent(&mut events);
        (tabs, events)
    }

    #[test]
    fn a_project_moves_to_another_tab() {
        let (mut tabs, mut events) = two_tabs();
        tabs.move_project(0, "p2", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3", "p2"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["p2", "a3"]]);
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        assert_eq!(tabs.front_index(), 1);
        tabs.forward_events();
        assert!(sent(&mut events).is_empty(), "no tab lost what it shows");
    }

    #[test]
    fn a_project_moves_to_a_new_tab() {
        let (mut tabs, mut events) = two_tabs();
        tabs.move_project(0, "p2", WorkspaceTarget::New);
        assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3"], vec!["p2"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["a3"], vec!["p2"]]);
        assert_eq!(tabs.front_index(), 1, "the tab in front stays");
        assert!(sent(&mut events).is_empty());

        // Brought forward, the new tab loads its project.
        tabs.focus(2);
        assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
    }

    #[test]
    fn moving_the_last_project_of_a_tab_closes_it() {
        let (mut tabs, mut events) = two_tabs();
        tabs.move_project(1, "a3", WorkspaceTarget::Existing(0));
        assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
        assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);
        assert_eq!(tabs.front_index(), 0, "the tab it went to comes forward");
        assert_eq!(shown(&tabs), ["a1"]);
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

        // A tab closing before the one in front shifts it.
        let (mut tabs, mut events) = three_tabs();
        tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["p2", "a1"], vec!["a3"]]);
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(tabs.front().agent(), "a3");
        assert!(sent(&mut events).is_empty());

        // The one in front closing, after the tab it goes to.
        let (mut tabs, _events) = three_tabs();
        tabs.move_project(2, "a3", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["p2", "a3"]]);
        assert_eq!(tabs.front_index(), 1);
    }

    #[test]
    fn moving_the_project_a_tab_shows_loads_its_next() {
        let (mut tabs, mut events) = two_tabs();
        tabs.focus(0);
        switch(&mut tabs, TWO, "a1");
        let _ = sent(&mut events);

        tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
        assert_eq!(tabs.front_index(), 0, "the tab in front stays");
        assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
        tabs.forward_events();
        assert!(sent(&mut events).is_empty(), "asked once");

        switch(&mut tabs, TWO, "p2");
        assert_eq!(shown(&tabs), ["p2", "a3"]);
        assert_eq!(held(&tabs), [vec!["p2"], vec!["a1", "a1.1", "a3"]]);
        assert_eq!(tabs.front_index(), 0);
        tabs.forward_events();
        assert!(sent(&mut events).is_empty());
    }

    #[test]
    fn moves_that_cannot_be_done_are_ignored() {
        let (mut tabs, mut events) = two_tabs();
        let before = (tabs.arrangement(), held(&tabs).concat().len());
        let cases = [
            (0, "gone", WorkspaceTarget::Existing(1), "unknown key"),
            (1, "a1", WorkspaceTarget::Existing(0), "not in the tab"),
            (0, "a1", WorkspaceTarget::Existing(0), "to the tab it is in"),
            (0, "a1", WorkspaceTarget::Existing(2), "to no tab"),
            (1, "a3", WorkspaceTarget::New, "the tab's only project"),
            (2, "a3", WorkspaceTarget::Existing(0), "from no tab"),
        ];
        for (from, key, to, why) in cases {
            tabs.move_project(from, key, to);
            assert_eq!(
                (tabs.arrangement(), held(&tabs).concat().len()),
                before,
                "{why}"
            );
            assert_eq!(tabs.front_index(), 1, "{why}");
            assert!(sent(&mut events).is_empty(), "{why}");
        }
    }

    /// The index each tab's UI was last told it has among the tabs, with
    /// the titles it was told.
    fn places(tabs: &Tabs) -> Vec<(usize, Vec<String>)> {
        tabs.iter()
            .map(|tab| {
                let (labels, current) = tab.tui.workspaces();
                (current, labels.to_vec())
            })
            .collect()
    }

    /// Each tab's UI knows every tab's title and its own place.
    fn assert_places(tabs: &Tabs) {
        let titles: Vec<String> = tabs.iter().map(Tab::title).collect();
        let want: Vec<_> = (0..tabs.len()).map(|i| (i, titles.clone())).collect();
        assert_eq!(places(tabs), want);
    }

    #[test]
    fn each_tab_knows_its_place_as_tabs_open_close_and_move() {
        let (mut tabs, _events) = tabs();
        start(&mut tabs, TWO, "a1");
        assert_eq!(places(&tabs), [(0, vec!["A1".to_string()])]);

        let (mut tabs, _events) = three_tabs();
        assert_places(&tabs);
        assert_eq!(places(&tabs)[0].1, ["", "", "A3"]);

        tabs.move_tab(2, 0);
        assert_places(&tabs);
        assert_eq!(places(&tabs)[1].1, ["A3", "", ""]);

        tabs.move_project(0, "a3", WorkspaceTarget::New);
        assert_eq!(tabs.len(), 3, "a3 was alone: nothing moves");
        tabs.move_project(1, "a1", WorkspaceTarget::Existing(2));
        assert_eq!(tabs.len(), 2);
        assert_places(&tabs);

        tabs.move_project(1, "a1", WorkspaceTarget::New);
        assert_eq!(tabs.len(), 3);
        assert_places(&tabs);

        assert!(tabs.close(0));
        assert_places(&tabs);
        assert_eq!(places(&tabs).len(), 2);

        // A tab opened for a new project knows its place too.
        tabs.focus(0);
        plus(&mut tabs);
        let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
        switch(&mut tabs, more, "p4");
        assert_eq!(tabs.len(), 3);
        assert_places(&tabs);
    }

    /// The UI of the tab at `index`, drawn 120 columns by 30 rows.
    fn screen(tabs: &mut Tabs, index: usize) -> Vec<String> {
        testing::screen(&mut tabs.tabs[index].tui)
    }

    use testing::cell_of;

    fn click(tabs: &mut Tabs, index: usize, button: MouseButton, cell: (u16, u16)) {
        testing::click(&mut tabs.tabs[index].tui, button, cell);
    }

    /// Right-click `name` in the projects list of the tab at `index`,
    /// then pick `entry` from its "Move to workspace".
    fn pick_move(tabs: &mut Tabs, index: usize, name: &str, entry: &str) {
        testing::pick_move(&mut tabs.tabs[index].tui, name, entry);
    }

    #[test]
    fn the_menu_of_a_tab_moves_a_project_to_another() {
        let (mut tabs, mut events) = two_tabs();
        // `two_tabs` picked its folder without the browser `+` opened.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        tabs.tabs[0].tui.handle_key(esc);
        tabs.focus(0);
        switch(&mut tabs, TWO, "a1");
        tabs.forward_events();
        let _ = sent(&mut events);

        // The menu offers the other tab, by its place and title.
        let rows = screen(&mut tabs, 0);
        click(&mut tabs, 0, MouseButton::Right, cell_of(&rows, "P2"));
        let rows = screen(&mut tabs, 0);
        click(
            &mut tabs,
            0,
            MouseButton::Left,
            cell_of(&rows, "Move to workspace"),
        );
        let rows = screen(&mut tabs, 0);
        assert!(
            rows.iter().any(|row| row.contains("Workspace 2  A3")),
            "{}",
            rows.join("\n")
        );
        click(
            &mut tabs,
            0,
            MouseButton::Left,
            cell_of(&rows, "Workspace 2"),
        );
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);

        tabs.forward_events();
        assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3", "p2"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["p2", "a3"]]);
        assert_eq!(tabs.front_index(), 0);
        assert!(sent(&mut events).is_empty(), "the session hears nothing");

        // An agent moves its whole project, and its emptied tab closes.
        pick_move(&mut tabs, 0, "A1.1", "Workspace 2");
        tabs.forward_events();
        assert_eq!(tabs.arrangement(), [["a3", "p2", "a1"]]);
        assert_eq!(tabs.front_index(), 0);
    }

    #[test]
    fn moves_asked_in_one_update_go_to_the_tabs_the_menus_named() {
        let (mut tabs, _events) = three_tabs();
        // a1's tab asks to go to a3's, then a3's to p2's: the first move
        // closes a1's tab and shifts the others.
        pick_move(&mut tabs, 0, "A1", "Workspace 3");
        pick_move(&mut tabs, 2, "A3", "Workspace 2");
        tabs.forward_events();
        assert_eq!(tabs.arrangement(), [vec!["p2", "a3"], vec!["a1"]]);
        assert_places(&tabs);
    }

    /// Each update from now on asks the session nothing more.
    fn settled(tabs: &mut Tabs, events: &mut mpsc::Receiver<UiEvent>) {
        for update in 0..3 {
            tabs.forward_events();
            assert!(sent(events).is_empty(), "update {update} asked again");
        }
    }

    #[test]
    fn a_tab_left_with_nothing_to_show_hands_the_front_to_its_moved_agent() {
        // p2 has every agent archived: once a1 goes, its tab shows none.
        let agents: Agents = &[("a1", 0), ("a1.1", 1), ("a3", 0)];
        let (mut tabs, mut events) = tabs_with(&[&["a1", "p2"], &["a3"]]);
        tabs.apply(projects(&["a1", "p2", "a3"]));
        start(&mut tabs, agents, "a1");
        tabs.focus(1);
        switch(&mut tabs, agents, "a3");
        tabs.focus(0);
        switch(&mut tabs, agents, "a1");
        let _ = sent(&mut events);
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        assert_eq!(tabs.front_index(), 0);

        tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
        assert_eq!(tabs.front_index(), 1);
        // The session has a1 already and would send its chat only on a
        // switch, so it turns to a3, which a3's tab shows, on the way.
        assert_eq!(
            sent(&mut events),
            [r#"FocusAgent("a3")"#, r#"SelectAgent("a1")"#]
        );
        settled(&mut tabs, &mut events);

        switch(&mut tabs, agents, "a3");
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        assert_eq!(tabs.front_index(), 1);
        settled(&mut tabs, &mut events);

        // The switch back to a1, with its chat.
        tabs.apply(UiCommand::SetActiveAgent("a1".into()));
        tabs.apply(message("the chat of a1"));
        tabs.apply(list(agents, "a1"));
        tabs.apply(UiCommand::SetActiveAgent("a1".into()));
        assert_eq!(shown(&tabs)[1], "a1");
        assert_eq!(tabs.front_index(), 1);
        let chat = tabs.tabs[1].tui.messages();
        assert!(chat.iter().any(|m| m.content() == "the chat of a1"));
        assert_eq!(held(&tabs), [Vec::<&str>::new(), vec!["a1", "a1.1", "a3"]]);
        settled(&mut tabs, &mut events);

        // The emptied tab, brought forward, asks for nothing it can't show.
        tabs.focus(0);
        settled(&mut tabs, &mut events);
    }

    #[test]
    fn the_session_alone_with_the_moved_agent_shows_it_without_a_switch() {
        let agents: Agents = &[("a1", 0)];
        let (mut tabs, mut events) = tabs_with(&[&["a1", "p2"], &["p3"]]);
        tabs.apply(projects(&["a1", "p2", "p3"]));
        start(&mut tabs, agents, "a1");
        let _ = sent(&mut events);

        tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
        assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["p3", "a1"]]);
        assert_eq!(tabs.front_index(), 1);
        assert_eq!(shown(&tabs)[1], "a1");
        assert!(sent(&mut events).is_empty(), "no agent to turn to");
        settled(&mut tabs, &mut events);
    }

    #[test]
    fn until_the_session_turns_it_speaks_to_the_tab_showing_its_agent() {
        let (mut tabs, mut events) = two_tabs();
        tabs.focus(0);
        switch(&mut tabs, TWO, "a1");
        let _ = sent(&mut events);

        tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
        assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
        // The session still has a1: a1's old tab shows it, the other
        // holds it now.
        assert_eq!(shown(&tabs), ["a1", "a3"]);
        let error = |tab: &Tab, text: &str| matches!(tab.tui.notification(), Some((shown, _)) if shown.contains(text));
        tabs.apply(UiCommand::ShowError("a1 failed".into()));
        assert!(error(&tabs.tabs[0], "a1 failed"));
        assert!(!error(&tabs.tabs[1], "a1 failed"));

        // Turned to p2, it speaks to p2's tab.
        switch(&mut tabs, TWO, "p2");
        assert_eq!(shown(&tabs), ["p2", "a3"]);
        tabs.apply(UiCommand::ShowError("p2 failed".into()));
        assert!(error(&tabs.tabs[0], "p2 failed"));
        assert!(!error(&tabs.tabs[1], "p2 failed"));
        settled(&mut tabs, &mut events);
    }

    #[test]
    fn closing_a_tab_does_the_moves_asked_before_it() {
        // a1's tab asks to go to a3's, which closes it and shifts p2's.
        let (mut tabs, _events) = three_tabs();
        pick_move(&mut tabs, 0, "A1", "Workspace 3");
        assert!(tabs.close(1), "p2's tab");
        assert_eq!(tabs.arrangement(), [["a3", "a1", "p2"]]);
        assert_eq!(tabs.front_index(), 0);

        // The tab closed is the one the move emptied.
        let (mut tabs, _events) = three_tabs();
        pick_move(&mut tabs, 0, "A1", "Workspace 3");
        assert!(tabs.close(0), "closed, emptied");
        assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
        assert_eq!(tabs.front_index(), 1);

        // The move leaves one tab, which stays.
        let (mut tabs, _events) = tabs_with(&[&["a1"], &["a3"]]);
        start(&mut tabs, &[("a1", 0), ("a3", 0)], "a3");
        pick_move(&mut tabs, 0, "A1", "Workspace 2");
        assert!(!tabs.close(1), "the last tab stays");
        assert_eq!(tabs.arrangement(), [["a3", "a1"]]);
        assert_eq!(tabs.front_index(), 0);
    }

    #[test]
    fn a_tabs_own_switch_goes_to_the_session_alone() {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        let _ = sent(&mut events);

        // The tab switched on screen already; nothing is asked for the
        // agent it showed before, which would turn the session there.
        tabs.forward(0, UiEvent::FocusAgent("a1.1".into()));
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1.1")"#]);
        switch(&mut tabs, ONE, "a1.1");

        tabs.forward(0, UiEvent::SelectAgent("a1".into()));
        assert_eq!(sent(&mut events), [r#"SelectAgent("a1")"#]);
        switch(&mut tabs, ONE, "a1");

        // A select of the agent the session has is still sent: its chat
        // comes with the answer.
        tabs.forward(0, UiEvent::SelectAgent("a1".into()));
        assert_eq!(sent(&mut events), [r#"SelectAgent("a1")"#]);
        switch(&mut tabs, ONE, "a1");

        // A focus on it gets its answer here, as the session sends none.
        tabs.forward(0, UiEvent::FocusAgent("a1".into()));
        assert!(sent(&mut events).is_empty());
        assert_eq!(shown(&tabs), ["a1"]);
    }

    #[test]
    fn a_failed_switch_is_no_longer_waited_for() {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        tabs.forward(0, UiEvent::SelectAgent("a1.1".into()));
        assert_eq!(sent(&mut events), [r#"SelectAgent("a1.1")"#]);
        tabs.apply(UiCommand::ShowError("Could not open agent".into()));
        tabs.apply(UiCommand::SwitchFailed("a1.1".into()));

        // The session still has a1, so asking for a1.1 again is sent.
        tabs.forward(0, UiEvent::FocusAgent("a1.1".into()));
        assert_eq!(sent(&mut events), [r#"FocusAgent("a1.1")"#]);
    }
}
