//! The TASKS side tab: the work agents did or are doing in the active
//! agent's workspace, grouped under the plan it works on. Each plan is a
//! foldable header with its todos; the plan being worked on comes first,
//! then the other plans newest first. Work on no listed plan goes last
//! under a `No Plan` header, shown only when it has rows.
//!
//! What the user asked is not listed: a request (a task no other task
//! handed out) has no row. Its todo list is listed instead, item by item,
//! under its group's header. An agent's todo list carries over from one
//! request to the next, so each agent's items are merged across its
//! requests in the group: an item keeps the place it first had and shows
//! how its newest copy stands. The request that builds a plan (`/build`,
//! or a typed "implement") adds no items to that plan's group, whose
//! todos are its list.
//!
//! A task an agent handed out with `task` sits under the todo or item it
//! does, one level in; one doing none sits beside the items of the
//! request it is part of. The tasks it handed out in turn sit one level
//! under it. Nothing else is indented, so a row's icon sits right under
//! its header's fold arrow.
//!
//! ```text
//! ▾  Auth rework              1/3     a plan
//! ☒  Add session table                its todos
//! ▣  Write the login handler
//!   ⢎⡱ Write the login handler  → coder    the task doing it
//! ☐  Wire the middleware
//! ▾  No Plan
//! ☒  Find the uses  → explorer        a request's items
//!   ✓  Find the uses of calc_total  → explorer
//! ☐  Rename the crate
//! ✓  Fix the typo  → coder            a task doing no item
//! ```
//!
//! Plans being worked on, with todos left or with work still open, start
//! open, as do todos and items not done yet and tasks still being worked
//! on; the rest start folded. Only rows with something under them fold.
//! Folds the user makes stick across updates.
//!
//! Only the tasks of the agents the side panel's scope lets through are
//! listed, with every task under them, and only the plans those tasks
//! work on or whose todos they do. The plan being worked on is always
//! listed.

use std::collections::{HashMap, HashSet};

use super::processes::SIDE_PAGE;
use super::side_scope::AgentFilter;
use super::{contains, follow_selection, App, NotificationType, SideScope};
use crate::events::{PlanView, TaskView, TodoView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// One row of the tab. `fold` is whether what is under the row shows,
/// `None` when nothing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRow {
    /// A group's header: a plan, or `None` for the No Plan group
    Plan { plan: Option<usize>, open: bool },
    /// A todo of an open plan
    Todo {
        plan: usize,
        todo: usize,
        fold: Option<bool>,
    },
    /// An item of an agent's requests' todo lists: `item` of `task`, the
    /// newest request holding it. `link` is the task doing it, when
    /// loaded.
    Item {
        task: usize,
        item: usize,
        link: Option<usize>,
        fold: Option<bool>,
    },
    /// A task an agent handed out, `depth` levels in: 0 beside the items,
    /// one more under the todo, item or task it belongs to
    Task {
        task: usize,
        depth: usize,
        fold: Option<bool>,
    },
}

impl TaskRow {
    /// Index of the task the row belongs to: the task itself, or the
    /// request an item was last listed by; `None` on plans and todos.
    #[must_use]
    pub const fn task(self) -> Option<usize> {
        match self {
            Self::Task { task, .. } | Self::Item { task, .. } => Some(task),
            Self::Plan { .. } | Self::Todo { .. } => None,
        }
    }

    /// Index of the plan of a plan header or todo; `None` on the No Plan
    /// header, items and tasks.
    #[must_use]
    pub const fn plan(self) -> Option<usize> {
        match self {
            Self::Plan { plan, .. } => plan,
            Self::Todo { plan, .. } => Some(plan),
            Self::Task { .. } | Self::Item { .. } => None,
        }
    }

    /// Levels the row is indented: only tasks under a todo, an item or
    /// another task are.
    #[must_use]
    pub const fn depth(self) -> usize {
        match self {
            Self::Task { depth, .. } => depth,
            Self::Plan { .. } | Self::Todo { .. } | Self::Item { .. } => 0,
        }
    }

    /// Whether what is under the row shows; `None` when nothing is.
    #[must_use]
    pub const fn fold(self) -> Option<bool> {
        match self {
            Self::Plan { open, .. } => Some(open),
            Self::Todo { fold, .. } | Self::Item { fold, .. } | Self::Task { fold, .. } => fold,
        }
    }
}

/// What a row shows, by path, id and content rather than index, so the
/// highlight can find it again after the lists change.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Plan(String),
    NoPlan,
    Todo(String, usize),
    /// An item, by agent id and content
    Item(String, String),
    Task(String),
}

