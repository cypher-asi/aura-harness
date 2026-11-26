//! Moving the highlight, folding and opening rows, and finding a row
//! again by what it shows.

use super::{RowKey, TaskList, TaskRow};
use crate::events::{PlanView, TaskView};

/// Indexes of the rows `rows[at]` is under, nearest first: the todo,
/// item or task it is nested in, its round, and its group's header.
fn containers(rows: &[TaskRow], at: usize) -> Vec<usize> {
    let Some(&row) = rows.get(at) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = row.depth();
    let mut in_round = matches!(row, TaskRow::Item { in_round: true, .. });
    for (i, &above) in rows[..at].iter().enumerate().rev() {
        if depth > 0 {
            if above.depth() < depth {
                out.push(i);
                depth = above.depth();
                in_round = matches!(above, TaskRow::Item { in_round: true, .. });
            }
            continue;
        }
        match above {
            TaskRow::Round { .. } if in_round => {
                out.push(i);
                in_round = false;
            }
            TaskRow::Plan { .. } => {
                out.push(i);
                break;
            }
            _ => {}
        }
    }
    out
}

impl TaskList {
    /// Move the highlight by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    /// Highlight row `index` (clamped).
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// Highlight task `id`, opening its group, its round, the todo or
    /// item it is nested under and the tasks it is part of. Returns
    /// whether it has a row; a request has none, so its group's header is
    /// highlighted.
    pub fn select_task(&mut self, id: &str) -> bool {
        let key = RowKey::Task(id.to_string());
        self.reveal = true;
        let all = self.build_rows();
        self.reveal = false;
        if let Some(at) = all.iter().position(|&row| self.row_is(row, &key)) {
            for above in containers(&all, at) {
                self.set_open(all[above], true);
            }
            self.rebuild(Some(key.clone()));
            return self
                .rows
                .get(self.selected)
                .is_some_and(|&row| self.row_is(row, &key));
        }
        let Some(root) = self.listed_root(id) else {
            self.rebuild(None);
            return false;
        };
        let group = self
            .tasks
            .get(root)
            .and_then(|root| self.plan_index().group_of(root));
        let group_key = match group.and_then(|plan| self.plans.get(plan)) {
            Some(plan) => {
                let path = plan.path.clone();
                self.plan_folds.insert(path.clone(), true);
                RowKey::Plan(path)
            }
            None => {
                self.no_plan_open = Some(true);
                RowKey::NoPlan
            }
        };
        self.rebuild(Some(group_key.clone()));
        if let Some(index) = self
            .rows
            .iter()
            .position(|&row| self.row_is(row, &group_key))
        {
            self.select(index);
        }
        false
    }

    /// Whether task `id` is the request building the plan whose group
    /// it is in, whose list the plan's todos are.
    #[must_use]
    pub fn is_hidden_build(&self, id: &str) -> bool {
        let Some(root) = self.listed_root(id).and_then(|root| self.tasks.get(root)) else {
            return false;
        };
        root.id == id && self.hides(root, self.plan_index().group_of(root))
    }

    /// The task of the highlighted row: the task itself, or the request
    /// an item was last listed by.
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

    /// The plan of the group the highlighted row is in; `None` under No
    /// Plan.
    #[must_use]
    pub fn selected_group_plan(&self) -> Option<&PlanView> {
        let header =
            self.rows
                .get(..=self.selected)?
                .iter()
                .rev()
                .find_map(|row| match row {
                    TaskRow::Plan { plan, .. } => Some(*plan),
                    _ => None,
                })??;
        self.plans.get(header)
    }

    /// Fold or open what is under the highlighted row. A row with nothing
    /// under it folds the row it is under, moving the highlight there.
    pub fn toggle_selected(&mut self) {
        let Some(&row) = self.rows.get(self.selected) else {
            return;
        };
        let Some(open) = row.fold().map(|open| !open) else {
            if let Some(&above) = containers(&self.rows, self.selected).first() {
                self.selected = above;
                self.toggle_selected();
            }
            return;
        };
        if let Some(keep) = self.set_open(row, open) {
            self.rebuild(Some(keep));
        }
    }

    /// Keep `row` open (`true`) or folded, returning what it shows.
    fn set_open(&mut self, row: TaskRow, open: bool) -> Option<RowKey> {
        let plan_path = |plan: usize| self.plans.get(plan).map(|plan| plan.path.clone());
        let task_id = |task: usize| self.tasks.get(task).map(|task| task.id.clone());
        match row {
            TaskRow::Plan { plan: None, .. } => {
                self.no_plan_open = Some(open);
                Some(RowKey::NoPlan)
            }
            TaskRow::Plan {
                plan: Some(plan), ..
            } => {
                let path = plan_path(plan)?;
                self.plan_folds.insert(path.clone(), open);
                Some(RowKey::Plan(path))
            }
            TaskRow::Round { first, .. } => {
                let id = task_id(first)?;
                self.round_folds.insert(id.clone(), open);
                Some(RowKey::Round(id))
            }
            TaskRow::Todo { plan, todo, .. } => {
                let path = plan_path(plan)?;
                self.todo_folds.insert((path.clone(), todo), open);
                Some(RowKey::Todo(path, todo))
            }
            TaskRow::Item { task, item, .. } => {
                let key = self.item_key(task, item)?;
                self.item_folds.insert(key.clone(), open);
                Some(RowKey::Item(key.0, key.1))
            }
            TaskRow::Task { task, .. } => {
                let id = task_id(task)?;
                self.folds.insert(id.clone(), open);
                Some(RowKey::Task(id))
            }
        }
    }

