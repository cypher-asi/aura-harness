//! The window's tabs, and how one agent session serves all of them.
//!
//! A tab is a workspace: it holds some of the session's projects (a root
//! agent and the agents under it) and shows only those, in its own copy
//! of the terminal UI. The session knows nothing of tabs. It has one
//! event loop, one command stream and one "active" agent, whose Record
//! panel, sandbox and notifications it sends without an address, so the
//! agent ties a command to a tab:
//!
//! - The agent list is cut down to each tab's projects, and commands
//!   addressed to an agent go to the tab that holds it.
//! - Lists that aren't about agents (models, definitions) go to every tab.
//! - Anything else goes to the tab holding the session's active agent.
//! - Before a tab's events are sent, the session is asked to make that
//!   tab's agent the active one, so the answers come back to it.
//!
//! Invariant: every project is in exactly one tab. A new one joins the
//! tab that asked for it; `+` asks for one in a tab of its own.

use crate::tab::{Settings, Tab};
use aura_ui::events::{AgentSummary, ProcessSummary, UiCommand, UiEvent};
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
    /// Every process of the session, as last listed.
    processes: Vec<ProcessSummary>,
    /// The latest of each list all tabs show whole, for tabs opened later.
    lists: [Option<UiCommand>; 4],
    /// The session's active agent, as it last reported.
    session_active: String,
    /// Agent the session was asked to make active and hasn't confirmed.
    focus_sent: Option<String>,
    /// `+` was pressed and the agent it asked for is yet to be listed.
    opening: bool,
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
            processes: Vec::new(),
            lists: [None, None, None, None],
            session_active: String::new(),
            focus_sent: None,
            opening: false,
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

    /// The root agent heading the project `agent` is in.
    fn root_of(&self, agent: &str) -> Option<&str> {
        let mut root = None;
        for listed in &self.agents {
            if listed.heads_project() {
                root = Some(listed.id.as_str());
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

    /// The tab that gets what the session sends without an address.
    fn active_index(&self) -> usize {
        self.owner(&self.session_active).unwrap_or(self.front)
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
        if let UiCommand::ForAgent { agent_id, .. } = &command {
            match self.owner(agent_id) {
                Some(index) => self.tabs[index].tui.process_command(command),
                // Not listed yet: a tab's UI keeps it until it is.
                None => self.broadcast(command),
            }
            return;
        }
        match command {
            UiCommand::SetActiveAgent(agent) => {
                if self.owner(&agent).is_some() {
                    self.session_switched(agent);
                } else {
                    // A new agent: the list that names it follows.
                    self.unlisted = Some((agent, Vec::new()));
                }
            }
            UiCommand::SetProcesses(processes) => {
                self.processes = processes;
                self.send_processes();
            }
            // Each tab's GRAPH shows the flow of its own agent and ignores
            // the others'.
            UiCommand::GraphUpsert { .. }
            | UiCommand::FlowLoad { .. }
            | UiCommand::FlowUpsert { .. }
            | UiCommand::EndLink
            | UiCommand::SetAgentDefinitions(_)
            | UiCommand::SetArchivedAgents(_)
            | UiCommand::SetModels { .. }
            | UiCommand::SetApiStatus { .. } => {
                let slot = match command {
                    UiCommand::SetAgentDefinitions(_) => Some(0),
                    UiCommand::SetModels { .. } => Some(1),
                    UiCommand::SetApiStatus { .. } => Some(2),
                    UiCommand::SetArchivedAgents(_) => Some(3),
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

    fn place_projects(&mut self) {
        let roots: Vec<String> = self
            .agents
            .iter()
            .filter(|agent| agent.heads_project())
            .map(|agent| agent.id.clone())
            .collect();
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
    /// agent the tab shows rather than on the session's.
    fn agents_of(&self, tab: &Tab) -> Vec<AgentSummary> {
        let mut out = Vec::new();
        let mut held = false;
        for agent in &self.agents {
            if agent.heads_project() {
                held = tab.projects.contains(&agent.id);
            }
            if held {
                let mut agent = agent.clone();
                if !tab.agent().is_empty() {
                    agent.is_active = agent.id == tab.agent();
                }
                out.push(agent);
            }
        }
        out
    }

    fn send_agents(&mut self) {
        let lists: Vec<_> = self.tabs.iter().map(|tab| self.agents_of(tab)).collect();
        for (tab, agents) in self.tabs.iter_mut().zip(lists) {
            tab.tui.process_command(UiCommand::SetAgents(agents));
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
        let index = self.owner(&agent).unwrap_or(self.front);
        if self.focus_sent.is_none() && self.front().agent().is_empty() {
            self.front = index;
        }
        self.tabs[index]
            .tui
            .process_command(UiCommand::SetActiveAgent(agent));
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

    /// `+`: ask the session for a new agent, which gets a tab of its own
    /// once the session lists it.
    pub fn new_tab(&mut self) {
        self.forward_events();
        self.opening = true;
        self.asker = None;
        self.send(UiEvent::CreateAgent(None));
    }

    /// Bring the tab at `index` to the front.
    pub fn focus(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.front = index;
            self.sync_focus();
        }
    }

    pub fn next(&mut self) {
        self.focus((self.front + 1) % self.tabs.len());
    }

    pub fn previous(&mut self) {
        self.focus((self.front + self.tabs.len() - 1) % self.tabs.len());
    }

    /// Close the tab at `index`. Its projects move to the tab before it
    /// (after it, for the first), so nothing is lost. Returns `false` for
    /// the last tab, which stays.
    pub fn close(&mut self, index: usize) -> bool {
        if self.tabs.len() < 2 || index >= self.tabs.len() {
            return false;
        }
        self.forward_events();
        let closed = self.tabs.remove(index);
        let into = index.saturating_sub(1);
        self.tabs[into].projects.extend(closed.projects);
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
    /// then make sure it follows the tab in front.
    pub fn forward_events(&mut self) {
        for index in 0..self.tabs.len() {
            while let Some(event) = self.tabs[index].next_event() {
                self.forward(index, event);
            }
        }
        self.sync_focus();
    }

    fn forward(&mut self, from: usize, event: UiEvent) {
        match &event {
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
        let agent = self.tabs[from].agent().to_string();
        self.request(&agent, false);
        self.send(event);
    }

    /// Make the session follow the tab in front. A tab that has shown
    /// nothing yet opens on its first project.
    fn sync_focus(&mut self) {
        let tab = self.front();
        if tab.agent().is_empty() {
            if let Some(project) = tab.projects.first().cloned() {
                self.request(&project, true);
            }
        } else {
            let agent = tab.agent().to_string();
            self.request(&agent, false);
        }
    }

    /// Ask the session to make `agent` its active agent, unless it is or
    /// was already asked to. `load` asks for the agent's chat as well,
    /// for a tab that doesn't hold it yet.
    fn request(&mut self, agent: &str, load: bool) {
        if agent.is_empty() {
            return;
        }
        let asked = match &self.focus_sent {
            Some(sent) => sent == agent,
            None => self.session_active == agent,
        };
        if !asked {
            self.focus_sent = Some(agent.to_string());
            self.send(if load {
                UiEvent::SelectAgent(agent.to_string())
            } else {
                UiEvent::FocusAgent(agent.to_string())
            });
        }
    }

    fn send(&self, event: UiEvent) {
        if let Err(e) = self.session.try_send(event) {
            tracing::warn!(error = %e, "the agent session did not take an event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui::events::{MessageData, MessageRole};
    use aura_ui_terminal::AlertSettings;

    fn tabs_with(restore: &[&[&str]]) -> (Tabs, mpsc::Receiver<UiEvent>) {
        let (session, events) = mpsc::channel(64);
        let settings = Settings {
            verbose: false,
            alerts: AlertSettings::from_env(),
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

    fn message(text: &str) -> UiCommand {
        UiCommand::ShowMessage(MessageData::new(MessageRole::System, text))
    }

    fn messages(tabs: &Tabs) -> Vec<usize> {
        tabs.iter().map(|tab| tab.tui.messages().len()).collect()
    }

    const ONE: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0)];
    const TWO: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0)];

    /// Two tabs: every project of [`ONE`], and `a3` opened with `+` and
    /// in front.
    fn two_tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        tabs.new_tab();
        switch(&mut tabs, TWO, "a3");
        let _ = sent(&mut events);
        (tabs, events)
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
    fn plus_opens_a_tab_holding_the_new_agent_only() {
        let (mut tabs, mut events) = tabs();
        start(&mut tabs, ONE, "a1");
        tabs.new_tab();
        assert_eq!(sent(&mut events), ["CreateAgent(None)"]);
        assert_eq!(tabs.len(), 1, "no tab until the agent exists");

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
            name: "docs".into(),
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
        agents.insert(
            2,
            AgentSummary {
                id: "a1.2".into(),
                name: "A1.2".into(),
                added_to_project: true,
                ..AgentSummary::default()
            },
        );
        tabs.apply(UiCommand::SetAgents(agents));
        assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
        assert_eq!(held(&tabs), [vec!["a1", "a1.1", "a1.2", "p2"], vec!["a3"]]);
    }

    #[test]
    fn a_failed_create_opens_no_tab() {
        let (mut tabs, _events) = tabs();
        start(&mut tabs, ONE, "a1");
        tabs.new_tab();
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
}
