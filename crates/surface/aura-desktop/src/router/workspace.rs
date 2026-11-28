//! Opening, closing and moving tabs, and moving projects between them.
use super::Tabs;
use aura_ui::events::{AgentSummary, UiCommand};
use aura_ui_terminal::WorkspaceTarget;
use std::path::Path;

impl Tabs {
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
    pub(super) fn tab_of_folder(&self, folder: &Path) -> Option<(usize, Option<String>)> {
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
    pub(super) fn move_project(&mut self, from: usize, key: &str, to: WorkspaceTarget) {
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
    pub(super) fn load_into(&mut self, index: usize, agent: &str) {
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
    pub(super) fn stand_in(&self, index: usize, agent: &str) -> Option<String> {
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
    pub(super) fn target_now(&self, told: &[u64], to: WorkspaceTarget) -> Option<WorkspaceTarget> {
        match to {
            WorkspaceTarget::Existing(index) => {
                let id = *told.get(index)?;
                let now = self.tabs.iter().position(|tab| tab.id == id)?;
                Some(WorkspaceTarget::Existing(now))
            }
            WorkspaceTarget::New => Some(WorkspaceTarget::New),
        }
    }

    /// The agent the tab at `index` was last left on in its projects
    /// (see `selection`), else the first agent listed of them.
    pub(super) fn first_agent(&self, index: usize) -> Option<String> {
        if let Some(agent) = self.remembered_in(index) {
            return Some(agent);
        }
        self.tabs.get(index)?.projects.iter().find_map(|key| {
            self.agents
                .iter()
                .find(|agent| agent.project_key() == key)
                .map(|agent| agent.id.clone())
        })
    }
}
