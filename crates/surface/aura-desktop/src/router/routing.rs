//! Commands from the session, routed to the tabs they are for.
use super::{Tabs, MAX_HELD};
use aura_ui::events::{AgentSummary, UiCommand};

impl Tabs {
    /// Apply a command from the session to the tabs it is for.
    pub fn apply(&mut self, command: UiCommand) {
        match command {
            UiCommand::SetAgents(agents) => self.listed(agents),
            command => self.route(command),
        }
    }

    pub(super) fn route(&mut self, command: UiCommand) {
        match &mut self.unlisted {
            Some((_, held)) if held.len() < MAX_HELD => held.push(command),
            Some(_) => {
                self.release();
                self.deliver(command);
            }
            None => self.deliver(command),
        }
    }

    pub(super) fn deliver(&mut self, command: UiCommand) {
        if matches!(command, UiCommand::ForAgent { .. }) {
            self.deliver_for_agent(command);
            return;
        }
        match command {
            UiCommand::SetActiveAgent(agent) => self.active_reported(agent),
            UiCommand::SwitchFailed(agent) => self.switch_failed(agent),
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
            | UiCommand::SetApiStatus { .. } => self.deliver_to_all(command),
            other => self.deliver_to_active(other),
        }
    }

    /// A command addressed to an agent goes to the tab holding it.
    fn deliver_for_agent(&mut self, command: UiCommand) {
        let UiCommand::ForAgent {
            agent_id,
            command: inner,
        } = &command
        else {
            return;
        };
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
    }

    /// The session reported `agent` as its active one.
    fn active_reported(&mut self, agent: String) {
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

    fn switch_failed(&mut self, agent: String) {
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

    /// A list every tab shows whole, kept for tabs opened later.
    fn deliver_to_all(&mut self, command: UiCommand) {
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

    /// A command without an address goes to the tab of the active agent.
    fn deliver_to_active(&mut self, command: UiCommand) {
        if matches!(command, UiCommand::ShowError(_)) {
            // What a tab or `+` asked for may have failed; a later
            // project must not be taken for the answer.
            self.opening = false;
            self.asker = None;
        }
        let index = self.active_index();
        self.tabs[index].tui.process_command(command);
    }

    pub(super) fn broadcast(&mut self, command: UiCommand) {
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
    pub(super) fn listed(&mut self, agents: Vec<AgentSummary>) {
        self.agents = agents;
        self.place_projects();
        self.send_agents();
        self.send_processes();
        self.release();
    }

    /// Stop waiting for the session to list the agent it switched to.
    pub(super) fn release(&mut self) {
        if let Some((agent, held)) = self.unlisted.take() {
            self.session_switched(agent);
            for command in held {
                self.route(command);
            }
        }
    }
}