/// The listed plans by what places a request in their group: their path,
/// and the tasks their todos name. Where two plans match, the first
/// listed wins.
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

/// One agent's merged items in a group, with the tasks nested under
/// them and those under none.
struct AgentItems {
    /// `(task, item, link)` of each item, in the order first listed
    items: Vec<(usize, usize, Option<usize>)>,
    /// The tasks under each item, by its place in `items`
    nested: Vec<Vec<usize>>,
    /// Tasks doing none of the items or todos
    loose: Vec<usize>,
}

/// The rows of a group under its header, and whether any of its work is
/// still open.
struct GroupBody {
    rows: Vec<TaskRow>,
    has_open_task: bool,
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
    /// Items the user folded (`false`) or opened (`true`), by agent id
    /// and content
    item_folds: HashMap<(String, String), bool>,
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

    /// The item of an item row, as its newest copy has it.
    #[must_use]
    pub fn item(&self, task: usize, item: usize) -> Option<&TodoView> {
        self.tasks.get(task)?.items.get(item)
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

    /// Highlight task `id`, opening its group, the todo or item it is
    /// nested under and the tasks it is part of. Returns whether it has a
    /// row; a request has none, so its group's header is highlighted.
    pub fn select_task(&mut self, id: &str) -> bool {
        let mut chain = vec![id.to_string()];
        let mut parent = self.find(id).and_then(|task| task.parent.clone());
        let mut seen = HashSet::new();
        while let Some(id) = parent.filter(|id| seen.insert(id.clone())) {
            self.folds.insert(id.clone(), true);
            parent = self.find(&id).and_then(|task| task.parent.clone());
            chain.push(id);
        }
        let links = |todo: &TodoView| todo.task.as_ref().is_some_and(|t| chain.contains(t));
        let items: Vec<(String, String)> = self
            .scoped_tasks()
            .filter(|task| task.parent.is_none())
            .flat_map(|task| {
                task.items
                    .iter()
                    .filter(|item| links(item))
                    .map(|item| (task.agent_id.clone(), item.content.clone()))
            })
            .collect();
        for key in items {
            self.item_folds.insert(key, true);
        }
        let mut group_key = None;
        if let Some(root) = self.listed_root(id) {
            let group = self
                .tasks
                .get(root)
                .and_then(|root| self.plan_index().group_of(root));
            match group.and_then(|plan| self.plans.get(plan)) {
                Some(plan) => {
                    let path = plan.path.clone();
                    let todos: Vec<usize> = plan
                        .todos
                        .iter()
                        .enumerate()
                        .filter(|(_, todo)| links(todo))
                        .map(|(i, _)| i)
                        .collect();
                    for todo in todos {
                        self.todo_folds.insert((path.clone(), todo), true);
                    }
                    self.plan_folds.insert(path.clone(), true);
                    group_key = Some(RowKey::Plan(path));
                }
                None => {
                    self.no_plan_open = Some(true);
                    group_key = Some(RowKey::NoPlan);
                }
            }
        }
        self.rebuild(Some(RowKey::Task(id.to_string())));
        let found = self
            .rows
            .get(self.selected)
            .is_some_and(|&row| self.row_is(row, &RowKey::Task(id.to_string())));
        if !found {
            if let Some(key) = group_key {
                if let Some(index) = self.rows.iter().position(|&row| self.row_is(row, &key)) {
                    self.select(index);
                }
            }
        }
        found
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
            // Rows at the left edge are under their group's header.
            let depth = row.depth();
            let above = self.rows.get(..self.selected).and_then(|rows| {
                rows.iter().rposition(|&above| {
                    if depth == 0 {
                        matches!(above, TaskRow::Plan { .. })
                    } else {
                        above.depth() < depth
                    }
                })
            });
            if let Some(above) = above {
                self.selected = above;
                self.toggle_selected();
            }
            return;
        };
        let plan_path = |plan: usize| self.plans.get(plan).map(|plan| plan.path.clone());
        let keep = match row {
            TaskRow::Plan { plan: None, .. } => {
                self.no_plan_open = Some(open);
                RowKey::NoPlan
            }
            TaskRow::Plan {
                plan: Some(plan), ..
            } => {
                let Some(path) = plan_path(plan) else {
                    return;
                };
                self.plan_folds.insert(path.clone(), open);
                RowKey::Plan(path)
            }
            TaskRow::Todo { plan, todo, .. } => {
                let Some(path) = plan_path(plan) else {
                    return;
                };
                self.todo_folds.insert((path.clone(), todo), open);
                RowKey::Todo(path, todo)
            }
            TaskRow::Item { task, item, .. } => {
                let Some(key) = self.item_key(task, item) else {
                    return;
                };
                self.item_folds.insert(key.clone(), open);
                RowKey::Item(key.0, key.1)
            }
            TaskRow::Task { task, .. } => {
                let Some(id) = self.tasks.get(task).map(|task| task.id.clone()) else {
                    return;
                };
                self.folds.insert(id.clone(), open);
                RowKey::Task(id)
            }
        };
        self.rebuild(Some(keep));
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

    fn find(&self, id: &str) -> Option<&TaskView> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// Agent id and content of `item` of `task`, which key its fold.
    fn item_key(&self, task: usize, item: usize) -> Option<(String, String)> {
        let view = self.tasks.get(task)?;
        let item = view.items.get(item)?;
        Some((view.agent_id.clone(), item.content.clone()))
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

    /// Whether `item` of `task` shows the tasks nested under it: as the
    /// user left it, else while the item is not closed.
    fn is_item_open(&self, task: usize, item: usize) -> bool {
        let Some(key) = self.item_key(task, item) else {
            return true;
        };
        self.item_folds.get(&key).copied().unwrap_or_else(|| {
            self.item(task, item)
                .is_some_and(|item| !item.state.is_closed())
        })
    }

    /// Whether `root`, a request in plan `group`'s group, is the request
    /// building that plan, whose list the plan's todos are.
    fn hides(&self, root: &TaskView, group: Option<usize>) -> bool {
        root.builds
            && group
                .and_then(|plan| self.plans.get(plan))
                .is_some_and(|plan| root.plan.as_deref() == Some(plan.path.as_str()))
    }

    /// The topmost listed task `id` is part of: the request its tree
    /// hangs from. `None` when `id` is not listed.
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
            TaskRow::Task { task, .. } => self
                .tasks
                .get(task)
                .map(|task| RowKey::Task(task.id.clone())),
        }
    }

