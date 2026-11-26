//! The agents, projects and processes the session lists, and commands for
//! one agent's chat.

use crate::app::App;
use crate::events::{AgentSummary, ProjectSummary, UiCommand};
use std::collections::HashSet;

impl App {
    /// Apply a command about the listed agents, projects or processes, or
    /// one for an agent's chat; any other comes back.
    pub(super) fn agent_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::ForAgent { agent_id, command } => {
                self.process_agent_command(&agent_id, *command);
            }
            UiCommand::SetPlacement { agent_id, state } => self.cmd_set_placement(agent_id, state),
            UiCommand::SetHost(view) => self.cmd_set_host(*view),
            UiCommand::SetProjects(projects) => self.cmd_set_projects(projects),
            UiCommand::SetAgents(agents) => self.cmd_set_agents(agents),
            UiCommand::SetArchivedAgents(agents) => self.set_archived(agents),
            UiCommand::SetActiveAgent(agent_id) => self.cmd_set_active_agent(&agent_id),
            UiCommand::SwitchFailed(agent_id) => self.cmd_switch_failed(&agent_id),
            UiCommand::SetAgentDefinitions(definitions) => self.set_definitions(definitions),
            UiCommand::SetProcesses(processes) => self.set_processes(processes),
            other => return Some(other),
        }
        None
    }

    fn cmd_set_projects(&mut self, projects: Vec<ProjectSummary>) {
        self.project_list.projects = projects;
        self.projects_changed_under_channel_form();
    }

    fn cmd_set_agents(&mut self, agents: Vec<AgentSummary>) {
        self.agents = agents;
        self.project_list
            .collapsed_projects
            .retain(|key| self.agents.iter().any(|agent| agent.project_key() == key));
        // An agent that lost its spawns drops its fold, so a new spawn
        // under it later shows up instead of hiding.
        let foldable: HashSet<&str> = (0..self.agents.len())
            .filter(|&i| self.agent_has_spawns(i))
            .map(|i| self.agents[i].id.as_str())
            .collect();
        self.project_list
            .collapsed_agents
            .retain(|id| foldable.contains(id.as_str()));
        self.select_visible_active();
        self.refresh_side_scope();
    }
}
