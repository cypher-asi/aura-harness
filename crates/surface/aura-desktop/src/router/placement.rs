//! Placing the session's projects in tabs and showing each tab its lists.
use super::{Arrangement, Tabs};
use crate::tab::Tab;
use aura_ui::events::{
    empty_projects, starts_project, AgentSummary, ProcessSummary, ProjectSummary, UiCommand,
};

impl Tabs {
    /// Keys of the listed projects in list order, with those that have
    /// no agent listed in their places.
    pub(super) fn project_keys(&self) -> Vec<String> {
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

    pub(super) fn place_projects(&mut self) {
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
    pub(super) fn bring_back(&mut self, saved: &Arrangement, roots: &[String]) {
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
    pub(super) fn agents_of(&self, tab: &Tab) -> Vec<AgentSummary> {
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
    pub(super) fn projects_of(&self, tab: &Tab) -> Vec<ProjectSummary> {
        self.projects
            .iter()
            .filter(|project| tab.projects.contains(&project.id))
            .cloned()
            .collect()
    }

    pub(super) fn send_agents(&mut self) {
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
    pub(super) fn sync_workspaces(&mut self) {
        let titles: Vec<String> = self.tabs.iter().map(Tab::title).collect();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.tui.set_workspaces(titles.clone(), index);
        }
    }

    /// Show each tab the processes of its own agents, and those of none.
    pub(super) fn send_processes(&mut self) {
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
    pub(super) fn session_switched(&mut self, agent: String) {
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
    pub(super) fn open(&mut self) -> usize {
        let mut tab = Tab::new(self.next_id, self.settings);
        self.next_id += 1;
        for command in self.lists.iter().flatten() {
            tab.tui.process_command(command.clone());
        }
        self.tabs.push(tab);
        self.tabs.len() - 1
    }
}