    /// Whether `row` shows what `key` names.
    fn row_is(&self, row: TaskRow, key: &RowKey) -> bool {
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
            (TaskRow::Task { task, .. }, RowKey::Task(id)) => {
                self.tasks.get(task).is_some_and(|t| &t.id == id)
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
    /// guards against parent loops and tasks listed twice.
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
            let Some(view) = self.tasks.get(task) else {
                continue;
            };
            if !seen.insert(task) {
                continue;
            }
            tree.push((task, depth));
            if let Some(parts) = children.get(view.id.as_str()) {
                stack.extend(parts.iter().rev().map(|&part| (part, depth + 1)));
            }
        }
        tree
    }

    /// Rows of the tasks in `tree`, as [`Self::walk`] lists them, leaving
    /// out what is under a folded task.
    fn task_rows(
        &self,
        tree: &[(usize, usize)],
        children: &HashMap<&str, Vec<usize>>,
        rows: &mut Vec<TaskRow>,
    ) {
        let mut folded_at = None;
        for &(task, depth) in tree {
            if folded_at.is_some_and(|folded| depth > folded) {
                continue;
            }
            folded_at = None;
            let Some(view) = self.tasks.get(task) else {
                continue;
            };
            let has_parts = children
                .get(view.id.as_str())
                .is_some_and(|parts| !parts.is_empty());
            let fold = has_parts.then(|| self.is_open(view));
            rows.push(TaskRow::Task { task, depth, fold });
            if fold == Some(false) {
                folded_at = Some(depth);
            }
        }
    }

