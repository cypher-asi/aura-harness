//! The TASKS side tab: every piece of work agents did or are doing in the
//! active agent's workspace, grouped under the plan it works on. Each plan
//! is a foldable header with its todos, then the tasks working on it; the
//! plan being worked on comes first, then the other plans newest first.
//! Tasks that work on no plan, or on one that is not listed, go last
//! under a `No Plan` header, shown only when there are such tasks.
//!
//! A user's request is a task, and each piece of it an agent handed to
//! another with `task` is a task under it, in the same group; each shows
//! what its agent has done and has left. Plans being worked on, with
//! todos left or with a task still open, start open, as do tasks still
//! being worked on or left with work to do; the rest start folded. Folds
//! the user makes stick across updates.
//!
//! Only the tasks of the agents the side panel's scope lets through are
//! listed, with every task under them, and only the plans those tasks
//! work on or whose todos they do. The plan being worked on is always
//! listed.

use std::collections::{HashMap, HashSet};

use super::processes::SIDE_PAGE;
use super::side_scope::AgentFilter;
use super::{contains, follow_selection, App, NotificationType, SideScope};
use crate::events::{PlanView, TaskView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// One row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRow {
    /// A group's header: a plan, or `None` for the No Plan group
    Plan { plan: Option<usize>, open: bool },
    /// A todo of an open plan
    Todo { plan: usize, todo: usize },
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
    /// Index of the task the row belongs to; `None` on plan headers and
    /// todos.
    #[must_use]
    pub const fn task(self) -> Option<usize> {
        match self {
            Self::Task { task, .. } | Self::Item { task, .. } => Some(task),
            Self::Plan { .. } | Self::Todo { .. } => None,
        }
    }

    /// Index of the plan of a plan header or todo; `None` on the No Plan
    /// header and on task rows, even when the task works on a plan.
    #[must_use]
    pub const fn plan(self) -> Option<usize> {
        match self {
            Self::Plan { plan, .. } => plan,
            Self::Todo { plan, .. } => Some(plan),
            Self::Task { .. } | Self::Item { .. } => None,
        }
    }

    /// Whether the row's plan or task is open. Todos and items only show
    /// under an open one.
    const fn is_open(self) -> bool {
        match self {
            Self::Plan { open, .. } | Self::Task { open, .. } => open,
            Self::Todo { .. } | Self::Item { .. } => true,
        }
    }
}

/// What a row shows, by path and id rather than index, so the highlight
/// can find it again after the lists change.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Plan(String),
    NoPlan,
    Todo(String, usize),
    Task(String),
    Item(String, usize),
}

/// The plans and tasks, and how the tab shows them.
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
    /// Newest first
    plans: Vec<PlanView>,
    /// Path of the plan being worked on
    current_plan: Option<String>,
    /// Plans the user folded (`false`) or opened (`true`), by path
    plan_folds: HashMap<String, bool>,
    /// Whether the user folded (`false`) or opened the No Plan group
    no_plan_open: Option<bool>,
    /// Which of `plans` are listed for the scoped tasks
    plans_shown: Vec<bool>,
    rows: Vec<TaskRow>,
    selected: usize,
}

impl TaskList {
    /// The tasks, newest first, listed or not.
    #[must_use]
    pub fn tasks(&self) -> &[TaskView] {
        &self.tasks
    }

