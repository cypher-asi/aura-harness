//! The TASKS side tab: every piece of work agents did or are doing in the
//! active agent's workspace, newest first. A user's request is a task, and
//! each piece of it an agent handed to another with `task` is a task under
//! it; each shows what its agent has done and has left. Tasks still being
//! worked on, or left with work to do, start open; the rest start folded.
//! Only the tasks of the agents the side panel's scope lets through are
//! listed, with every task under them.

use std::collections::{HashMap, HashSet};

use super::processes::SIDE_PAGE;
use super::side_scope::AgentFilter;
use super::{contains, follow_selection, App, NotificationType};
use crate::events::TaskView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// One row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRow {
    /// A task's title row, `depth` levels under the task it is part of
    Task {
        task: usize,
        depth: usize,
        open: bool,
    },
    /// An item of an open task
    Item {
        task: usize,
        item: usize,
        depth: usize,
    },
}

impl TaskRow {
    /// Index of the task the row belongs to.
    #[must_use]
    pub const fn task(self) -> usize {
        match self {
            Self::Task { task, .. } | Self::Item { task, .. } => task,
        }
    }
}

/// The tasks and how the tab shows them.
#[derive(Debug, Default)]
pub struct TaskList {
    /// Newest first
    tasks: Vec<TaskView>,
    /// Tasks the user folded (`false`) or opened (`true`), by id
    folds: HashMap<String, bool>,
    /// Agents whose tasks are listed
    filter: AgentFilter,
    /// Which of `tasks` the filter lets through
    scoped: Vec<bool>,
    rows: Vec<TaskRow>,
    selected: usize,
}

impl TaskList {
    /// The tasks, newest first, listed or not.
    #[must_use]
    pub fn tasks(&self) -> &[TaskView] {
        &self.tasks
    }

    /// The tasks listed, newest first.
    pub fn scoped_tasks(&self) -> impl Iterator<Item = &TaskView> {
        self.tasks
            .iter()
            .zip(&self.scoped)
            .filter_map(|(task, &scoped)| scoped.then_some(task))
    }