    /// One agent's items in a group: the items of its `requests` (newest
    /// first), merged, with the tasks they handed out placed under the
    /// item doing them, or loose when no item or one of `todo_links` does.
    fn agent_items(
        &self,
        requests: &[usize],
        group: Option<usize>,
        todo_links: &HashMap<&str, usize>,
        index: &HashMap<&str, usize>,
        children: &HashMap<&str, Vec<usize>>,
    ) -> AgentItems {
        let mut items: Vec<(usize, usize, Option<&str>)> = Vec::new();
        let mut at: HashMap<&str, usize> = HashMap::new();
        let mut parts: Vec<usize> = Vec::new();
        for &request in requests.iter().rev() {
            let Some(view) = self.tasks.get(request) else {
                continue;
            };
            if let Some(handed) = children.get(view.id.as_str()) {
                parts.extend(handed);
            }
            if self.hides(view, group) {
                continue;
            }
            for (i, item) in view.items.iter().enumerate() {
                match at.get(item.content.as_str()) {
                    // A later copy says how it stands; the link is kept
                    // when the later list was not handed the task.
                    Some(&k) => {
                        let link = item.task.as_deref().or(items[k].2);
                        items[k] = (request, i, link);
                    }
                    None => {
                        at.insert(item.content.as_str(), items.len());
                        items.push((request, i, item.task.as_deref()));
                    }
                }
            }
        }
        let item_links: HashMap<&str, usize> = items
            .iter()
            .enumerate()
            .filter_map(|(k, &(_, _, link))| link.map(|link| (link, k)))
            .collect();
        let mut nested = vec![Vec::new(); items.len()];
        let mut loose = Vec::new();
        for part in parts {
            let Some(id) = self.tasks.get(part).map(|task| task.id.as_str()) else {
                continue;
            };
            if todo_links.contains_key(id) {
                continue;
            }
            match item_links.get(id) {
                Some(&k) => nested[k].push(part),
                None => loose.push(part),
            }
        }
        AgentItems {
            items: items
                .into_iter()
                .map(|(task, item, link)| (task, item, link.and_then(|id| index.get(id).copied())))
                .collect(),
            nested,
            loose,
        }
    }

    /// The rows under the header of plan `group`'s group (`None` for No
    /// Plan), whose requests are `tops`, newest first.
    fn group_body(
        &self,
        group: Option<usize>,
        tops: &[usize],
        (index, children): (&HashMap<&str, usize>, &HashMap<&str, Vec<usize>>),
        seen: &mut HashSet<usize>,
    ) -> GroupBody {
        let plan = group.and_then(|plan| self.plans.get(plan));
        let todos = plan.map_or(&[][..], |plan| &plan.todos);
        let mut todo_links: HashMap<&str, usize> = HashMap::new();
        for (i, todo) in todos.iter().enumerate() {
            if let Some(id) = todo.task.as_deref() {
                todo_links.entry(id).or_insert(i);
            }
        }

        // Each agent's requests, newest first, the agent last at work
        // first. A task handed out by a task the scope leaves out is a
        // task still, listed after the items.
        let mut agents: Vec<&str> = Vec::new();
        let mut requests: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut orphans: Vec<usize> = Vec::new();
        let mut has_open_task = false;
        for &top in tops {
            let Some(view) = self.tasks.get(top) else {
                continue;
            };
            has_open_task |= !view.state.is_closed();
            if view.parent.is_some() {
                if !todo_links.contains_key(view.id.as_str()) {
                    orphans.push(top);
                }
                continue;
            }
            let agent = view.agent_id.as_str();
            if !requests.contains_key(agent) {
                agents.push(agent);
            }
            requests.entry(agent).or_default().push(top);
        }

        // The tasks under the plan's todos: what a request handed out
        // for one, wherever the request is listed, and such a task whose
        // request is left out.
        let mut under_todos: Vec<Vec<usize>> = vec![Vec::new(); todos.len()];
        for &top in tops {
            let Some(view) = self.tasks.get(top) else {
                continue;
            };
            let handed = match view.parent {
                Some(_) => std::slice::from_ref(&top),
                None => children
                    .get(view.id.as_str())
                    .map_or(&[][..], Vec::as_slice),
            };
            for &part in handed {
                let todo = self
                    .tasks
                    .get(part)
                    .and_then(|part| todo_links.get(part.id.as_str()))
                    .and_then(|&todo| under_todos.get_mut(todo));
                if let Some(under) = todo {
                    under.push(part);
                }
            }
        }

        let mut rows = Vec::new();
        let mut trees = Vec::new();
        for (todo, tops) in under_todos.iter().enumerate() {
            let tree = self.walk(tops, 1, children, seen);
            let fold = match plan {
                Some(plan) if !tree.is_empty() => Some(self.is_todo_open(plan, todo)),
                _ => None,
            };
            if let Some(plan) = group {
                rows.push(TaskRow::Todo { plan, todo, fold });
            }
            if fold == Some(true) {
                self.task_rows(&tree, children, &mut rows);
            }
            trees.push(tree);
        }
        for agent in agents {
            let Some(requests) = requests.get(agent) else {
                continue;
            };
            let block = self.agent_items(requests, group, &todo_links, index, children);
            for (&(task, item, link), nested) in block.items.iter().zip(&block.nested) {
                let tree = self.walk(nested, 1, children, seen);
                let fold = (!tree.is_empty()).then(|| self.is_item_open(task, item));
                rows.push(TaskRow::Item {
                    task,
                    item,
                    link,
                    fold,
                });
                if fold == Some(true) {
                    self.task_rows(&tree, children, &mut rows);
                }
                trees.push(tree);
            }
            let tree = self.walk(&block.loose, 0, children, seen);
            self.task_rows(&tree, children, &mut rows);
            trees.push(tree);
        }
        let tree = self.walk(&orphans, 0, children, seen);
        self.task_rows(&tree, children, &mut rows);
        trees.push(tree);
        has_open_task |= trees
            .iter()
            .flatten()
            .any(|&(task, _)| self.tasks.get(task).is_some_and(|t| !t.state.is_closed()));
        GroupBody {
            rows,
            has_open_task,
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
        let mut index: HashMap<&str, usize> = HashMap::with_capacity(self.tasks.len());
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); self.plans.len()];
        let mut no_plan = Vec::new();
        for (i, task) in self.tasks.iter().enumerate() {
            index.entry(task.id.as_str()).or_insert(i);
            if !self.scoped[i] {
                continue;
            }
            match task
                .parent
                .as_deref()
                .filter(|parent| listed.contains(parent))
            {
                Some(parent) => children.entry(parent).or_default().push(i),
                None => match plan_index.group_of(task) {
                    Some(plan) => groups[plan].push(i),
                    None => no_plan.push(i),
                },
            }
        }
        // A task's parts in the order given.
        for parts in children.values_mut() {
            parts.reverse();
        }