    /// The plans, newest first, listed or not.
    #[must_use]
    pub fn plans(&self) -> &[PlanView] {
        &self.plans
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

    /// Plans left out because no listed task works on them.
    #[must_use]
    pub fn hidden_plans(&self) -> usize {
        self.plans_shown.iter().filter(|&&shown| !shown).count()
    }

    /// List only the tasks of the agents `filter` lets through, the tasks
    /// under those, and the plans they work on.
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

    /// Whether `plan` is the one being worked on.
    #[must_use]
    pub fn is_current(&self, plan: &PlanView) -> bool {
        self.current_plan.as_deref() == Some(plan.path.as_str())
    }

    /// Replace the tasks, keeping the highlight on the same row when it
    /// is still listed.
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

    /// Replace the plans, keeping the highlight on the same row when it
    /// is still listed.
    pub fn set_plans(&mut self, plans: Vec<PlanView>) {
        let keep = self.selected_key();
        self.plans = plans;
        self.rebuild(keep);
    }

    /// Mark `plan` as the one being worked on, replacing its listed copy,
    /// or adding it on top when it isn't listed. `None` leaves the plans
    /// as they are with none marked.
    pub fn set_current_plan(&mut self, plan: Option<&PlanView>) {
        let keep = self.selected_key();
        self.current_plan = plan.map(|plan| plan.path.clone());
        if let Some(plan) = plan {
            match self.plans.iter_mut().find(|p| p.path == plan.path) {
                Some(listed) => *listed = plan.clone(),
                None => self.plans.insert(0, plan.clone()),
            }
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

    /// Highlight task `id`, opening its group and the tasks it is part
    /// of. Returns whether it is listed.
    pub fn select_task(&mut self, id: &str) -> bool {
        let mut parent = self.find(id).and_then(|task| task.parent.clone());
        let mut seen = HashSet::new();
        while let Some(id) = parent.filter(|id| seen.insert(id.clone())) {
            self.folds.insert(id.clone(), true);
            parent = self.find(&id).and_then(|task| task.parent.clone());
        }
        if let Some(root) = self.listed_root(id) {
            match self.group_of(root) {
                Some(plan) => {
                    let path = self.plans[plan].path.clone();
                    self.plan_folds.insert(path, true);
                }
                None => self.no_plan_open = Some(true),
            }
        }
        self.rebuild(Some(RowKey::Task(id.to_string())));
        self.selected_task().is_some_and(|task| task.id == id)
    }

    /// The task of the highlighted row.
    #[must_use]
    pub fn selected_task(&self) -> Option<&TaskView> {
        let row = self.rows.get(self.selected)?;
        self.tasks.get(row.task()?)
    }

    /// The plan of the highlighted plan header or todo.
    #[must_use]
    pub fn selected_plan(&self) -> Option<&PlanView> {
        let row = self.rows.get(self.selected)?;
        self.plans.get(row.plan()?)
    }

    /// Fold or open the group or task of the highlighted row. Folding
    /// from a todo or an item moves the highlight to its header.
    pub fn toggle_selected(&mut self) {
        let Some(&row) = self.rows.get(self.selected) else {
            return;
        };
        let open = !row.is_open();
        let keep = match row {
            TaskRow::Plan { plan: None, .. } => {
                self.no_plan_open = Some(open);
                RowKey::NoPlan
            }
            TaskRow::Plan {
                plan: Some(plan), ..
            }
            | TaskRow::Todo { plan, .. } => {
                let Some(path) = self.plans.get(plan).map(|plan| plan.path.clone()) else {
                    return;
                };
                self.plan_folds.insert(path.clone(), open);
                RowKey::Plan(path)
            }
            TaskRow::Task { task, .. } | TaskRow::Item { task, .. } => {
                let Some(id) = self.tasks.get(task).map(|task| task.id.clone()) else {
                    return;
                };
                self.folds.insert(id.clone(), open);
                RowKey::Task(id)
            }
        };
        self.rebuild(Some(keep));
    }

    /// Open (`true`) or fold the group or task of the highlighted row,
    /// when it is not that way already.
    pub fn set_selected_open(&mut self, open: bool) {
        if self
            .rows
            .get(self.selected)
            .is_some_and(|row| row.is_open() != open)
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

    /// Whether plan `plan` is open: as the user left it, else when it is
    /// being worked on, has todos left or `has_open_task` in its group.
    fn is_plan_open(&self, plan: &PlanView, has_open_task: bool) -> bool {
        self.plan_folds
            .get(&plan.path)
            .copied()
            .unwrap_or_else(|| self.is_current(plan) || plan.has_open_todos() || has_open_task)
    }

    /// The topmost listed task `id` is part of: the root of the tree it
    /// is drawn in. `None` when `id` is not listed.
    fn listed_root(&self, id: &str) -> Option<usize> {
        let is_listed = |index: usize| self.scoped.get(index).copied().unwrap_or(false);
        let mut root = self
            .tasks
            .iter()
            .position(|task| task.id == id)
            .filter(|&index| is_listed(index))?;
        let mut seen = HashSet::from([root]);
        while let Some(parent) = self.tasks[root]
            .parent
            .as_deref()
            .and_then(|parent| self.tasks.iter().position(|task| task.id == parent))
            .filter(|&parent| is_listed(parent) && seen.insert(parent))
        {
            root = parent;
        }
        Some(root)
    }

    /// The listed plan whose group the tree rooted at task `root` goes
    /// in; `None` for the No Plan group.
    fn group_of(&self, root: usize) -> Option<usize> {
        let path = self.tasks.get(root)?.plan.as_deref()?;
        self.plans
            .iter()
            .zip(&self.plans_shown)
            .position(|(plan, &shown)| shown && plan.path == path)
    }

    /// What the highlighted row shows.
    fn selected_key(&self) -> Option<RowKey> {
        let row = *self.rows.get(self.selected)?;
        let plan_path = |plan: usize| self.plans.get(plan).map(|plan| plan.path.clone());
        let task_id = |task: usize| self.tasks.get(task).map(|task| task.id.clone());
        match row {
            TaskRow::Plan { plan: None, .. } => Some(RowKey::NoPlan),
            TaskRow::Plan {
                plan: Some(plan), ..
            } => plan_path(plan).map(RowKey::Plan),
            TaskRow::Todo { plan, todo } => plan_path(plan).map(|path| RowKey::Todo(path, todo)),
            TaskRow::Task { task, .. } => task_id(task).map(RowKey::Task),
            TaskRow::Item { task, item, .. } => task_id(task).map(|id| RowKey::Item(id, item)),
        }
    }

    /// Whether `row` shows what `key` names.
    fn row_is(&self, row: TaskRow, key: &RowKey) -> bool {
        let plan_is =
            |plan: usize, path: &str| self.plans.get(plan).is_some_and(|p| p.path == path);
        let task_is = |task: usize, id: &str| self.tasks.get(task).is_some_and(|t| t.id == id);
        match (row, key) {
            (TaskRow::Plan { plan: None, .. }, RowKey::NoPlan) => true,
            (
                TaskRow::Plan {
                    plan: Some(plan), ..
                },
                RowKey::Plan(path),
            ) => plan_is(plan, path.as_str()),
            (TaskRow::Todo { plan, todo }, RowKey::Todo(path, wanted)) => {
                todo == *wanted && plan_is(plan, path.as_str())
            }
            (TaskRow::Task { task, .. }, RowKey::Task(id)) => task_is(task, id.as_str()),
            (TaskRow::Item { task, item, .. }, RowKey::Item(id, wanted)) => {
                item == *wanted && task_is(task, id.as_str())
            }
            _ => false,
        }
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

    /// Which plans are listed: every plan when every task is, else the
    /// current plan and the plans the listed tasks work on or whose todos
    /// they do.
    fn shown_plans(&self) -> Vec<bool> {
        if self.filter.is_none() {
            return vec![true; self.plans.len()];
        }
        let ids: HashSet<&str> = self.scoped_tasks().map(|task| task.id.as_str()).collect();
        let paths: HashSet<&str> = self
            .scoped_tasks()
            .filter_map(|task| task.plan.as_deref())
            .collect();
        self.plans
            .iter()
            .map(|plan| {
                self.is_current(plan)
                    || paths.contains(plan.path.as_str())
                    || plan
                        .todos
                        .iter()
                        .any(|todo| todo.task.as_deref().is_some_and(|id| ids.contains(id)))
            })
            .collect()
    }

    /// Every task of the trees rooted at `tops`, in drawing order, with
    /// its depth, whether folded or not. `seen` guards against parent
    /// loops.
    fn walk(
        &self,
        tops: &[usize],
        children: &HashMap<&str, Vec<usize>>,
        seen: &mut HashSet<usize>,
    ) -> Vec<(usize, usize)> {
        let mut tree = Vec::new();
        let mut stack: Vec<(usize, usize)> = tops.iter().rev().map(|&top| (top, 0)).collect();
        while let Some((task, depth)) = stack.pop() {
            if !seen.insert(task) {
                continue;
            }
            tree.push((task, depth));
            if let Some(parts) = children.get(self.tasks[task].id.as_str()) {
                stack.extend(parts.iter().rev().map(|&part| (part, depth + 1)));
            }
        }
        tree
    }

    /// Rows of the tasks in `tree`, as [`Self::walk`] lists them, leaving
    /// out what is under a folded task.
    fn task_rows(&self, tree: &[(usize, usize)], rows: &mut Vec<TaskRow>) {
        let mut folded_at = None;
        for &(task, depth) in tree {
            if folded_at.is_some_and(|folded| depth > folded) {
                continue;
            }
            folded_at = None;
            let view = &self.tasks[task];
            let open = self.is_open(view);
            rows.push(TaskRow::Task { task, depth, open });
            if open {
                rows.extend((0..view.items.len()).map(|item| TaskRow::Item {
                    task,
                    item,
                    depth: depth + 1,
                }));
            } else {
                folded_at = Some(depth);
            }
        }
    }

    fn rebuild(&mut self, keep: Option<RowKey>) {
        self.scoped = self.scope();
        self.plans_shown = self.shown_plans();
        let rows = self.build_rows();
        self.rows = rows;
        let kept = keep.and_then(|key| self.rows.iter().position(|&row| self.row_is(row, &key)));
        self.select(kept.unwrap_or(self.selected));
    }

    fn build_rows(&self) -> Vec<TaskRow> {
        let listed: HashSet<&str> = self.scoped_tasks().map(|task| task.id.as_str()).collect();
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); self.plans.len()];
        let mut no_plan = Vec::new();
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
                None => match self.group_of(index) {
                    Some(plan) => groups[plan].push(index),
                    None => no_plan.push(index),
                },
            }
        }
        // Newest first at the top; a task's parts in the order given.
        for parts in children.values_mut() {
            parts.reverse();
        }

        let mut order: Vec<usize> = (0..self.plans.len())
            .filter(|&plan| self.plans_shown[plan])
            .collect();
        order.sort_by_key(|&plan| !self.is_current(&self.plans[plan]));

        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for plan in order {
            let view = &self.plans[plan];
            let tree = self.walk(&groups[plan], &children, &mut seen);
            let has_open_task = tree
                .iter()
                .any(|&(task, _)| !self.tasks[task].state.is_closed());
            let open = self.is_plan_open(view, has_open_task);
            rows.push(TaskRow::Plan {
                plan: Some(plan),
                open,
            });
            if open {
                rows.extend((0..view.todos.len()).map(|todo| TaskRow::Todo { plan, todo }));
                self.task_rows(&tree, &mut rows);
            }
        }
        if !no_plan.is_empty() {
            let open = self.no_plan_open.unwrap_or(true);
            rows.push(TaskRow::Plan { plan: None, open });
            if open {
                let tree = self.walk(&no_plan, &children, &mut seen);
                self.task_rows(&tree, &mut rows);
            }
        }
        rows
    }
}

impl App {
    /// The TASKS tab's plans and tasks.
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

