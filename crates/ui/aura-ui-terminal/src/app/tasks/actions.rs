//! What the TASKS tab's rows open, and clicks and keys on them.

use super::TaskRow;
use crate::app::processes::on_row_icon;
use crate::app::{contains, App, NotificationType, SideScope};
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// Keys of the TASKS tree that would otherwise go to the input, used
    /// while it is empty: Home/End jump, Space folds, `p` opens the row's
    /// plan and `t` highlights the task doing its todo. Returns whether
    /// `key` was one.
    pub(in crate::app) fn handle_tasks_shortcut(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Home => self.task_list.select(0),
            KeyCode::End => self.task_list.select(usize::MAX),
            KeyCode::Char(' ') => self.task_list.toggle_selected(),
            KeyCode::Char('p') => self.open_selected_row_plan(),
            KeyCode::Char('t') => self.jump_to_todo_task(),
            _ => return false,
        }
        true
    }

    /// Open the highlighted row: a plan's file, at the todo for a todo;
    /// the agent of a task, of the request an item is on, or of a round's
    /// first request. The No Plan header has nothing to open, so it folds.
    pub(super) fn open_selected_task_row(&mut self) {
        match self.task_list.rows.get(self.task_list.selected) {
            Some(TaskRow::Plan { plan: None, .. }) => self.task_list.toggle_selected(),
            Some(TaskRow::Plan { .. } | TaskRow::Todo { .. }) => self.open_selected_plan(),
            Some(TaskRow::Task { .. } | TaskRow::Item { .. } | TaskRow::Round { .. }) => {
                self.open_selected_task_agent();
            }
            None => {}
        }
    }

    /// Show the agent doing the highlighted task in the chat. A member of
    /// the open channel opens as its MEMBERS row does, as a member's chat
    /// in the channel; any other agent takes the agent app to it.
    pub(in crate::app) fn open_selected_task_agent(&mut self) {
        let Some(task) = self.task_list.selected_task() else {
            return;
        };
        if task.agent_id.is_empty() {
            return;
        }
        if task.agent_id == self.active_agent_id {
            self.notifications.current = Some((
                format!("{} is already open", task.agent_name),
                NotificationType::Success,
            ));
            return;
        }
        let target = task.agent_id.clone();
        let member = self
            .open_channel()
            .and_then(|channel| channel.members.iter().position(|m| m.id == target));
        match member {
            Some(index) => self.open_channel_member(index),
            None => self.request_agent_switch(target),
        }
    }

    /// Open the plan of the highlighted row: the plan itself on a header
    /// or todo; on a task, or an item of a request, the plan it works on,
    /// at the todo it was handed, else its group's plan. The No Plan group
    /// has none.
    pub(in crate::app) fn open_selected_row_plan(&mut self) {
        if self.task_list.selected_plan().is_some() {
            self.open_selected_plan();
            return;
        }
        let own = self
            .task_list
            .selected_task()
            .and_then(|task| Some((task.plan.clone()?, task.plan_todo.clone())));
        let group = || {
            self.task_list
                .selected_group_plan()
                .map(|plan| (plan.path.clone(), None))
        };
        match own.or_else(group) {
            Some((plan, todo)) => self.open_plan_at(&plan, todo.as_deref()),
            None => self.warn("No plan to open"),
        }
    }

    /// Highlight the task doing the highlighted todo or item, or say why
    /// it can't: the row is neither, no task does it, it is the plan's
    /// build, the scope hides the task, or the task is not loaded.
    pub(in crate::app) fn jump_to_todo_task(&mut self) {
        let list = &self.task_list;
        let task = match list.rows.get(list.selected) {
            Some(&TaskRow::Todo { plan, todo, .. }) => list
                .plans
                .get(plan)
                .and_then(|plan| plan.todos.get(todo))
                .and_then(|todo| todo.task.clone()),
            Some(&TaskRow::Item {
                task, item, link, ..
            }) => link
                .and_then(|link| list.tasks.get(link))
                .map(|linked| linked.id.clone())
                .or_else(|| list.item(task, item).and_then(|item| item.task.clone())),
            _ => {
                self.warn("Select a todo to jump to its task");
                return;
            }
        };
        let Some(task) = task else {
            self.warn("No task is doing this todo");
            return;
        };
        if list.is_hidden_build(&task) {
            self.warn("This todo is part of the plan's build");
            return;
        }
        if self.task_list.select_task(&task) {
            return;
        }
        let list = &self.task_list;
        let is_loaded = list.tasks().iter().any(|listed| listed.id == task);
        let is_scoped = list.scoped_tasks().any(|listed| listed.id == task);
        if is_loaded && !is_scoped {
            self.warn(format!(
                "Another agent is doing this todo; the {} button lists it",
                SideScope::BUTTON
            ));
        } else {
            self.warn("That todo's task isn't loaded");
        }
    }

    /// Show `message` as a warning.
    fn warn(&mut self, message: impl Into<String>) {
        self.notifications.current = Some((message.into(), NotificationType::Warning));
    }

    /// Clicking a row's fold glyph folds or opens what is under it;
    /// clicking anywhere else on a row opens it as Enter does.
    pub(in crate::app) fn click_task_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.task_row_at(column, row) else {
            return;
        };
        self.task_list.select(index);
        let Some(&target) = self.task_list.rows.get(index) else {
            return;
        };
        let on_fold = target.fold().is_some()
            && self
                .areas
                .task_rows
                .is_some_and(|rows| on_row_icon(column, rows.x, target.depth()));
        if on_fold {
            self.task_list.toggle_selected();
        } else {
            self.open_selected_task_row();
        }
    }

    pub(in crate::app) fn task_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .task_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.tasks_scroll + usize::from(row - rows.y);
        (index < self.task_list.rows.len()).then_some(index)
    }
}