    /// Tasks of agents the filter leaves out.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.scoped.iter().filter(|&&scoped| !scoped).count()
    }

    /// Whether some agents' tasks are left out, or would be.
    #[must_use]
    pub const fn filtered(&self) -> bool {
        self.filter.is_some()
    }

    /// List only the tasks of the agents `filter` lets through, and the
    /// tasks under those.
    pub fn set_filter(&mut self, filter: AgentFilter) {
        if filter == self.filter {
            return;
        }
        let keep = self.selected_key();
        self.filter = filter;
        self.rebuild(keep);
    }

    /// Rows on screen, in order.
    #[must_use]
    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }

    /// Highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Replace the tasks, keeping the highlight on the same task or item
    /// when it is still listed.
    pub fn set_tasks(&mut self, tasks: Vec<TaskView>) {
        let keep = self.selected_key();
        self.tasks = tasks;
        self.rebuild(keep);
    }

    /// Replace the listed copy of `task`, or add it on top.
    pub fn upsert(&mut self, task: TaskView) {
        let keep = self.selected_key();
        match self.tasks.iter_mut().find(|t| t.id == task.id) {
            Some(listed) => *listed = task,
            None => self.tasks.insert(0, task),
        }
        self.rebuild(keep);
    }

    /// Move the highlight by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    /// Highlight row `index` (clamped).
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// Highlight task `id`, opening the tasks it is part of. Returns
    /// whether it is listed.
    pub fn select_task(&mut self, id: &str) -> bool {
        let mut parent = self.find(id).and_then(|task| task.parent.clone());
        let mut seen = HashSet::new();
        while let Some(id) = parent.filter(|id| seen.insert(id.clone())) {
            self.folds.insert(id.clone(), true);
            parent = self.find(&id).and_then(|task| task.parent.clone());
        }
        self.rebuild(Some((id.to_string(), None)));
        self.selected_task().is_some_and(|task| task.id == id)
    }

    /// The task of the highlighted row.
    #[must_use]
    pub fn selected_task(&self) -> Option<&TaskView> {
        let row = self.rows.get(self.selected)?;
        self.tasks.get(row.task())
    }

    /// Fold or open the task of the highlighted row. Folding from an item
    /// moves the highlight to its task.
    pub fn toggle_selected(&mut self) {
        let Some(task) = self.selected_task() else {
            return;
        };
        let (id, open) = (task.id.clone(), !self.is_open(task));
        self.folds.insert(id.clone(), open);
        self.rebuild(Some((id, None)));
    }

    /// Open (`true`) or fold the task of the highlighted row, when it is
    /// not that way already.
    pub fn set_selected_open(&mut self, open: bool) {
        if self
            .selected_task()
            .is_some_and(|task| self.is_open(task) != open)
        {
            self.toggle_selected();
        }
    }

    fn find(&self, id: &str) -> Option<&TaskView> {
        self.tasks.iter().find(|task| task.id == id)
    }

    fn is_open(&self, task: &TaskView) -> bool {
        self.folds
            .get(&task.id)
            .copied()
            .unwrap_or(!task.state.is_closed())
    }

    /// Id of the highlighted row's task, and its item.
    fn selected_key(&self) -> Option<(String, Option<usize>)> {
        let row = *self.rows.get(self.selected)?;
        let id = self.tasks.get(row.task())?.id.clone();
        let item = match row {
            TaskRow::Task { .. } => None,
            TaskRow::Item { item, .. } => Some(item),
        };
        Some((id, item))
    }

    /// Which tasks the filter lets through: those of its agents, and every
    /// task under one of those.
    fn scope(&self) -> Vec<bool> {
        let Some(agents) = &self.filter else {
            return vec![true; self.tasks.len()];
        };
        let index: HashMap<&str, usize> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(i, task)| (task.id.as_str(), i))
            .collect();
        let mut scoped: Vec<bool> = self
            .tasks
            .iter()
            .map(|task| agents.contains(&task.agent_id))
            .collect();
        loop {
            let mut grew = false;
            for (i, task) in self.tasks.iter().enumerate() {
                let under_scoped = task
                    .parent
                    .as_deref()
                    .and_then(|parent| index.get(parent))
                    .is_some_and(|&parent| scoped[parent]);
                if !scoped[i] && under_scoped {
                    scoped[i] = true;
                    grew = true;
                }
            }
            if !grew {
                return scoped;
            }
        }
    }

    fn rebuild(&mut self, keep: Option<(String, Option<usize>)>) {
        self.scoped = self.scope();
        let listed: HashSet<&str> = self.scoped_tasks().map(|task| task.id.as_str()).collect();
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut tops = Vec::new();
        for (index, task) in self.tasks.iter().enumerate() {
            if !self.scoped[index] {
                continue;
            }
            match task
                .parent
                .as_deref()
                .filter(|parent| listed.contains(parent))
            {
                Some(parent) => children.entry(parent).or_default().push(index),
                None => tops.push(index),
            }
        }
        // Newest first at the top; a task's parts in the order given.
        for parts in children.values_mut() {
            parts.reverse();
        }
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<(usize, usize)> = tops.into_iter().rev().map(|top| (top, 0)).collect();
        while let Some((task, depth)) = stack.pop() {
            if !seen.insert(task) {
                continue;
            }
            let view = &self.tasks[task];
            let open = self.is_open(view);
            rows.push(TaskRow::Task { task, depth, open });
            if !open {
                continue;
            }
            rows.extend((0..view.items.len()).map(|item| TaskRow::Item {
                task,
                item,
                depth: depth + 1,
            }));
            if let Some(parts) = children.get(view.id.as_str()) {
                stack.extend(parts.iter().rev().map(|&part| (part, depth + 1)));
            }
        }
        self.rows = rows;
        let kept = keep.and_then(|(id, item)| {
            let task = self.tasks.iter().position(|t| t.id == id)?;
            self.rows.iter().position(|&row| match (row, item) {
                (TaskRow::Task { task: t, .. }, None) => t == task,
                (
                    TaskRow::Item {
                        task: t, item: i, ..
                    },
                    Some(item),
                ) => t == task && i == item,
                _ => false,
            })
        });
        self.select(kept.unwrap_or(self.selected));
    }
}

