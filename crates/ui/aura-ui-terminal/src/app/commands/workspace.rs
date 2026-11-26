//! The active agent's workspace: its sandbox tree, files opened in the
//! editor, plans and tasks.

use crate::app::{App, SidePanelTab};
use crate::events::{PlanView, UiCommand};
use std::path::PathBuf;

impl App {
    /// Apply a command about the agent's workspace; any other comes back.
    pub(super) fn workspace_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::SetSandboxRoot(root) => self.set_sandbox_root(root),
            UiCommand::OpenEditor(path) => self.pending.editor_request = Some(path),
            UiCommand::SetPlan(plan) => self.cmd_set_plan(plan),
            UiCommand::SetPlans(plans) => self.task_list.set_plans(plans),
            UiCommand::SetTasks(tasks) => self.task_list.set_tasks(tasks),
            UiCommand::UpsertTask(task) => self.task_list.upsert(task),
            other => return Some(other),
        }
        None
    }

    /// Point the Sandbox tree at `root`. A tree held for it already, as
    /// when its agent comes back on screen, keeps its opened folders,
    /// highlight and scroll, and is read again on the next tick to show
    /// what changed while it was away.
    fn set_sandbox_root(&mut self, root: PathBuf) {
        if self.sandbox.root() == Some(root.as_path()) {
            self.ticks.sandbox_stale = true;
            return;
        }
        self.sandbox.set_root(root);
        self.sandbox_scroll = 0;
    }

    /// Re-read the sandbox tree on the next tick, so files the agent
    /// writes show up.
    pub(super) fn refresh_sandbox(&mut self) {
        self.ticks.sandbox_stale = true;
    }

    fn cmd_set_plan(&mut self, plan: Option<PlanView>) {
        let current = self.active_plan.as_ref().map(|p| p.path.as_str());
        if plan
            .as_ref()
            .is_some_and(|p| Some(p.path.as_str()) != current)
        {
            self.show_side_tab(SidePanelTab::Tasks);
        }
        self.task_list.set_current_plan(plan.as_ref());
        self.active_plan = plan;
    }
}