    /// Open (`true`) or fold what is under the highlighted row, when it
    /// is not that way already.
    pub fn set_selected_open(&mut self, open: bool) {
        if self
            .rows
            .get(self.selected)
            .is_some_and(|row| row.fold().unwrap_or(true) != open)
        {
            self.toggle_selected();
        }
    }

    /// Agent id and content of `item` of `task`, which key its fold.
    fn item_key(&self, task: usize, item: usize) -> Option<(String, String)> {
        let view = self.tasks.get(task)?;
        let item = view.items.get(item)?;
        Some((view.agent_id.clone(), item.content.clone()))
    }

    pub(super) fn is_open(&self, task: &TaskView) -> bool {
        self.reveal
            || self
                .folds
                .get(&task.id)
                .copied()
                .unwrap_or(!task.state.is_closed())
    }

    /// Whether plan `plan` is open: as the user left it, else when it is
    /// being worked on, has todos left or `has_open_task` in its group.
    pub(super) fn is_plan_open(&self, plan: &PlanView, has_open_task: bool) -> bool {
        self.reveal
            || self
                .plan_folds
                .get(&plan.path)
                .copied()
                .unwrap_or_else(|| self.is_current(plan) || plan.has_open_todos() || has_open_task)
    }

    /// Whether todo `todo` of `plan` shows the tasks nested under it: as
    /// the user left it, else while the todo is not closed.
    pub(super) fn is_todo_open(&self, plan: &PlanView, todo: usize) -> bool {
        self.reveal
            || self
                .todo_folds
                .get(&(plan.path.clone(), todo))
                .copied()
                .unwrap_or_else(|| plan.todos.get(todo).is_some_and(|t| !t.state.is_closed()))
    }

    /// Whether `item` of `task` shows the tasks nested under it: as the
    /// user left it, else while the item is not closed.
    pub(super) fn is_item_open(&self, task: usize, item: usize) -> bool {
        let Some(key) = self.item_key(task, item) else {
            return true;
        };
        self.reveal
            || self.item_folds.get(&key).copied().unwrap_or_else(|| {
                self.item(task, item)
                    .is_some_and(|item| !item.state.is_closed())
            })
    }

    /// Whether the round whose first request is `first` shows its items:
    /// as the user left it, else while `has_open_work`.
    pub(super) fn is_round_open(&self, first: usize, has_open_work: bool) -> bool {
        let id = self.tasks.get(first).map_or("", |task| task.id.as_str());
        self.reveal || self.round_folds.get(id).copied().unwrap_or(has_open_work)
    }

    /// What the highlighted row shows.
    pub(super) fn selected_key(&self) -> Option<RowKey> {
        let row = *self.rows.get(self.selected)?;
        let plan_path = |plan: usize| self.plans.get(plan).map(|plan| plan.path.clone());
        match row {
            TaskRow::Plan { plan: None, .. } => Some(RowKey::NoPlan),
            TaskRow::Plan {
                plan: Some(plan), ..
            } => plan_path(plan).map(RowKey::Plan),
            TaskRow::Todo { plan, todo, .. } => {
                plan_path(plan).map(|path| RowKey::Todo(path, todo))
            }
            TaskRow::Item { task, item, .. } => self
                .item_key(task, item)
                .map(|(agent, content)| RowKey::Item(agent, content)),
            TaskRow::Round { first, .. } => self
                .tasks
                .get(first)
                .map(|task| RowKey::Round(task.id.clone())),
            TaskRow::Task { task, .. } => self
                .tasks
                .get(task)
                .map(|task| RowKey::Task(task.id.clone())),
        }
    }

    /// Whether `row` shows what `key` names.
    pub(super) fn row_is(&self, row: TaskRow, key: &RowKey) -> bool {
        let plan_is =
            |plan: usize, path: &str| self.plans.get(plan).is_some_and(|p| p.path == path);
        match (row, key) {
            (TaskRow::Plan { plan: None, .. }, RowKey::NoPlan) => true,
            (
                TaskRow::Plan {
                    plan: Some(plan), ..
                },
                RowKey::Plan(path),
            ) => plan_is(plan, path.as_str()),
            (TaskRow::Todo { plan, todo, .. }, RowKey::Todo(path, wanted)) => {
                todo == *wanted && plan_is(plan, path.as_str())
            }
            (TaskRow::Item { task, item, .. }, RowKey::Item(agent, content)) => self
                .item_key(task, item)
                .is_some_and(|(a, c)| &a == agent && &c == content),
            (TaskRow::Task { task, .. }, RowKey::Task(id))
            | (TaskRow::Round { first: task, .. }, RowKey::Round(id)) => {
                self.tasks.get(task).is_some_and(|t| &t.id == id)
            }
            _ => false,
        }
    }
}