impl App {
    /// The TASKS tab's tasks.
    #[must_use]
    pub const fn task_list(&self) -> &TaskList {
        &self.task_list
    }

    /// Keep the highlighted row inside `visible` rows, returning the first
    /// row to draw.
    pub fn sync_tasks_scroll(&mut self, visible: usize) -> usize {
        follow_selection(
            self.areas.tasks_scroll,
            self.task_list.selected,
            visible,
            self.task_list.rows.len(),
        )
    }

    /// Where the task rows were drawn, for mouse clicks.
    pub fn set_task_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.task_rows = Some(area);
        self.areas.tasks_scroll = scroll;
    }

    /// Keys of the TASKS tab: Enter shows the task's agent, Right unfolds
    /// it and Left folds it. `p`, which opens its plan, is a side-list
    /// shortcut.
    pub(super) fn handle_tasks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.task_list.move_by(-1),
            KeyCode::Down => self.task_list.move_by(1),
            KeyCode::PageUp => self.task_list.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.task_list.move_by(SIDE_PAGE),
            KeyCode::Home => self.task_list.select(0),
            KeyCode::End => self.task_list.select(usize::MAX),
            KeyCode::Enter => self.open_selected_task_agent(),
            KeyCode::Right => self.task_list.set_selected_open(true),
            KeyCode::Left => self.task_list.set_selected_open(false),
            _ => {}
        }
    }

    /// Show the agent doing the highlighted task in the chat.
    pub(super) fn open_selected_task_agent(&mut self) {
        let Some(task) = self.task_list.selected_task() else {
            return;
        };
        if task.agent_id.is_empty() {
            return;
        }
        if task.agent_id == self.active_agent_id {
            self.notification = Some((
                format!("{} is already open", task.agent_name),
                NotificationType::Success,
            ));
            return;
        }
        let target = task.agent_id.clone();
        self.request_agent_switch(target);
    }

    /// Open the plan the highlighted task works on, at its todo.
    pub(super) fn open_selected_task_plan(&mut self) {
        let Some(task) = self.task_list.selected_task() else {
            return;
        };
        let Some(plan) = task.plan.clone() else {
            self.notification = Some((
                "This task has no plan".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        let todo = task.plan_todo.clone();
        self.open_plan_at(&plan, todo.as_deref());
    }

    /// Clicking a task's fold glyph folds or opens it; clicking anywhere
    /// else on a row shows its agent.
    pub(super) fn click_task_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.task_row_at(column, row) else {
            return;
        };
        self.task_list.select(index);
        let on_fold = self
            .areas
            .task_rows
            .is_some_and(|rows| match self.task_list.rows[index] {
                TaskRow::Task { depth, .. } => super::processes::on_row_icon(column, rows.x, depth),
                TaskRow::Item { .. } => false,
            });
        if on_fold {
            self.task_list.toggle_selected();
        } else {
            self.open_selected_task_agent();
        }
    }

    pub(super) fn task_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .task_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.tasks_scroll + usize::from(row - rows.y);
        (index < self.task_list.rows.len()).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{TaskState, TodoState, TodoView, UiCommand, UiEvent};
    use tokio::sync::mpsc;

    fn switched(rx: &mut mpsc::Receiver<UiEvent>) -> Option<String> {
        match rx.try_recv() {
            Ok(UiEvent::SelectAgent(id)) => Some(id),
            _ => None,
        }
    }

    fn task(id: &str, parent: Option<&str>, state: TaskState, items: usize) -> TaskView {
        TaskView {
            id: id.to_string(),
            title: format!("do {id}"),
            state,
            agent_id: format!("agent-{id}"),
            agent_name: format!("Agent {id}"),
            parent: parent.map(str::to_string),
            items: (0..items)
                .map(|i| TodoView {
                    content: format!("{id} {i}"),
                    state: TodoState::Pending,
                    assignee: None,
                    task: None,
                })
                .collect(),
            ..TaskView::default()
        }
    }

    fn shape(list: &TaskList) -> Vec<String> {
        list.rows()
            .iter()
            .map(|row| match *row {
                TaskRow::Task { task, depth, open } => format!(
                    "{}{}{}",
                    "  ".repeat(depth),
                    if open { "-" } else { "+" },
                    list.tasks()[task].id
                ),
                TaskRow::Item { task, item, depth } => format!(
                    "{}{}",
                    "  ".repeat(depth),
                    list.tasks()[task].items[item].content
                ),
            })
            .collect()
    }

    use TaskState::{Completed as C, Running as R};

    #[test]
    fn spawned_tasks_sit_under_the_request_in_order_and_done_ones_fold() {
        let mut list = TaskList::default();
        list.set_tasks(vec![
            task("b", Some("a"), C, 1),
            task("c", Some("a"), R, 1),
            task("a", None, R, 1),
            task("old", None, C, 2),
        ]);
        assert_eq!(
            shape(&list),
            ["-a", "  a 0", "  -c", "    c 0", "  +b", "+old"],
            "parts newest first, like the requests"
        );
    }

    #[test]
    fn an_update_keeps_the_highlight_and_a_new_task_goes_on_top() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("a", None, R, 2)]);
        list.select(2);
        list.upsert(task("b", None, R, 0));
        assert_eq!(shape(&list), ["-b", "-a", "  a 0", "  a 1"]);
        assert_eq!(list.selected(), 3, "still on a's second item");

        list.upsert(task("a", None, C, 2));
        assert_eq!(shape(&list), ["-b", "+a"], "a closed and folds");
        list.select(1);
        list.toggle_selected();
        assert_eq!(shape(&list).len(), 4, "the user opens it again");
    }

    #[test]
    fn selecting_a_folded_part_opens_its_request() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("b", Some("a"), C, 0), task("a", None, C, 0)]);
        assert_eq!(shape(&list), ["+a"]);
        assert!(list.select_task("b"));
        assert_eq!(shape(&list), ["-a", "  +b"]);
        assert_eq!(list.selected(), 1);
        assert!(!list.select_task("missing"));
    }

    #[test]
    fn enter_shows_the_tasks_agent_and_arrows_fold() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetTasks(vec![task("a", None, R, 1)]));
        app.process_command(UiCommand::UpsertTask(task("b", Some("a"), R, 0)));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Tasks);
        assert_eq!(shape(app.task_list()), ["-a", "  a 0", "  -b"]);

        app.handle_key(KeyEvent::from(KeyCode::End));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(switched(&mut rx).as_deref(), Some("agent-b"));

        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(shape(app.task_list()), ["+a"]);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert_eq!(shape(app.task_list()).len(), 3, "Space opens it again");
    }

    #[test]
    fn p_opens_the_plan_at_the_tasks_todo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(
            dir.path().join(".aura/plans/a.plan.md"),
            "---\nname: a\ntodos:\n- id: one\n  content: first\n- id: two\n  content: second\n---\n",
        )
        .unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        let mut part = task("a", None, R, 0);
        part.plan = Some(".aura/plans/a.plan.md".to_string());
        part.plan_todo = Some("two".to_string());
        app.process_command(UiCommand::SetTasks(vec![part]));
        app.focus = super::super::PanelFocus::Sandbox;
        app.show_side_tab(super::super::SidePanelTab::Tasks);

        app.handle_key(KeyEvent::from(KeyCode::Char('p')));
        let Some(super::super::DetailView::File(view)) = app.detail_view() else {
            panic!("the plan opens as a file");
        };
        assert_eq!(view.vim.line(), 5, "the cursor lands on the todo");
    }

    #[test]
    fn a_click_shows_the_agent_and_the_fold_glyph_folds() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetTasks(vec![task("a", None, R, 1)]));
        app.show_side_tab(super::super::SidePanelTab::Tasks);
        app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);

        app.click_task_row(11, 5);
        assert_eq!(shape(app.task_list()), ["+a"]);
        app.click_task_row(20, 5);
        assert_eq!(switched(&mut rx).as_deref(), Some("agent-a"));
    }
}