        let mut order: Vec<usize> = (0..self.plans.len())
            .filter(|&plan| self.plans_shown[plan])
            .collect();
        order.sort_by_key(|&plan| !self.is_current(&self.plans[plan]));

        let lookups = (&index, &children);
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for plan in order {
            let body = self.group_body(Some(plan), &groups[plan], lookups, &mut seen);
            let open = self.is_plan_open(&self.plans[plan], body.has_open_task);
            rows.push(TaskRow::Plan {
                plan: Some(plan),
                open,
            });
            if open {
                rows.extend(body.rows);
            }
        }
        let body = self.group_body(None, &no_plan, lookups, &mut seen);
        if !body.rows.is_empty() {
            let open = self.no_plan_open.unwrap_or(true);
            rows.push(TaskRow::Plan { plan: None, open });
            if open {
                rows.extend(body.rows);
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

    /// Keys of the TASKS tab: Enter opens the row, Right unfolds what is
    /// under it and Left folds it. `p`, `t` and Space are side-list
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
    /// the agent of a task, or of the request an item is on. The No Plan
    /// header has nothing to open, so it folds.
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
    /// or todo; on a task, or an item of a request, the plan it works on,
    /// at the todo it was handed, else its group's plan. The No Plan group
    /// has none.
    pub(super) fn open_selected_row_plan(&mut self) {
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
    pub(super) fn jump_to_todo_task(&mut self) {
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
        self.notification = Some((message.into(), NotificationType::Warning));
    }

    /// Clicking a row's fold glyph folds or opens what is under it;
    /// clicking anywhere else on a row opens it as Enter does.
    pub(super) fn click_task_row(&mut self, column: u16, row: u16) {
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

    use TaskState::{Completed as C, Running as R};
    use TodoState::{Completed as Done, InProgress as Doing, Pending as Todo};

    fn switched(rx: &mut mpsc::Receiver<UiEvent>) -> Option<String> {
        match rx.try_recv() {
            Ok(UiEvent::SelectAgent(id)) => Some(id),
            _ => None,
        }
    }

    fn notice(app: &App) -> Option<&str> {
        app.notification().map(|(text, _)| text.as_str())
    }

    fn item(content: &str, state: TodoState, task: Option<&str>) -> TodoView {
        TodoView {
            content: content.to_string(),
            state,
            assignee: None,
            task: task.map(str::to_string),
        }
    }

    /// Request `id` of agent `agent`, with `items`.
    fn request(id: &str, agent: &str, items: Vec<TodoView>) -> TaskView {
        TaskView {
            id: id.to_string(),
            title: format!("asked {id}"),
            state: R,
            agent_id: agent.to_string(),
            agent_name: agent.to_uppercase(),
            items,
            ..TaskView::default()
        }
    }

    /// Task `id` handed out by `parent`, with an item of its own the tab
    /// never lists.
    fn part(id: &str, parent: &str, state: TaskState) -> TaskView {
        TaskView {
            id: id.to_string(),
            title: format!("do {id}"),
            state,
            agent_id: format!("agent-{id}"),
            agent_name: format!("Agent {id}"),
            assignee: Some("coder".to_string()),
            parent: Some(parent.to_string()),
            items: vec![item(&format!("inside {id}"), Todo, None)],
            ..TaskView::default()
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
                .map(|(i, &state)| item(&format!("{name} {i}"), state, None))
                .collect(),
        }
    }

    /// The rows as text, indented two spaces per level: `-`/`+` before
    /// what is open/folded, `#` before a todo, `@` before a task; items
    /// are their content.
    fn shape(list: &TaskList) -> Vec<String> {
        list.rows()
            .iter()
            .map(|&row| {
                let fold = match row.fold() {
                    Some(true) => "-",
                    Some(false) => "+",
                    None => "",
                };
                let text = match row {
                    TaskRow::Plan { plan, .. } => plan
                        .map_or("No Plan", |plan| list.plans()[plan].name.as_str())
                        .to_string(),
                    TaskRow::Todo { plan, todo, .. } => {
                        format!("#{}", list.plans()[plan].todos[todo].content)
                    }
                    TaskRow::Item { task, item, .. } => {
                        list.tasks()[task].items[item].content.clone()
                    }
                    TaskRow::Task { task, .. } => format!("@{}", list.tasks()[task].id),
                };
                let text = match row {
                    TaskRow::Todo { .. } => format!("#{fold}{}", &text[1..]),
                    _ => format!("{fold}{text}"),
                };
                format!("{}{text}", "  ".repeat(row.depth()))
            })
            .collect()
    }

    fn list_of(plans: Vec<PlanView>, tasks: Vec<TaskView>) -> TaskList {
        let mut list = TaskList::default();
        list.set_plans(plans);
        list.set_tasks(tasks);
        list
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

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    fn selected_shape(app: &App) -> String {
        shape(app.task_list())[app.task_list().selected()].clone()
    }

    #[test]
    fn requests_have_no_row_their_items_and_parts_do_at_the_left_edge() {
        let list = list_of(
            Vec::new(),
            vec![
                part("p", "r", R),
                request(
                    "r",
                    "lead",
                    vec![item("x", Done, None), item("y", Todo, None)],
                ),
            ],
        );
        assert_eq!(shape(&list), ["-No Plan", "x", "y", "@p"]);
        assert!(
            list.rows().iter().all(|row| row.depth() == 0),
            "icons sit under the header's arrow"
        );
    }

    #[test]
    fn a_request_with_nothing_to_list_adds_no_rows() {
        let list = list_of(Vec::new(), vec![request("r", "lead", Vec::new())]);
        assert!(list.rows().is_empty(), "{:?}", shape(&list));
    }

    #[test]
    fn an_agents_list_is_merged_across_its_requests_newest_state_wins() {
        let list = list_of(
            Vec::new(),
            vec![
                request(
                    "commit",
                    "lead",
                    vec![
                        item("a", Done, None),
                        item("b", Done, None),
                        item("c", Todo, None),
                    ],
                ),
                request(
                    "impl",
                    "lead",
                    vec![item("a", Done, None), item("b", Doing, None)],
                ),
            ],
        );
        assert_eq!(shape(&list), ["-No Plan", "a", "b", "c"]);
        let TaskRow::Item { task, item, .. } = list.rows()[2] else {
            panic!("an item row");
        };
        assert_eq!(list.item(task, item).map(|i| i.state), Some(Done));
    }

    #[test]
    fn the_agent_last_at_work_comes_first() {
        let list = list_of(
            Vec::new(),
            vec![
                request("new", "b", vec![item("b's", Todo, None)]),
                request("old", "a", vec![item("a's", Todo, None)]),
            ],
        );
        assert_eq!(shape(&list), ["-No Plan", "b's", "a's"]);
    }

    #[test]
    fn a_task_sits_one_level_under_the_item_it_does_even_after_the_list_moves_on() {
        let tasks = vec![
            request(
                "later",
                "lead",
                vec![item("x", Todo, None), item("y", Todo, None)],
            ),
            part("p2", "first", R),
            part("p1", "first", R),
            request(
                "first",
                "lead",
                vec![item("x", Todo, Some("p1")), item("y", Todo, None)],
            ),
        ];
        let list = list_of(Vec::new(), tasks);
        assert_eq!(
            shape(&list),
            ["-No Plan", "-x", "  @p1", "y", "@p2"],
            "p1 does x, which the later list carries without the link"
        );
    }

    #[test]
    fn a_tasks_own_parts_nest_under_it_and_its_items_are_not_listed() {
        let list = list_of(
            Vec::new(),
            vec![
                part("q", "p", R),
                part("p", "r", R),
                request("r", "lead", Vec::new()),
            ],
        );
        assert_eq!(shape(&list), ["-No Plan", "-@p", "  @q"]);
    }

    #[test]
    fn done_items_fold_their_tasks_and_folds_stick() {
        let tasks = || {
            vec![
                part("p1", "r", C),
                request("r", "lead", vec![item("x", Done, Some("p1"))]),
            ]
        };
        let mut app = app_on_tasks(Vec::new(), tasks());
        assert_eq!(shape(app.task_list()), ["-No Plan", "+x"]);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        assert_eq!(shape(app.task_list()), ["-No Plan", "-x", "  @p1"]);
        app.process_command(UiCommand::SetTasks(tasks()));
        assert_eq!(
            shape(app.task_list()),
            ["-No Plan", "-x", "  @p1"],
            "the fold survives updates"
        );
        assert_eq!(selected_shape(&app), "-x", "and so does the highlight");

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!(
            shape(app.task_list()),
            ["-No Plan", "+x"],
            "Left on a task with nothing under it folds the item it is under"
        );
        assert_eq!(selected_shape(&app), "+x");
    }

    #[test]
    fn a_new_request_keeps_the_highlight_on_its_item() {
        let mut app = app_on_tasks(
            Vec::new(),
            vec![request(
                "one",
                "lead",
                vec![item("x", Todo, None), item("y", Todo, None)],
            )],
        );
        press(&mut app, KeyCode::End);
        assert_eq!(selected_shape(&app), "y");
        app.process_command(UiCommand::UpsertTask(request(
            "two",
            "other",
            vec![item("z", Todo, None)],
        )));
        assert_eq!(shape(app.task_list()), ["-No Plan", "z", "x", "y"]);
        assert_eq!(selected_shape(&app), "y");
    }

    /// Plan `auth` with todos `auth 0`, which `p1` does, and `auth 1`;
    /// `build` builds it with items of its own and hands out `p1` and
    /// then `p2`.
    fn building() -> (PlanView, Vec<TaskView>) {
        let mut auth = plan("auth", &[Doing, Todo]);
        auth.todos[0].task = Some("p1".to_string());
        let build = TaskView {
            plan: Some(path("auth")),
            builds: true,
            ..request("build", "lead", vec![item("own 0", Todo, None)])
        };
        let tasks = vec![part("p2", "build", R), part("p1", "build", R), build];
        (auth, tasks)
    }

    #[test]
    fn the_build_adds_no_items_and_its_tasks_go_under_the_todos_they_do() {
        let (auth, tasks) = building();
        let list = list_of(vec![auth], tasks);
        assert_eq!(
            shape(&list),
            ["-auth", "#-auth 0", "  @p1", "#auth 1", "@p2"]
        );
        assert!(list.is_hidden_build("build"));
        assert!(!list.is_hidden_build("p1"), "only the request is hidden");
    }

    #[test]
    fn a_build_whose_plan_is_not_listed_lists_its_items_under_no_plan() {
        let (_, tasks) = building();
        let list = list_of(vec![plan("other", &[Todo])], tasks);
        assert_eq!(
            shape(&list),
            ["-other", "#other 0", "-No Plan", "own 0", "@p1", "@p2"]
        );
        assert!(!list.is_hidden_build("build"));
    }

    #[test]
    fn plans_with_open_work_start_open_and_done_ones_folded() {
        let list = list_of(
            vec![
                plan("todos", &[Todo]),
                plan("task", &[Done]),
                plan("done", &[Done]),
            ],
            vec![TaskView {
                plan: Some(path("task")),
                ..request("r", "lead", Vec::new())
            }],
        );
        assert_eq!(
            shape(&list),
            ["-todos", "#todos 0", "-task", "#task 0", "+done"]
        );
    }

    #[test]
    fn the_current_plan_comes_first_and_starts_open_even_when_done() {
        let mut list = list_of(vec![plan("a", &[Todo]), plan("b", &[Done])], Vec::new());
        list.set_current_plan(Some(&plan("b", &[Done])));
        assert_eq!(shape(&list), ["-b", "#b 0", "-a", "#a 0"]);
        list.set_current_plan(Some(&plan("c", &[Todo])));
        assert_eq!(list.plans()[0].path, path("c"), "a new plan goes on top");
    }

    #[test]
    fn selecting_a_folded_task_opens_its_group_and_item() {
        let mut list = list_of(
            Vec::new(),
            vec![
                part("p1", "r", C),
                request("r", "lead", vec![item("x", Done, Some("p1"))]),
            ],
        );
        list.select(0);
        list.toggle_selected();
        assert_eq!(shape(&list), ["+No Plan"]);
        assert!(list.select_task("p1"));
        assert_eq!(shape(&list), ["-No Plan", "-x", "  @p1"]);
        assert_eq!(list.selected(), 2);
        assert!(!list.select_task("missing"));
        assert!(
            !list.select_task("r"),
            "a request has no row; its group's header is highlighted"
        );
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn t_selects_the_task_doing_a_todo_or_item_or_says_why_not() {
        let (mut auth, mut tasks) = building();
        auth.todos[1].task = Some("build".to_string());
        tasks.push(part("p3", "r", R));
        tasks.push(request(
            "r",
            "other",
            vec![item("x", Todo, Some("p3")), item("y", Todo, None)],
        ));
        let mut app = app_on_tasks(vec![auth], tasks);
        assert_eq!(
            shape(app.task_list()),
            ["-auth", "#-auth 0", "  @p1", "#auth 1", "@p2", "-No Plan", "-x", "  @p3", "y"]
        );

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("Select a todo to jump to its task"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(selected_shape(&app), "  @p1", "the todo opens to show it");

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("This todo is part of the plan's build"));
        assert_eq!(selected_shape(&app), "#auth 1", "the highlight stays put");

        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(notice(&app), Some("No task is doing this todo"));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(selected_shape(&app), "  @p3", "an item's task");
    }

    #[test]
    fn enter_and_clicks_show_the_agent_and_fold_on_the_icon() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetTasks(vec![
            part("p", "r", R),
            request("r", "lead", vec![item("x", Todo, Some("p"))]),
        ]));
        app.show_side_tab(SidePanelTab::Tasks);
        app.set_task_rows_area(Rect::new(10, 5, 30, 8), 0);
        assert_eq!(shape(app.task_list()), ["-No Plan", "-x", "  @p"]);

        // Column 1 of the rows holds every icon at the left edge.
        app.click_task_row(11, 6);
        assert_eq!(shape(app.task_list()), ["-No Plan", "+x"]);
        app.click_task_row(20, 6);
        assert_eq!(
            switched(&mut rx).as_deref(),
            Some("lead"),
            "an item shows the agent of its request"
        );
        app.click_task_row(20, 5);
        assert_eq!(
            shape(app.task_list()),
            ["+No Plan"],
            "No Plan folds anywhere"
        );
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
    fn p_opens_a_tasks_plan_at_its_todo_and_says_when_there_is_none() {
        let (mut app, _dir) = app_with_plan_file(
            "---\nname: a\ntodos:\n- id: one\n  content: first\n- id: two\n  content: second\n---\n",
        );
        let mut handed = part("pa", "r", R);
        handed.plan = Some(path("a"));
        handed.plan_todo = Some("two".to_string());
        app.process_command(UiCommand::SetTasks(vec![
            handed,
            request("r", "lead", vec![item("x", Todo, None)]),
        ]));
        assert_eq!(shape(app.task_list()), ["-No Plan", "x", "@pa"]);

        press(&mut app, KeyCode::Char('p'));
        assert_eq!(notice(&app), Some("No plan to open"));
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(open_file_line(&app), 5, "the cursor lands on the todo");
    }

    #[test]
    fn enter_on_a_plan_header_opens_its_file_and_on_a_todo_at_the_todo() {
        let (mut app, _dir) =
            app_with_plan_file("---\nname: a\ntodos:\n- content: a 0\n- content: a 1\n---\n");
        app.process_command(UiCommand::SetPlans(vec![plan("a", &[Todo, Todo])]));
        assert_eq!(shape(app.task_list()), ["-a", "#a 0", "#a 1"]);

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            open_file_line(&app),
            0,
            "the header opens the file at the top"
        );
        app.close_detail();
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Enter);
        assert_eq!(open_file_line(&app), 4, "a todo opens at its line");
        app.close_detail();
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(open_file_line(&app), 4, "p on a todo opens the plan too");
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
