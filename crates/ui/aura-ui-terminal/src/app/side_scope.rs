//! Whose items the side panel's PROCESSES, TRACE and TASKS tabs list: the
//! active agent's, with the agents it spawned, or every agent's in its
//! project. The TASKS tab lists the plans those agents' tasks work on.
//! The `*` button at the right end of the panel's header bar switches
//! every tab at once.

use std::collections::HashSet;

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
        self.side_scope
    }

    /// Show the whole project's items in the side panel's lists, or only
    /// the active agent's again.
    pub fn toggle_side_scope(&mut self) {
        self.side_scope = self.side_scope.toggled();
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
            return match self.side_scope {
                SideScope::Agent => Some(HashSet::from([self.active_agent_id.clone()])),
                SideScope::Project => None,
            };
        };
        let agents = &self.agents;
        let range = match self.side_scope {
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
    /// run again.
    pub(super) fn refresh_side_scope(&mut self) {
        let agents = self.scope_agents();
        self.trace_feed.set_filter(agents.clone());
        self.task_list.set_filter(agents.clone());
        self.process_filter = agents;
        self.reshow_processes();
        self.refresh_members();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        AgentSummary, PlanView, ProcessKind, TaskView, TodoState, TodoView, TraceNode, UiCommand,
    };

    fn agent(id: &str, project: &str, depth: usize) -> AgentSummary {
        AgentSummary {
            id: id.into(),
            name: id.to_uppercase(),
            project_id: Some(project.into()),
            depth,
            ..AgentSummary::default()
        }
    }

    fn process(id: &str, owner: &str) -> ProcessSummary {
        ProcessSummary {
            id: id.into(),
            kind: ProcessKind::Command,
            label: id.into(),
            owner_agent_id: owner.into(),
            jump_agent_id: owner.into(),
            ..ProcessSummary::default()
        }
    }

    fn task(id: &str, agent: &str, parent: Option<&str>, plan: Option<&str>) -> TaskView {
        TaskView {
            id: id.into(),
            title: id.into(),
            agent_id: agent.into(),
            parent: parent.map(str::to_string),
            plan: plan.map(str::to_string),
            ..TaskView::default()
        }
    }

    fn plan(path: &str, todo_task: Option<&str>) -> PlanView {
        PlanView {
            path: path.into(),
            name: path.into(),
            todos: vec![TodoView {
                content: "step".into(),
                state: TodoState::Pending,
                assignee: None,
                task: todo_task.map(str::to_string),
            }],
        }
    }

    fn step(agent: &str, id: &str) -> UiCommand {
        UiCommand::ForAgent {
            agent_id: agent.into(),
            command: Box::new(UiCommand::GraphUpsert {
                run_id: format!("run-{agent}"),
                nodes: vec![TraceNode {
                    id: id.into(),
                    kind: "tool_call".into(),
                    ..TraceNode::default()
                }],
            }),
        }
    }

    /// Project `site` with `a` (which spawned `ac`) and `b`; project
    /// `notes` with `n`.
    fn app() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![
            agent("a", "site", 0),
            agent("ac", "site", 1),
            agent("b", "site", 0),
            agent("n", "notes", 0),
        ]));
        app.process_command(UiCommand::SetActiveAgent("a".into()));
        app.process_command(UiCommand::SetProcesses(vec![
            process("pa", "a"),
            process("pac", "ac"),
            process("pb", "b"),
            process("pn", "n"),
            process("none", ""),
        ]));
        app.process_command(UiCommand::SetTasks(vec![
            task("ta", "a", None, Some("plan-a")),
            task("tac", "ac", Some("ta"), None),
            task("tb", "b", None, Some("plan-b")),
        ]));
        app.process_command(UiCommand::SetPlans(vec![
            plan("plan-a", None),
            plan("plan-b", None),
            plan("plan-c", Some("tac")),
        ]));
        for (agent, id) in [("a", "sa"), ("b", "sb"), ("ac", "sac"), ("n", "sn")] {
            app.process_command(step(agent, id));
        }
        app
    }

    fn processes(app: &App) -> Vec<&str> {
        app.processes().iter().map(|p| p.id.as_str()).collect()
    }

    fn steps(app: &App) -> Vec<&str> {
        app.trace_feed()
            .entries()
            .map(|e| e.node.id.as_str())
            .collect()
    }

    fn tasks(app: &App) -> Vec<&str> {
        let list = app.task_list();
        list.scoped_tasks().map(|t| t.id.as_str()).collect()
    }

    fn plans(app: &App) -> Vec<&str> {
        let list = app.task_list();
        list.rows()
            .iter()
            .filter_map(|row| match *row {
                crate::app::tasks::TaskRow::Plan {
                    plan: Some(plan), ..
                } => Some(list.plans()[plan].path.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_lists_show_the_active_agent_and_its_spawns() {
        let app = app();
        assert_eq!(app.side_scope(), SideScope::Agent);
        assert_eq!(processes(&app), ["pa", "pac", "none"]);
        assert_eq!(steps(&app), ["sa", "sac"]);
        assert_eq!(tasks(&app), ["ta", "tac"]);
        assert_eq!(plans(&app), ["plan-a", "plan-c"]);
        assert_eq!(app.task_list().hidden(), 1);
        assert_eq!(app.task_list().hidden_plans(), 1);
    }

    #[test]
    fn all_shows_the_whole_project_and_no_other() {
        let mut app = app();
        app.toggle_side_scope();
        assert_eq!(app.side_scope(), SideScope::Project);
        assert_eq!(processes(&app), ["pa", "pac", "pb", "none"]);
        assert_eq!(steps(&app), ["sa", "sb", "sac"]);
        assert_eq!(tasks(&app), ["ta", "tac", "tb"]);
        assert_eq!(plans(&app), ["plan-a", "plan-b", "plan-c"]);

        app.toggle_side_scope();
        assert_eq!(processes(&app), ["pa", "pac", "none"]);
    }

    #[test]
    fn the_lists_follow_the_active_agent() {
        let mut app = app();
        app.process_command(UiCommand::SetActiveAgent("b".into()));
        assert_eq!(processes(&app), ["pb", "none"]);
        assert_eq!(steps(&app), ["sb"]);
        assert_eq!(tasks(&app), ["tb"]);
        assert_eq!(plans(&app), ["plan-b"]);

        app.process_command(UiCommand::SetActiveAgent("ac".into()));
        assert_eq!(processes(&app), ["pac", "none"]);
        assert_eq!(steps(&app), ["sac"]);
        assert_eq!(tasks(&app), ["tac"]);

        app.toggle_side_scope();
        app.process_command(UiCommand::SetActiveAgent("n".into()));
        assert_eq!(processes(&app), ["pn", "none"]);
        assert_eq!(steps(&app), ["sn"]);
    }

    #[test]
    fn a_new_spawn_joins_its_parents_lists() {
        let mut app = app();
        app.process_command(UiCommand::SetAgents(vec![
            agent("a", "site", 0),
            agent("ac", "site", 1),
            agent("b", "site", 0),
            agent("bc", "site", 1),
            agent("n", "notes", 0),
        ]));
        app.process_command(UiCommand::SetActiveAgent("b".into()));
        app.process_command(step("bc", "sbc"));
        assert_eq!(steps(&app), ["sb", "sbc"]);
    }

    #[test]
    fn the_highlight_stays_on_its_row_across_the_switch() {
        let mut app = app();
        app.selected_process = 1;
        app.toggle_side_scope();
        assert_eq!(
            app.selected_process_data().map(|p| p.id.as_str()),
            Some("pac")
        );
        app.toggle_side_scope();
        assert_eq!(
            app.selected_process_data().map(|p| p.id.as_str()),
            Some("pac")
        );
    }

    #[test]
    fn processes_of_no_agent_are_always_shown() {
        let none = process("x", "");
        let agents = HashSet::from(["a".to_string()]);
        assert!(process_in_scope(&none, &agents));
        let mut spawned = process("y", "child");
        assert!(!process_in_scope(&spawned, &agents));
        spawned.parent_chain = vec!["a".into()];
        assert!(process_in_scope(&spawned, &agents));
    }
}