    /// Keys of the TASKS tab: Enter opens the row, Right unfolds its plan
    /// or task and Left folds it. `p`, `t` and Space are side-list
    /// shortcuts.
    pub(super) fn handle_tasks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.task_list.move_by(-1),
            KeyCode::Down => self.task_list.move_by(1),
            KeyCode::PageUp => self.task_list.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.task_list.move_by(SIDE_PAGE),
            KeyCode::Home => self.task_list.select(0),
            KeyCode::End => self.task_list.select(usize::MAX),
            KeyCode::Enter => self.open_selected_task_row(),
            KeyCode::Right => self.task_list.set_selected_open(true),
            KeyCode::Left => self.task_list.set_selected_open(false),
            _ => {}
        }
    }

    /// Open the highlighted row: a plan's file, at the todo for a todo;
    /// a task's agent. The No Plan header has nothing to open, so it
    /// folds.
    fn open_selected_task_row(&mut self) {
        match self.task_list.rows.get(self.task_list.selected) {
            Some(TaskRow::Plan { plan: None, .. }) => self.task_list.toggle_selected(),
            Some(TaskRow::Plan { .. } | TaskRow::Todo { .. }) => self.open_selected_plan(),
            Some(TaskRow::Task { .. } | TaskRow::Item { .. }) => self.open_selected_task_agent(),
            None => {}
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

    /// Open the plan of the highlighted row: the plan itself on a header
    /// or todo, the plan a task works on, at its todo, on a task row.
    pub(super) fn open_selected_row_plan(&mut self) {
        if self.task_list.selected_plan().is_some() {
            self.open_selected_plan();
        } else {
            self.open_selected_task_plan();
        }
    }

    /// Open the plan the highlighted task works on, at its todo.
    fn open_selected_task_plan(&mut self) {
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

    /// Highlight the task doing the highlighted todo.
    pub(super) fn jump_to_todo_task(&mut self) {
        let task = match (
            self.task_list.selected_plan(),
            self.task_list.rows.get(self.task_list.selected),
        ) {
            (Some(plan), Some(TaskRow::Todo { todo, .. })) => {
                plan.todos.get(*todo).and_then(|todo| todo.task.clone())
            }
            _ => None,
        };
        let Some(task) = task else {
            self.notification = Some((
                "No task is doing this todo".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        let hidden = !self.task_list.select_task(&task)
            && self
                .task_list
                .tasks()
                .iter()
                .any(|listed| listed.id == task);
        if hidden {
            self.notification = Some((
                format!(
                    "Another agent is doing this todo; the {} button lists it",
                    SideScope::BUTTON
                ),
                NotificationType::Warning,
            ));
        }
    }

    /// Clicking a plan's or task's fold glyph folds or opens it; clicking
    /// anywhere else on a row opens it as Enter does.
    pub(super) fn click_task_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.task_row_at(column, row) else {
            return;
        };
        self.task_list.select(index);
        let fold_depth = match self.task_list.rows.get(index) {
            Some(TaskRow::Plan { .. }) => Some(0),
            Some(&TaskRow::Task { depth, .. }) => Some(depth),
            _ => None,
        };
        let on_fold = self.areas.task_rows.is_some_and(|rows| {
            fold_depth.is_some_and(|depth| super::processes::on_row_icon(column, rows.x, depth))
        });
        if on_fold {
            self.task_list.toggle_selected();
        } else {
            self.open_selected_task_row();
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
    use crate::app::{DetailView, PanelFocus, SidePanelTab};
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

    fn planned(id: &str, plan: &str, state: TaskState) -> TaskView {
        TaskView {
            plan: Some(path(plan)),
            ..task(id, None, state, 0)
        }
    }

    fn path(name: &str) -> String {
        format!(".aura/plans/{name}.plan.md")
    }

    fn plan(name: &str, states: &[TodoState]) -> PlanView {
        PlanView {
            path: path(name),
            name: name.to_string(),
            todos: states
                .iter()
                .enumerate()
                .map(|(i, &state)| TodoView {
                    content: format!("{name} {i}"),
                    state,
                    assignee: None,
                    task: None,
                })
                .collect(),
        }
    }

    /// The rows as text: `-`/`+` before an open/folded plan or task, `#`
    /// before a todo, items indented under their task.
    fn shape(list: &TaskList) -> Vec<String> {
        let fold = |open| if open { "-" } else { "+" };
        list.rows()
            .iter()
            .map(|row| match *row {
                TaskRow::Plan { plan, open } => format!(
                    "{}{}",
                    fold(open),
                    plan.map_or("No Plan", |plan| list.plans()[plan].name.as_str())
                ),
                TaskRow::Todo { plan, todo } => {
                    format!("#{}", list.plans()[plan].todos[todo].content)
                }
                TaskRow::Task { task, depth, open } => format!(
                    "{}{}{}",
                    "  ".repeat(depth),
                    fold(open),
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
    use TodoState::{Completed as Done, InProgress as Doing, Pending as Todo};

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
            ["-No Plan", "-a", "  a 0", "  -c", "    c 0", "  +b", "+old"],
            "parts newest first, like the requests"
        );
    }

    #[test]
    fn an_update_keeps_the_highlight_and_a_new_task_goes_on_top() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("a", None, R, 2)]);
        list.select(3);
        list.upsert(task("b", None, R, 0));
        assert_eq!(shape(&list), ["-No Plan", "-b", "-a", "  a 0", "  a 1"]);
        assert_eq!(list.selected(), 4, "still on a's second item");

        list.upsert(task("a", None, C, 2));
        assert_eq!(shape(&list), ["-No Plan", "-b", "+a"], "a closed and folds");
        list.select(2);
        list.toggle_selected();
        assert_eq!(shape(&list).len(), 5, "the user opens it again");
    }

    #[test]
    fn selecting_a_folded_part_opens_its_request_and_group() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("b", Some("a"), C, 0), task("a", None, C, 0)]);
        assert_eq!(shape(&list), ["-No Plan", "+a"]);
        list.select(0);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+No Plan"]);
        assert!(list.select_task("b"));
        assert_eq!(shape(&list), ["-No Plan", "-a", "  +b"]);
        assert_eq!(list.selected(), 2);
        assert!(!list.select_task("missing"));

        list.set_plans(vec![plan("p", &[Done])]);
        list.set_tasks(vec![planned("t", "p", C)]);
        assert_eq!(shape(&list), ["+p"], "a done plan with done tasks folds");
        assert!(list.select_task("t"));
        assert_eq!(shape(&list), ["-p", "#p 0", "+t"]);
        assert_eq!(list.selected(), 2);
    }

    #[test]
    fn tasks_group_under_their_plan_with_its_todos_above_them() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("auth", &[Done, Todo]), plan("parser", &[Done])]);
        list.set_tasks(vec![
            planned("fix", "parser", C),
            task("part", Some("build"), R, 0),
            planned("build", "auth", R),
        ]);
        assert_eq!(
            shape(&list),
            ["-auth", "#auth 0", "#auth 1", "-build", "  -part", "+parser"],
            "plans newest first, the part with its request"
        );
    }

    #[test]
    fn a_part_stays_with_its_root_whatever_plan_it_names() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo]), plan("b", &[Todo])]);
        let mut part = task("part", Some("root"), R, 0);
        part.plan = Some(path("b"));
        list.set_tasks(vec![part, planned("root", "a", R)]);
        assert_eq!(
            shape(&list),
            ["-a", "#a 0", "-root", "  -part", "-b", "#b 0"]
        );
    }

    #[test]
    fn no_plan_lists_tasks_of_no_listed_plan_and_only_when_there_are_some() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo])]);
        assert_eq!(shape(&list), ["-a", "#a 0"], "no No Plan header");

        list.set_tasks(vec![
            planned("gone", "missing", R),
            task("loose", None, R, 0),
            planned("t", "a", R),
        ]);
        assert_eq!(
            shape(&list),
            ["-a", "#a 0", "-t", "-No Plan", "-gone", "-loose"]
        );
    }

    #[test]
    fn a_plan_with_no_tasks_still_lists_its_todos() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("fresh", &[Todo, Todo])]);
        assert_eq!(shape(&list), ["-fresh", "#fresh 0", "#fresh 1"]);
        assert!(list.selected_task().is_none());
        assert_eq!(list.selected_plan().map(|p| p.name.as_str()), Some("fresh"));
    }

    #[test]
    fn plans_with_open_work_start_open_and_done_ones_folded() {
        let mut list = TaskList::default();
        list.set_plans(vec![
            plan("todos", &[Done, Doing]),
            plan("task", &[Done]),
            plan("done", &[Done, Done]),
        ]);
        list.set_tasks(vec![planned("t", "task", R), planned("d", "done", C)]);
        assert_eq!(
            shape(&list),
            ["-todos", "#todos 0", "#todos 1", "-task", "#task 0", "-t", "+done"]
        );
    }

    #[test]
    fn the_current_plan_comes_first_and_starts_open_even_when_done() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo]), plan("b", &[Done])]);
        list.set_current_plan(Some(&plan("b", &[Done])));
        assert_eq!(shape(&list), ["-b", "#b 0", "-a", "#a 0"]);
        assert!(list.is_current(&list.plans()[1]));
    }

    #[test]
    fn current_plan_updates_its_listed_copy_or_goes_on_top() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo]), plan("b", &[Todo])]);
        list.select(2);
        list.set_current_plan(Some(&plan("b", &[Done, Todo])));
        assert_eq!(list.plans()[1].todos.len(), 2);
        assert_eq!(
            shape(&list)[list.selected()],
            "-b",
            "the highlight stays on b"
        );

        list.set_current_plan(Some(&plan("new", &[Todo])));
        assert_eq!(list.plans()[0].name, "new");
        list.set_current_plan(None);
        assert_eq!(list.plans().len(), 3);
        assert!(list.plans().iter().all(|p| !list.is_current(p)));
    }

    #[test]
    fn folds_stick_across_updates_and_folding_moves_to_the_header() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo, Todo]), plan("b", &[Todo])]);
        list.set_tasks(vec![task("loose", None, R, 1)]);
        list.select(2);
        list.toggle_selected();
        assert_eq!(
            shape(&list),
            ["+a", "-b", "#b 0", "-No Plan", "-loose", "  loose 0"]
        );
        assert_eq!(list.selected(), 0);
        list.select(3);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+a", "-b", "#b 0", "+No Plan"]);

        list.set_plans(vec![plan("a", &[Todo, Todo, Todo]), plan("b", &[Todo])]);
        list.set_tasks(vec![task("new", None, R, 0), task("loose", None, R, 1)]);
        assert_eq!(
            shape(&list),
            ["+a", "-b", "#b 0", "+No Plan"],
            "the folds survive updates"
        );
        assert_eq!(list.selected(), 3, "the highlight stays on No Plan");
    }

    #[test]
    fn enter_shows_the_tasks_agent_and_arrows_fold() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetTasks(vec![task("a", None, R, 1)]));
        app.process_command(UiCommand::UpsertTask(task("b", Some("a"), R, 0)));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Tasks);
        assert_eq!(shape(app.task_list()), ["-No Plan", "-a", "  a 0", "  -b"]);

        app.handle_key(KeyEvent::from(KeyCode::End));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(switched(&mut rx).as_deref(), Some("agent-b"));

        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(shape(app.task_list()), ["-No Plan", "+a"]);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert_eq!(shape(app.task_list()).len(), 4, "Space opens it again");

        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(shape(app.task_list()), ["+No Plan"], "Enter folds No Plan");
        assert!(switched(&mut rx).is_none());
    }

    /// An app on the TASKS tab of a temp sandbox holding `.aura/plans/a.plan.md`.
    fn app_with_plan_file(text: &str) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(dir.path().join(".aura/plans/a.plan.md"), text).unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Tasks);
        (app, dir)
    }

    fn open_file_line(app: &App) -> usize {
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("the plan opens as a file");
        };
        view.vim.line()
    }

    #[test]
    fn p_opens_the_plan_at_the_tasks_todo() {
        let (mut app, _dir) = app_with_plan_file(
            "---\nname: a\ntodos:\n- id: one\n  content: first\n- id: two\n  content: second\n---\n",
        );
        let mut part = task("a", None, R, 0);
        part.plan = Some(path("a"));
        part.plan_todo = Some("two".to_string());
        app.process_command(UiCommand::SetTasks(vec![part]));
        app.handle_key(KeyEvent::from(KeyCode::Down));

        app.handle_key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(open_file_line(&app), 5, "the cursor lands on the todo");
    }

    #[test]
    fn enter_on_a_plan_header_opens_its_file_and_on_a_todo_at_the_todo() {
        let (mut app, _dir) =
            app_with_plan_file("---\nname: a\ntodos:\n- content: a 0\n- content: a 1\n---\n");
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[Todo, Todo])]));
        app.process_command(UiCommand::SetTasks(vec![planned("t", "a", R)]));
        assert_eq!(shape(app.task_list()), ["-a", "#a 0", "#a 1", "-t"]);

        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            open_file_line(&app),
            0,
            "the header opens the file at the top"
        );
        app.close_detail();
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(open_file_line(&app), 4, "a todo opens at its line");
        app.close_detail();
        app.handle_key(KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(open_file_line(&app), 4, "p on a todo opens the plan too");
    }

    #[test]
    fn t_on_a_todo_selects_the_task_doing_it() {
        let mut app = App::new();
        let mut doing = plan("a", &[Doing]);
        doing.todos[0].task = Some("t1".to_string());
        app.process_command(UiCommand::SetPlans(vec![doing]));
        app.process_command(UiCommand::SetTasks(vec![
            task("t2", None, R, 0),
            task("t1", None, R, 0),
        ]));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Tasks);
        assert_eq!(
            shape(app.task_list()),
            ["-a", "#a 0", "-No Plan", "-t2", "-t1"]
        );

        app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert!(app.notification().is_some(), "a plan header has no task");
        assert_eq!(app.task_list().selected(), 0);
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert_eq!(app.side_tab(), SidePanelTab::Tasks);
        assert_eq!(
            app.task_list().selected_task().map(|t| t.id.as_str()),
            Some("t1")
        );
    }

    #[test]
    fn a_click_on_a_task_shows_the_agent_and_the_fold_glyph_folds() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetTasks(vec![task("a", None, R, 1)]));
        app.show_side_tab(SidePanelTab::Tasks);
        app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);

        app.click_task_row(11, 6);
        assert_eq!(shape(app.task_list()), ["-No Plan", "+a"]);
        app.click_task_row(20, 6);
        assert_eq!(switched(&mut rx).as_deref(), Some("agent-a"));
        app.click_task_row(20, 5);
        assert_eq!(
            shape(app.task_list()),
            ["+No Plan"],
            "No Plan folds anywhere"
        );
    }

    #[test]
    fn a_click_opens_a_plan_and_its_fold_glyph_folds_it() {
        let (mut app, _dir) = app_with_plan_file("a\n");
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[Todo])]));
        app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);

        app.click_task_row(11, 5);
        assert!(app.detail_view().is_none());
        assert_eq!(shape(app.task_list()), ["+a"]);
        app.click_task_row(20, 5);
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    }

    #[test]
    fn a_new_plan_opens_the_tasks_tab_but_its_updates_do_not() {
        let mut app = App::new();
        app.process_command(UiCommand::SetPlan(Some(plan("a", &[Todo, Todo]))));
        assert_eq!(app.side_tab(), SidePanelTab::Tasks);

        app.show_side_tab(SidePanelTab::Sandbox);
        app.process_command(UiCommand::SetPlan(Some(plan("a", &[Done, Todo]))));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);

        app.process_command(UiCommand::SetPlan(Some(plan("b", &[Todo]))));
        assert_eq!(app.side_tab(), SidePanelTab::Tasks);
    }

    #[test]
    fn set_plan_marks_the_listed_plan() {
        let mut app = App::new();
        app.process_command(UiCommand::SetPlans(vec![
            plan("a", &[Todo]),
            plan("b", &[Done]),
        ]));
        app.process_command(UiCommand::SetPlan(Some(plan("b", &[Done, Todo]))));
        let list = app.task_list();
        assert!(list.is_current(&list.plans()[1]));
        assert_eq!(list.plans()[1].todos.len(), 2);
    }
}
