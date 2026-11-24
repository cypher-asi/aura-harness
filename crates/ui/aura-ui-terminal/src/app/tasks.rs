//! The TASKS side tab: every piece of work agents did or are doing in the
//! active agent's workspace, grouped under the plan it works on. Each plan
//! is a foldable header with its todos, then the tasks working on it; the
//! plan being worked on comes first, then the other plans newest first.
//! A task that names no plan goes with the listed plan whose todo it
//! does. Tasks that work on no plan, or on one that is not listed, go
//! last under a `No Plan` header, shown only when there are such tasks.
//!
//! A user's request is a task, and each piece of it an agent handed to
//! another with `task` is a task under it, in the same group; each shows
//! what its agent has done and has left. Every row sits one level under
//! the row it belongs to: a group's todos and tasks under its header, a
//! task's items and then the tasks it handed out under the task.
//!
//! The request that builds a plan (`/build`, or a typed "implement") has
//! no row in that plan's group: the header stands for it, and its items
//! are the plan's todos. What it handed out moves up into the group, each
//! task under the todo that names it, the rest after the todos. Under No
//! Plan such a request is drawn like any other.
//!
//! ```text
//! ▾ Auth rework              1/3     depth 0: the plan
//!   ☒ Add session table              depth 1: its todos
//!   ▣ Write the login handler
//!     ⢎⡱ Write the login handler  → coder    depth 2: the build's part
//!   ☐ Wire the middleware
//!   ✓ Fix the typo  → coder          depth 1: a part naming no todo
//!   ✓ Rename the crate       2/2     depth 1: another request
//!     ☒ Find the uses                depth 2: its items, then its parts
//! ▾ No Plan
//!   ✓ Bump the version
//! ```
//!
//! Plans being worked on, with todos left or with a task still open,
//! start open, as do todos and tasks still being worked on or left with
//! work to do; the rest start folded. Folds the user makes stick across
//! updates.
//!
//! Only the tasks of the agents the side panel's scope lets through are
//! listed, with every task under them, and only the plans those tasks
//! work on or whose todos they do. The plan being worked on is always
//! listed. A request names its agent only when the scope spans the
//! project.

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
    /// A todo of an open plan, one level under its header; `fold` says
    /// whether the tasks nested under it show, `None` when it has none
    Todo {
        plan: usize,
        todo: usize,
        fold: Option<bool>,
    },
    /// A task's title row, `depth` levels under its group's header
    Task {
        task: usize,
        depth: usize,
        open: bool,
    },
    /// An item of an open task, one level under it
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

    /// Levels the row sits under its group's header: 0 for the header, 1
    /// for its todos and top-level tasks, one more for each level below.
    #[must_use]
    pub const fn depth(self) -> usize {
        match self {
            Self::Plan { .. } => 0,
            Self::Todo { .. } => 1,
            Self::Task { depth, .. } | Self::Item { depth, .. } => depth,
        }
    }

    /// Whether the row's plan, todo or task is open. Items, and todos
    /// with nothing nested, only show under an open one.
    const fn is_open(self) -> bool {
        match self {
            Self::Plan { open, .. } | Self::Task { open, .. } => open,
            Self::Todo {
                fold: Some(open), ..
            } => open,
            Self::Todo { fold: None, .. } | Self::Item { .. } => true,
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

/// The listed plans by what places a tree of tasks in their group: their
/// path, and the tasks their todos name. Where two plans match, the
/// first listed wins.
#[derive(Debug, Default)]
struct PlanIndex<'a> {
    by_path: HashMap<&'a str, usize>,
    by_todo_task: HashMap<&'a str, usize>,
}

impl PlanIndex<'_> {
    /// The listed plan whose group the tree rooted at `root` goes in:
    /// the plan it names, else the plan whose todo it does; `None` for
    /// the No Plan group. A named plan that is not listed puts the tree
    /// under No Plan even when a listed plan's todo names it.
    fn group_of(&self, root: &TaskView) -> Option<usize> {
        match root.plan.as_deref() {
            Some(path) => self.by_path.get(path).copied(),
            None => self.by_todo_task.get(root.id.as_str()).copied(),
        }
    }
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
    /// Todos the user folded (`false`) or opened (`true`), by plan path
    /// and todo index
    todo_folds: HashMap<(String, usize), bool>,
    /// Whether requests name their agent: the list spans the project
    show_agents: bool,
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

    /// Name each request's agent (`true`, for a list spanning the
    /// project), or not, since in one agent's scope it is the same agent.
    pub const fn set_show_agents(&mut self, show: bool) {
        self.show_agents = show;
    }

    /// Whether requests name their agent.
    #[must_use]
    pub const fn shows_agents(&self) -> bool {
        self.show_agents
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

    /// Highlight task `id`, opening its group, the todo it is nested
    /// under and the tasks it is part of. Returns whether it is listed.
    pub fn select_task(&mut self, id: &str) -> bool {
        let mut chain = vec![id.to_string()];
        let mut parent = self.find(id).and_then(|task| task.parent.clone());
        let mut seen = HashSet::new();
        while let Some(id) = parent.filter(|id| seen.insert(id.clone())) {
            self.folds.insert(id.clone(), true);
            parent = self.find(&id).and_then(|task| task.parent.clone());
            chain.push(id);
        }
        if let Some(root) = self.listed_root(id) {
            let group = self
                .tasks
                .get(root)
                .and_then(|root| self.plan_index().group_of(root));
            match group.and_then(|plan| self.plans.get(plan)) {
                Some(plan) => {
                    let path = plan.path.clone();
                    // A todo naming the task or one above it may hold it.
                    let todos: Vec<usize> = plan
                        .todos
                        .iter()
                        .enumerate()
                        .filter(|(_, todo)| todo.task.as_ref().is_some_and(|t| chain.contains(t)))
                        .map(|(i, _)| i)
                        .collect();
                    for todo in todos {
                        self.todo_folds.insert((path.clone(), todo), true);
                    }
                    self.plan_folds.insert(path, true);
                }
                None => self.no_plan_open = Some(true),
            }
        }
        self.rebuild(Some(RowKey::Task(id.to_string())));
        self.selected_task().is_some_and(|task| task.id == id)
    }

    /// Whether task `id` is the request building the plan whose group
    /// it is in, which has no row of its own.
    #[must_use]
    pub fn is_hidden_build(&self, id: &str) -> bool {
        let Some(root) = self.listed_root(id).and_then(|root| self.tasks.get(root)) else {
            return false;
        };
        root.id == id && self.hides(root, self.plan_index().group_of(root))
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

    /// Fold or open the group, todo or task of the highlighted row.
    /// Folding from a todo with nothing nested, or from an item, moves
    /// the highlight to its header.
    pub fn toggle_selected(&mut self) {
        let Some(&row) = self.rows.get(self.selected) else {
            return;
        };
        let open = !row.is_open();
        let plan_path = |plan: usize| self.plans.get(plan).map(|plan| plan.path.clone());
        let keep = match row {
            TaskRow::Plan { plan: None, .. } => {
                self.no_plan_open = Some(open);
                RowKey::NoPlan
            }
            TaskRow::Todo {
                plan,
                todo,
                fold: Some(_),
            } => {
                let Some(path) = plan_path(plan) else {
                    return;
                };
                self.todo_folds.insert((path.clone(), todo), open);
                RowKey::Todo(path, todo)
            }
            TaskRow::Plan {
                plan: Some(plan), ..
            }
            | TaskRow::Todo { plan, .. } => {
                let Some(path) = plan_path(plan) else {
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

    /// Whether todo `todo` of `plan` shows the tasks nested under it: as
    /// the user left it, else while the todo is not closed.
    fn is_todo_open(&self, plan: &PlanView, todo: usize) -> bool {
        self.todo_folds
            .get(&(plan.path.clone(), todo))
            .copied()
            .unwrap_or_else(|| plan.todos.get(todo).is_some_and(|t| !t.state.is_closed()))
    }

    /// Whether `root`, at the top of a tree in plan `group`'s group, is
    /// the request building that plan, which the header stands for.
    fn hides(&self, root: &TaskView, group: Option<usize>) -> bool {
        root.builds
            && group
                .and_then(|plan| self.plans.get(plan))
                .is_some_and(|plan| root.plan.as_deref() == Some(plan.path.as_str()))
    }

    /// The topmost listed task `id` is part of: the root of the tree it
    /// is drawn in. `None` when `id` is not listed.
    fn listed_root(&self, id: &str) -> Option<usize> {
        let is_listed = |index: usize| self.scoped.get(index).copied().unwrap_or(false);
        // The first task of an id, as a scan from the top would find it.
        let mut index: HashMap<&str, usize> = HashMap::with_capacity(self.tasks.len());
        for (i, task) in self.tasks.iter().enumerate() {
            index.entry(task.id.as_str()).or_insert(i);
        }
        let mut root = index.get(id).copied().filter(|&root| is_listed(root))?;
        let mut seen = HashSet::from([root]);
        while let Some(parent) = self
            .tasks
            .get(root)
            .and_then(|task| task.parent.as_deref())
            .and_then(|parent| index.get(parent).copied())
            .filter(|&parent| is_listed(parent) && seen.insert(parent))
        {
            root = parent;
        }
        Some(root)
    }

    /// The listed plans, for [`PlanIndex::group_of`].
    fn plan_index(&self) -> PlanIndex<'_> {
        let mut index = PlanIndex::default();
        let listed = self
            .plans
            .iter()
            .zip(&self.plans_shown)
            .enumerate()
            .filter_map(|(i, (plan, &shown))| shown.then_some((i, plan)));
        for (i, plan) in listed {
            index.by_path.entry(plan.path.as_str()).or_insert(i);
            for id in plan.todos.iter().filter_map(|todo| todo.task.as_deref()) {
                index.by_todo_task.entry(id).or_insert(i);
            }
        }
        index
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
            TaskRow::Todo { plan, todo, .. } => {
                plan_path(plan).map(|path| RowKey::Todo(path, todo))
            }
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
            (TaskRow::Todo { plan, todo, .. }, RowKey::Todo(path, wanted)) => {
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
    /// its depth, the tops at `depth`, whether folded or not. `seen`
    /// guards against parent loops.
    fn walk(
        &self,
        tops: &[usize],
        depth: usize,
        children: &HashMap<&str, Vec<usize>>,
        seen: &mut HashSet<usize>,
    ) -> Vec<(usize, usize)> {
        let mut tree = Vec::new();
        let mut stack: Vec<(usize, usize)> = tops.iter().rev().map(|&top| (top, depth)).collect();
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

    /// The tasks of plan `plan`'s group, the trees rooted at `tops`, as
    /// [`Self::walk`] lists them. A request building the plan has no row:
    /// each of its parts goes under the todo naming it, or with the rest
    /// in its place when no todo does.
    fn group_trees(
        &self,
        plan: usize,
        tops: &[usize],
        children: &HashMap<&str, Vec<usize>>,
        seen: &mut HashSet<usize>,
    ) -> GroupTrees {
        let todos = self.plans.get(plan).map_or(&[][..], |plan| &plan.todos);
        let mut links: HashMap<&str, usize> = HashMap::new();
        for (i, todo) in todos.iter().enumerate() {
            if let Some(id) = todo.task.as_deref() {
                links.entry(id).or_insert(i);
            }
        }
        let mut nested: Vec<Vec<usize>> = vec![Vec::new(); todos.len()];
        let mut rest = Vec::new();
        let mut has_open_task = false;
        for &top in tops {
            let Some(root) = self.tasks.get(top) else {
                continue;
            };
            if !self.hides(root, Some(plan)) {
                rest.push(top);
                continue;
            }
            if !seen.insert(top) {
                continue;
            }
            has_open_task |= !root.state.is_closed();
            let parts = children
                .get(root.id.as_str())
                .map_or(&[][..], Vec::as_slice);
            for &part in parts {
                let todo = self
                    .tasks
                    .get(part)
                    .and_then(|part| links.get(part.id.as_str()))
                    .and_then(|&todo| nested.get_mut(todo));
                match todo {
                    Some(under) => under.push(part),
                    None => rest.push(part),
                }
            }
        }
        let under_todos: Vec<_> = nested
            .iter()
            .map(|tops| self.walk(tops, 2, children, seen))
            .collect();
        let rest = self.walk(&rest, 1, children, seen);
        has_open_task |= under_todos
            .iter()
            .flatten()
            .chain(&rest)
            .any(|&(task, _)| !self.tasks[task].state.is_closed());
        GroupTrees {
            under_todos,
            rest,
            has_open_task,
        }
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
        let plan_index = self.plan_index();
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
                None => match plan_index.group_of(task) {
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
            let trees = self.group_trees(plan, &groups[plan], &children, &mut seen);
            let open = self.is_plan_open(view, trees.has_open_task);
            rows.push(TaskRow::Plan {
                plan: Some(plan),
                open,
            });
            if !open {
                continue;
            }
            for (todo, tree) in trees.under_todos.iter().enumerate() {
                let fold = (!tree.is_empty()).then(|| self.is_todo_open(view, todo));
                rows.push(TaskRow::Todo { plan, todo, fold });
                if fold == Some(true) {
                    self.task_rows(tree, &mut rows);
                }
            }
            self.task_rows(&trees.rest, &mut rows);
        }
        if !no_plan.is_empty() {
            let open = self.no_plan_open.unwrap_or(true);
            rows.push(TaskRow::Plan { plan: None, open });
            if open {
                let tree = self.walk(&no_plan, 1, &children, &mut seen);
                self.task_rows(&tree, &mut rows);
            }
        }
        rows
    }
}

/// A plan group's tasks with their depths, as [`TaskList::walk`] lists
/// them.
struct GroupTrees {
    /// The tasks nested under each of the plan's todos, by todo index
    under_todos: Vec<Vec<(usize, usize)>>,
    /// The other tasks, drawn after the todos
    rest: Vec<(usize, usize)>,
    /// Whether some task of the group, the hidden build too, is open
    has_open_task: bool,
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
    /// or todo, the plan a task works on, at its todo, on a task row. The
    /// No Plan header has none.
    pub(super) fn open_selected_row_plan(&mut self) {
        let row = self.task_list.rows.get(self.task_list.selected).copied();
        if matches!(row, Some(TaskRow::Plan { plan: None, .. })) {
            self.warn("No plan to open");
        } else if self.task_list.selected_plan().is_some() {
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
            self.warn("This task has no plan");
            return;
        };
        let todo = task.plan_todo.clone();
        self.open_plan_at(&plan, todo.as_deref());
    }

    /// Highlight the task doing the highlighted todo, or say why it
    /// can't: the row is not a todo, no task does it, it is the plan's
    /// build, which has no row, the scope hides the task, or the task is
    /// not loaded.
    pub(super) fn jump_to_todo_task(&mut self) {
        let list = &self.task_list;
        let Some(&TaskRow::Todo { plan, todo, .. }) = list.rows.get(list.selected) else {
            self.warn("Select a todo to jump to its task");
            return;
        };
        let task = list
            .plans
            .get(plan)
            .and_then(|plan| plan.todos.get(todo))
            .and_then(|todo| todo.task.clone());
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
        self.notification = Some((message.into(), NotificationType::Warning));
    }

    /// Clicking a plan's, todo's or task's fold glyph folds or opens it;
    /// clicking anywhere else on a row opens it as Enter does.
    pub(super) fn click_task_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.task_row_at(column, row) else {
            return;
        };
        self.task_list.select(index);
        let Some(&target) = self.task_list.rows.get(index) else {
            return;
        };
        let has_fold = matches!(
            target,
            TaskRow::Plan { .. } | TaskRow::Task { .. } | TaskRow::Todo { fold: Some(_), .. }
        );
        let on_fold = has_fold
            && self
                .areas
                .task_rows
                .is_some_and(|rows| super::processes::on_row_icon(column, rows.x, target.depth()));
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

    /// The rows as text, each indented two spaces per level: `-`/`+`
    /// before an open/folded plan, todo or task, `#` before a todo.
    fn shape(list: &TaskList) -> Vec<String> {
        let fold = |open| if open { "-" } else { "+" };
        list.rows()
            .iter()
            .map(|&row| {
                let text = match row {
                    TaskRow::Plan { plan, open } => format!(
                        "{}{}",
                        fold(open),
                        plan.map_or("No Plan", |plan| list.plans()[plan].name.as_str())
                    ),
                    TaskRow::Todo {
                        plan,
                        todo,
                        fold: folded,
                    } => format!(
                        "#{}{}",
                        folded.map_or("", fold),
                        list.plans()[plan].todos[todo].content
                    ),
                    TaskRow::Task { task, open, .. } => {
                        format!("{}{}", fold(open), list.tasks()[task].id)
                    }
                    TaskRow::Item { task, item, .. } => {
                        list.tasks()[task].items[item].content.clone()
                    }
                };
                format!("{}{text}", "  ".repeat(row.depth()))
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
            [
                "-No Plan",
                "  -a",
                "    a 0",
                "    -c",
                "      c 0",
                "    +b",
                "  +old"
            ],
            "parts newest first, like the requests, each a level under its parent"
        );
    }

    #[test]
    fn an_update_keeps_the_highlight_and_a_new_task_goes_on_top() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("a", None, R, 2)]);
        list.select(3);
        list.upsert(task("b", None, R, 0));
        assert_eq!(
            shape(&list),
            ["-No Plan", "  -b", "  -a", "    a 0", "    a 1"]
        );
        assert_eq!(list.selected(), 4, "still on a's second item");

        list.upsert(task("a", None, C, 2));
        assert_eq!(
            shape(&list),
            ["-No Plan", "  -b", "  +a"],
            "a closed and folds"
        );
        list.select(2);
        list.toggle_selected();
        assert_eq!(shape(&list).len(), 5, "the user opens it again");
    }

    #[test]
    fn selecting_a_folded_part_opens_its_request_and_group() {
        let mut list = TaskList::default();
        list.set_tasks(vec![task("b", Some("a"), C, 0), task("a", None, C, 0)]);
        assert_eq!(shape(&list), ["-No Plan", "  +a"]);
        list.select(0);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+No Plan"]);
        assert!(list.select_task("b"));
        assert_eq!(shape(&list), ["-No Plan", "  -a", "    +b"]);
        assert_eq!(list.selected(), 2);
        assert!(!list.select_task("missing"));

        list.set_plans(vec![plan("p", &[Done])]);
        list.set_tasks(vec![planned("t", "p", C)]);
        assert_eq!(shape(&list), ["+p"], "a done plan with done tasks folds");
        assert!(list.select_task("t"));
        assert_eq!(shape(&list), ["-p", "  #p 0", "  +t"]);
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
            [
                "-auth",
                "  #auth 0",
                "  #auth 1",
                "  -build",
                "    -part",
                "+parser"
            ],
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
            ["-a", "  #a 0", "  -root", "    -part", "-b", "  #b 0"]
        );
    }

    #[test]
    fn a_task_naming_no_plan_goes_with_the_plan_whose_todo_it_does() {
        let mut list = TaskList::default();
        let mut a = plan("a", &[Todo, Todo]);
        a.todos[0].task = Some("linked".to_string());
        a.todos[1].task = Some("own".to_string());
        list.set_plans(vec![a, plan("b", &[Todo])]);
        list.set_tasks(vec![task("linked", None, R, 0), planned("own", "b", R)]);
        assert_eq!(
            shape(&list),
            [
                "-a",
                "  #a 0",
                "  #a 1",
                "  -linked",
                "-b",
                "  #b 0",
                "  -own"
            ],
            "a's todo names both, but own works on b; a request nests under no todo"
        );
    }

    /// The text of the notification on screen.
    fn notice(app: &App) -> Option<&str> {
        app.notification().map(|(text, _)| text.as_str())
    }

    #[test]
    fn t_and_p_say_why_they_have_nothing_to_show() {
        let mut app = App::new();
        let mut a = plan("a", &[Todo, Todo]);
        a.todos[0].task = Some("gone".to_string());
        app.process_command(UiCommand::SetPlans(vec![a]));
        app.process_command(UiCommand::SetTasks(vec![task("loose", None, R, 0)]));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Tasks);
        assert_eq!(
            shape(app.task_list()),
            ["-a", "  #a 0", "  #a 1", "-No Plan", "  -loose"]
        );
        let press = |app: &mut App, code: KeyCode| app.handle_key(KeyEvent::from(code));

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("Select a todo to jump to its task"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("That todo's task isn't loaded"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("No task is doing this todo"));

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(notice(&app), Some("No plan to open"));
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("Select a todo to jump to its task"));

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(notice(&app), Some("This task has no plan"));
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("Select a todo to jump to its task"));
        assert_eq!(shape(app.task_list()).len(), 5, "nothing folded");
    }

    #[test]
    fn no_plan_lists_tasks_of_no_listed_plan_and_only_when_there_are_some() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo])]);
        assert_eq!(shape(&list), ["-a", "  #a 0"], "no No Plan header");

        list.set_tasks(vec![
            planned("gone", "missing", R),
            task("loose", None, R, 0),
            planned("t", "a", R),
        ]);
        assert_eq!(
            shape(&list),
            ["-a", "  #a 0", "  -t", "-No Plan", "  -gone", "  -loose"]
        );
    }

    #[test]
    fn a_plan_with_no_tasks_still_lists_its_todos() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("fresh", &[Todo, Todo])]);
        assert_eq!(shape(&list), ["-fresh", "  #fresh 0", "  #fresh 1"]);
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
            [
                "-todos",
                "  #todos 0",
                "  #todos 1",
                "-task",
                "  #task 0",
                "  -t",
                "+done"
            ]
        );
    }

    #[test]
    fn the_current_plan_comes_first_and_starts_open_even_when_done() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("a", &[Todo]), plan("b", &[Done])]);
        list.set_current_plan(Some(&plan("b", &[Done])));
        assert_eq!(shape(&list), ["-b", "  #b 0", "-a", "  #a 0"]);
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
            ["+a", "-b", "  #b 0", "-No Plan", "  -loose", "    loose 0"]
        );
        assert_eq!(list.selected(), 0);
        list.select(3);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+a", "-b", "  #b 0", "+No Plan"]);

        list.set_plans(vec![plan("a", &[Todo, Todo, Todo]), plan("b", &[Todo])]);
        list.set_tasks(vec![task("new", None, R, 0), task("loose", None, R, 1)]);
        assert_eq!(
            shape(&list),
            ["+a", "-b", "  #b 0", "+No Plan"],
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
        assert_eq!(
            shape(app.task_list()),
            ["-No Plan", "  -a", "    a 0", "    -b"]
        );

        app.handle_key(KeyEvent::from(KeyCode::End));
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(switched(&mut rx).as_deref(), Some("agent-b"));

        app.handle_key(KeyEvent::from(KeyCode::Home));
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(shape(app.task_list()), ["-No Plan", "  +a"]);
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
        assert_eq!(shape(app.task_list()), ["-a", "  #a 0", "  #a 1", "  -t"]);

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
            ["-a", "  #a 0", "  -t1", "-No Plan", "  -t2"],
            "t1 does a's todo, so it sits under a"
        );

        app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert_eq!(
            notice(&app),
            Some("Select a todo to jump to its task"),
            "a plan header has no task"
        );
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

        // `a` sits a level under No Plan, so its icon starts at column 3
        // of the rows; column 1 is No Plan's fold glyph.
        app.click_task_row(13, 6);
        assert_eq!(shape(app.task_list()), ["-No Plan", "  +a"]);
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

    /// Plan `auth` with todos `auth 0`, which `p1` does, and `auth 1`;
    /// `build` builds it with two items of its own and hands out `p1`,
    /// with an item, and then `p2`.
    fn building() -> (PlanView, Vec<TaskView>) {
        let mut auth = plan("auth", &[Doing, Todo]);
        auth.todos[0].task = Some("p1".to_string());
        let build = TaskView {
            plan: Some(path("auth")),
            builds: true,
            ..task("build", None, R, 2)
        };
        let tasks = vec![
            task("p2", Some("build"), R, 0),
            task("p1", Some("build"), R, 1),
            build,
        ];
        (auth, tasks)
    }

    /// An app on the focused TASKS tab listing `plans` and `tasks`.
    fn app_on_tasks(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetPlans(plans));
        app.process_command(UiCommand::SetTasks(tasks));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Tasks);
        app
    }

    #[test]
    fn the_build_request_has_no_row_and_its_parts_move_up_into_the_group() {
        let (auth, tasks) = building();
        let mut list = TaskList::default();
        list.set_plans(vec![auth]);
        list.set_tasks(tasks);
        assert_eq!(
            shape(&list),
            [
                "-auth",
                "  #-auth 0",
                "    -p1",
                "      p1 0",
                "  #auth 1",
                "  -p2"
            ],
            "p1 under the todo it does, p2 after the todos, none of build's items"
        );
        let build = list.tasks().iter().position(|t| t.id == "build");
        assert!(
            list.rows().iter().all(|row| row.task() != build),
            "no row of the build: {:?}",
            list.rows()
        );
        assert!(list.is_hidden_build("build"));
        assert!(!list.is_hidden_build("p1"), "only the request is hidden");
    }

    #[test]
    fn a_build_whose_plan_is_not_listed_gets_a_row_under_no_plan() {
        let (_, tasks) = building();
        let mut list = TaskList::default();
        list.set_plans(vec![plan("other", &[Todo])]);
        list.set_tasks(tasks);
        assert_eq!(
            shape(&list),
            [
                "-other",
                "  #other 0",
                "-No Plan",
                "  -build",
                "    build 0",
                "    build 1",
                "    -p1",
                "      p1 0",
                "    -p2"
            ]
        );
        assert!(!list.is_hidden_build("build"));
    }

    #[test]
    fn a_tasks_items_and_then_its_parts_sit_one_level_under_it() {
        let mut list = TaskList::default();
        list.set_plans(vec![plan("p", &[Todo])]);
        list.set_tasks(vec![
            task("part", Some("request"), R, 1),
            TaskView {
                plan: Some(path("p")),
                ..task("request", None, R, 2)
            },
        ]);
        assert_eq!(
            shape(&list),
            [
                "-p",
                "  #p 0",
                "  -request",
                "    request 0",
                "    request 1",
                "    -part",
                "      part 0"
            ]
        );
        let depths: Vec<usize> = list.rows().iter().map(|row| row.depth()).collect();
        assert_eq!(depths, [0, 1, 1, 2, 2, 2, 3]);
    }

    #[test]
    fn requests_name_their_agent_only_across_the_project() {
        let mut app = app_on_tasks(Vec::new(), vec![task("a", None, R, 0)]);
        assert_eq!(app.side_scope(), SideScope::Agent);
        assert!(!app.task_list().shows_agents());
        app.toggle_side_scope();
        assert!(app.task_list().shows_agents());
        app.toggle_side_scope();
        assert!(!app.task_list().shows_agents(), "back to the agent's scope");

        let mut list = TaskList::default();
        assert!(!list.shows_agents(), "a new list names no agent");
        list.set_show_agents(true);
        assert!(list.shows_agents());
    }

    #[test]
    fn a_todo_with_a_part_under_it_folds_and_opens() {
        let (auth, tasks) = building();
        let mut app = app_on_tasks(vec![auth], tasks);
        let press = |app: &mut App, code: KeyCode| app.handle_key(KeyEvent::from(code));
        let open = [
            "-auth",
            "  #-auth 0",
            "    -p1",
            "      p1 0",
            "  #auth 1",
            "  -p2",
        ];
        let folded = ["-auth", "  #+auth 0", "  #auth 1", "  -p2"];
        assert_eq!(shape(app.task_list()), open);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!(shape(app.task_list()), folded, "Left hides p1");
        assert_eq!(app.task_list().selected(), 1, "the highlight stays on it");
        press(&mut app, KeyCode::Left);
        assert_eq!(
            shape(app.task_list()),
            folded,
            "Left on a folded todo leaves the plan open"
        );
        press(&mut app, KeyCode::Right);
        assert_eq!(shape(app.task_list()), open, "Right shows p1 again");

        press(&mut app, KeyCode::Char(' '));
        assert_eq!(shape(app.task_list()), folded, "Space folds it");
        let (auth, tasks) = building();
        app.process_command(UiCommand::SetPlans(vec![auth]));
        app.process_command(UiCommand::SetTasks(tasks));
        assert_eq!(shape(app.task_list()), folded, "the fold survives updates");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(shape(app.task_list()), open, "Space opens it");
    }

    #[test]
    fn t_on_a_todo_selects_its_nested_part_or_says_it_is_the_build() {
        let (mut auth, tasks) = building();
        auth.todos[1].task = Some("build".to_string());
        let mut app = app_on_tasks(vec![auth], tasks);
        let press = |app: &mut App, code: KeyCode| app.handle_key(KeyEvent::from(code));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!(shape(app.task_list())[1], "  #+auth 0");

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(
            app.task_list().selected_task().map(|t| t.id.as_str()),
            Some("p1")
        );
        assert_eq!(
            shape(app.task_list())[app.task_list().selected()],
            "    -p1",
            "the todo opens to show it"
        );

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            shape(app.task_list())[app.task_list().selected()],
            "  #auth 1"
        );
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("This todo is part of the plan's build"));
        assert_eq!(app.task_list().selected(), 4, "the highlight stays put");
    }
}
