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
//! A plan's todo list can be rewritten between builds. A build whose list
//! the plan no longer has (fewer than half its items are the plan's
//! todos, and it is not its agent's newest build) goes in an earlier
//! round: a foldable header named by what its first request asked, with
//! that list's items under it. Builds in a row whose lists share an item
//! make one round. Earlier rounds come after the group's items, newest
//! first.
//!
//! A task an agent handed out with `task` sits under the todo or item it
//! does, one level in, titled by it; one doing none sits beside the items
//! of the request it is part of. Every task doing the same todo of the
//! plan sits under it, oldest first; a failed one a newer task made up
//! for is stale and drawn muted. The tasks it handed out in turn sit one
//! level under it. Nothing else is indented, so a row's icon sits right
//! under its header's fold arrow, rounds and their items included.
//!
//! ```text
//! ▾  Auth rework              1/3     a plan
//! ☒  Add session table                its todos
//! ▣  Write the login handler
//!   ⢎⡱ Write the login handler  → coder    the task doing it
//! ☐  Wire the middleware
//! ▾  Build the auth           2/2     an earlier round
//! ☒  Add the login form               its items
//!   ✗  Add the login form  → coder    a failed try, stale
//!   ✓  Add the login form  → coder
//! ☒  Add the logout button
//! ▾  No Plan
//! ☒  Find the uses  → explorer        a request's items
//!   ✓  Find the uses  → explorer
//! ☐  Rename the crate
//! ✓  Fix the typo  → coder            a task doing no item
//! ```
//!
//! Plans being worked on, with todos left or with work still open, start
//! open, as do rounds, todos and items not done yet and tasks still being
//! worked on; the rest start folded. Only rows with something under them
//! fold. Folds the user makes stick across updates.
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
    /// An earlier round of a plan's build: the build requests whose todo
    /// list the plan no longer has. `first` is the oldest of them, and
    /// `progress` its merged items' `(closed, total)`.
    Round {
        first: usize,
        open: bool,
        progress: (usize, usize),
    },
    /// An item of an agent's requests' todo lists: `item` of `task`, the
    /// newest request holding it. `link` is the task doing it, when
    /// loaded; `in_round` when it is listed under a [`Self::Round`].
    Item {
        task: usize,
        item: usize,
        link: Option<usize>,
        fold: Option<bool>,
        in_round: bool,
    },
    /// A task an agent handed out, `depth` levels in: 0 beside the items,
    /// one more under the todo, item or task it belongs to. `doing` is
    /// the todo or item it sits under; `stale` when a newer task does
    /// that one in its place.
    Task {
        task: usize,
        depth: usize,
        fold: Option<bool>,
        doing: Option<Doing>,
        stale: bool,
    },
}

/// The todo or item a task sits under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Todo { plan: usize, todo: usize },
    Item { task: usize, item: usize },
}

impl TaskRow {
    /// Index of the task the row belongs to: the task itself, the
    /// request an item was last listed by, or a round's first request;
    /// `None` on plans and todos.
    #[must_use]
    pub const fn task(self) -> Option<usize> {
        match self {
            Self::Task { task, .. } | Self::Item { task, .. } => Some(task),
            Self::Round { first, .. } => Some(first),
            Self::Plan { .. } | Self::Todo { .. } => None,
        }
    }

    /// Index of the plan of a plan header or todo; `None` on the No Plan
    /// header, rounds, items and tasks.
    #[must_use]
    pub const fn plan(self) -> Option<usize> {
        match self {
            Self::Plan { plan, .. } => plan,
            Self::Todo { plan, .. } => Some(plan),
            Self::Round { .. } | Self::Task { .. } | Self::Item { .. } => None,
        }
    }

    /// Levels the row is indented: only tasks under a todo, an item or
    /// another task are.
    #[must_use]
    pub const fn depth(self) -> usize {
        match self {
            Self::Task { depth, .. } => depth,
            Self::Plan { .. } | Self::Round { .. } | Self::Todo { .. } | Self::Item { .. } => 0,
        }
    }

    /// Whether what is under the row shows; `None` when nothing is.
    #[must_use]
    pub const fn fold(self) -> Option<bool> {
        match self {
            Self::Plan { open, .. } | Self::Round { open, .. } => Some(open),
            Self::Todo { fold, .. } | Self::Item { fold, .. } | Self::Task { fold, .. } => fold,
        }
    }
}

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

/// Whether `build`, a request building `plan`, built a todo list the plan
/// no longer has: fewer than half its items are the plan's todos. One
/// with no items built none.
fn built_earlier_list(build: &TaskView, plan: Option<&PlanView>) -> bool {
    let Some(plan) = plan else {
        return false;
    };
    let current = build
        .items
        .iter()
        .filter(|item| plan.todos.iter().any(|todo| todo.content == item.content))
        .count();
    !build.items.is_empty() && current * 2 < build.items.len()
}

/// The todo or item tasks sit under, and the newest of them doing it.
#[derive(Debug, Clone, Copy)]
struct Under {
    doing: Doing,
    link: Option<usize>,
}

/// What a row shows, by path, id and content rather than index, so the
/// highlight can find it again after the lists change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowKey {
    Plan(String),
    NoPlan,
    Todo(String, usize),
    /// An item, by agent id and content
    Item(String, String),
    /// A round, by its first request's id
    Round(String),
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

/// Merged items, with the tasks nested under each.
struct Items {
    /// `(task, item, link)` of each item, in the order first listed
    items: Vec<(usize, usize, Option<usize>)>,
    /// The tasks under each item, by its place in `items`, oldest first
    nested: Vec<Vec<usize>>,
}

/// An earlier round of a plan's build: its first request and its items.
struct RoundItems {
    first: usize,
    items: Items,
}

/// One agent's merged items in a group, its earlier rounds, and the
/// tasks under none.
struct AgentItems {
    items: Items,
    /// Newest first
    rounds: Vec<RoundItems>,
    /// Tasks doing none of the items or todos, oldest first
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
    /// Rounds the user folded (`false`) or opened (`true`), by their
    /// first request's id
    round_folds: HashMap<String, bool>,
    /// Whether rows are built with everything open, to find where a row
    /// sits ([`Self::select_task`])
    reveal: bool,
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

    /// The first line of what a task row shows as its title: the todo or
    /// item it sits under, else what its agent was asked. Empty on other
    /// rows.
    #[must_use]
    pub fn title_of(&self, row: TaskRow) -> &str {
        let TaskRow::Task { task, doing, .. } = row else {
            return "";
        };
        let label = match doing {
            Some(Doing::Todo { plan, todo }) => self
                .plans
                .get(plan)
                .and_then(|plan| plan.todos.get(todo))
                .map(|todo| todo.content.as_str()),
            Some(Doing::Item { task, item }) => {
                self.item(task, item).map(|item| item.content.as_str())
            }
            None => None,
        };
        label
            .or_else(|| self.tasks.get(task).map(|task| task.title.as_str()))
            .and_then(|title| title.lines().next())
            .unwrap_or("")
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

    fn is_open(&self, task: &TaskView) -> bool {
        self.reveal
            || self
                .folds
                .get(&task.id)
                .copied()
                .unwrap_or(!task.state.is_closed())
    }

    /// Whether plan `plan` is open: as the user left it, else when it is
    /// being worked on, has todos left or `has_open_task` in its group.
    fn is_plan_open(&self, plan: &PlanView, has_open_task: bool) -> bool {
        self.reveal
            || self
                .plan_folds
                .get(&plan.path)
                .copied()
                .unwrap_or_else(|| self.is_current(plan) || plan.has_open_todos() || has_open_task)
    }

    /// Whether todo `todo` of `plan` shows the tasks nested under it: as
    /// the user left it, else while the todo is not closed.
    fn is_todo_open(&self, plan: &PlanView, todo: usize) -> bool {
        self.reveal
            || self
                .todo_folds
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
        self.reveal
            || self.item_folds.get(&key).copied().unwrap_or_else(|| {
                self.item(task, item)
                    .is_some_and(|item| !item.state.is_closed())
            })
    }

    /// Whether the round whose first request is `first` shows its items:
    /// as the user left it, else while `has_open_work`.
    fn is_round_open(&self, first: usize, has_open_work: bool) -> bool {
        let id = self.tasks.get(first).map_or("", |task| task.id.as_str());
        self.reveal || self.round_folds.get(id).copied().unwrap_or(has_open_work)
    }

    /// Whether `root`, a request in plan `group`'s group, is the request
    /// building that plan, whose list the plan's todos are.
    fn hides(&self, root: &TaskView, group: Option<usize>) -> bool {
        root.builds
            && group
                .and_then(|plan| self.plans.get(plan))
                .is_some_and(|plan| root.plan.as_deref() == Some(plan.path.as_str()))
    }

    /// The requests among `tops` (newest first) that built a todo list
    /// plan `group` no longer has. An agent's newest build is never one:
    /// the plan mirrors the list it builds.
    fn earlier_builds(&self, group: Option<usize>, tops: &[usize]) -> HashSet<usize> {
        let plan = group.and_then(|plan| self.plans.get(plan));
        let mut newest: HashSet<&str> = HashSet::new();
        let mut earlier = HashSet::new();
        for &top in tops {
            let Some(view) = self.tasks.get(top) else {
                continue;
            };
            if view.parent.is_some() || !self.hides(view, group) {
                continue;
            }
            if newest.insert(view.agent_id.as_str()) {
                continue;
            }
            if built_earlier_list(view, plan) {
                earlier.insert(top);
            }
        }
        earlier
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
            (TaskRow::Task { task, .. }, RowKey::Task(id))
            | (TaskRow::Round { first: task, .. }, RowKey::Round(id)) => {
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
    /// out what is under a folded task. The tops of the tree sit `under`
    /// a todo or item, when given.
    fn task_rows(
        &self,
        tree: &[(usize, usize)],
        children: &HashMap<&str, Vec<usize>>,
        under: Option<Under>,
        rows: &mut Vec<TaskRow>,
    ) {
        let top = tree.first().map(|&(_, depth)| depth);
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
            let under = under.filter(|_| Some(depth) == top);
            rows.push(TaskRow::Task {
                task,
                depth,
                fold,
                doing: under.map(|under| under.doing),
                stale: under.is_some_and(|under| under.link.is_some_and(|link| link != task)),
            });
            if fold == Some(false) {
                folded_at = Some(depth);
            }
        }
    }

    /// The items of `requests`, oldest first, merged by content as
    /// `(task, item, link)`: an item keeps the place it first had and
    /// shows how its newest copy stands.
    fn merge_items(&self, requests: &[usize]) -> Vec<(usize, usize, Option<&str>)> {
        let mut items: Vec<(usize, usize, Option<&str>)> = Vec::new();
        let mut at: HashMap<&str, usize> = HashMap::new();
        for &request in requests {
            let Some(view) = self.tasks.get(request) else {
                continue;
            };
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
        items
    }

    /// `items` with the tasks of `parts` doing them nested under them,
    /// taken out of `parts`: the task an item links to and, `by_todo`,
    /// every other task doing the same todo of the plan.
    fn nest<'a>(
        &'a self,
        items: Vec<(usize, usize, Option<&'a str>)>,
        parts: &mut Vec<usize>,
        index: &HashMap<&str, usize>,
        by_todo: bool,
    ) -> Items {
        let mut by_link: HashMap<&str, usize> = HashMap::new();
        let mut by_plan_todo: HashMap<&str, usize> = HashMap::new();
        for (k, &(_, _, link)) in items.iter().enumerate() {
            let Some(link) = link else {
                continue;
            };
            by_link.insert(link, k);
            let todo = index
                .get(link)
                .and_then(|&task| self.tasks.get(task))
                .and_then(|task| task.plan_todo.as_deref());
            if let Some(todo) = todo.filter(|_| by_todo) {
                by_plan_todo.entry(todo).or_insert(k);
            }
        }
        let mut nested = vec![Vec::new(); items.len()];
        parts.retain(|&part| {
            let Some(view) = self.tasks.get(part) else {
                return false;
            };
            let k = by_link.get(view.id.as_str()).or_else(|| {
                view.plan_todo
                    .as_deref()
                    .and_then(|todo| by_plan_todo.get(todo))
            });
            match k {
                Some(&k) => {
                    nested[k].push(part);
                    false
                }
                None => true,
            }
        });
        for under in &mut nested {
            under.sort_unstable_by(|a, b| b.cmp(a));
        }
        Items {
            items: items
                .into_iter()
                .map(|(task, item, link)| (task, item, link.and_then(|id| index.get(id).copied())))
                .collect(),
            nested,
        }
    }

    /// One agent's items in a group: the items of its `requests` (newest
    /// first), merged, with the tasks they handed out placed under the
    /// item doing them, or loose when no item or one of `todo_links` does.
    /// Builds of a todo list the plan no longer has go in earlier rounds:
    /// one per run of builds whose lists share an item.
    fn agent_items(
        &self,
        requests: &[usize],
        group: Option<usize>,
        (todo_links, earlier): (&HashMap<&str, usize>, &HashSet<usize>),
        index: &HashMap<&str, usize>,
        children: &HashMap<&str, Vec<usize>>,
    ) -> AgentItems {
        let mut own: Vec<usize> = Vec::new();
        let mut parts: Vec<usize> = Vec::new();
        let mut rounds: Vec<(Vec<usize>, HashSet<&str>, Vec<usize>)> = Vec::new();
        for &request in requests.iter().rev() {
            let Some(view) = self.tasks.get(request) else {
                continue;
            };
            let handed = children
                .get(view.id.as_str())
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .copied()
                .filter(|&part| {
                    self.tasks
                        .get(part)
                        .is_some_and(|task| !todo_links.contains_key(task.id.as_str()))
                });
            if !self.hides(view, group) {
                own.push(request);
                parts.extend(handed);
                continue;
            }
            if !earlier.contains(&request) {
                parts.extend(handed);
                continue;
            }
            let contents: HashSet<&str> = view
                .items
                .iter()
                .map(|item| item.content.as_str())
                .collect();
            match rounds.last_mut() {
                Some((builds, listed, round_parts)) if !listed.is_disjoint(&contents) => {
                    builds.push(request);
                    listed.extend(contents);
                    round_parts.extend(handed);
                }
                _ => rounds.push((vec![request], contents, handed.collect())),
            }
        }
        let items = self.nest(self.merge_items(&own), &mut parts, index, false);
        let rounds = rounds
            .into_iter()
            .rev()
            .map(|(builds, _, mut round_parts)| {
                let items = self.nest(self.merge_items(&builds), &mut round_parts, index, true);
                parts.extend(round_parts);
                RoundItems {
                    first: builds[0],
                    items,
                }
            })
            .collect();
        parts.sort_unstable_by(|a, b| b.cmp(a));
        AgentItems {
            items,
            rounds,
            loose: parts,
        }
    }

    /// Rows of `items` and the tasks nested under them, onto `rows`.
    /// Returns the trees of those tasks.
    fn item_rows(
        &self,
        items: &Items,
        in_round: bool,
        children: &HashMap<&str, Vec<usize>>,
        seen: &mut HashSet<usize>,
        rows: &mut Vec<TaskRow>,
    ) -> Vec<Vec<(usize, usize)>> {
        let mut trees = Vec::new();
        for (&(task, item, link), nested) in items.items.iter().zip(&items.nested) {
            let tree = self.walk(nested, 1, children, seen);
            let fold = (!tree.is_empty()).then(|| self.is_item_open(task, item));
            rows.push(TaskRow::Item {
                task,
                item,
                link,
                fold,
                in_round,
            });
            if fold == Some(true) {
                let under = Under {
                    doing: Doing::Item { task, item },
                    link,
                };
                self.task_rows(&tree, children, Some(under), rows);
            }
            trees.push(tree);
        }
        trees
    }

    /// The rows of an earlier round: its header, then its items while
    /// open. Returns the trees of the tasks under them.
    fn round_rows(
        &self,
        round: &RoundItems,
        children: &HashMap<&str, Vec<usize>>,
        seen: &mut HashSet<usize>,
        rows: &mut Vec<TaskRow>,
    ) -> Vec<Vec<(usize, usize)>> {
        let mut body = Vec::new();
        let trees = self.item_rows(&round.items, true, children, seen, &mut body);
        let states: Vec<bool> = round
            .items
            .items
            .iter()
            .filter_map(|&(task, item, _)| self.item(task, item))
            .map(|item| item.state.is_closed())
            .collect();
        let closed = states.iter().filter(|&&closed| closed).count();
        let has_open_work = closed < states.len()
            || trees
                .iter()
                .flatten()
                .any(|&(task, _)| self.tasks.get(task).is_some_and(|t| !t.state.is_closed()));
        let open = self.is_round_open(round.first, has_open_work);
        rows.push(TaskRow::Round {
            first: round.first,
            open,
            progress: (closed, states.len()),
        });
        if open {
            rows.extend(body);
        }
        trees
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
        let mut todo_ids: HashMap<&str, usize> = HashMap::new();
        for (i, todo) in todos.iter().enumerate() {
            let Some(id) = todo.task.as_deref() else {
                continue;
            };
            todo_links.entry(id).or_insert(i);
            let todo_id = index
                .get(id)
                .and_then(|&task| self.tasks.get(task))
                .and_then(|task| task.plan_todo.as_deref());
            if let Some(todo_id) = todo_id {
                todo_ids.entry(todo_id).or_insert(i);
            }
        }

        // The tasks handed out in the group: what a request handed out,
        // wherever the request is listed, and such a task whose request
        // is left out. Each says whether it may do a todo by its id: not
        // when its request built a list the plan no longer has, whose
        // ids may since name other todos.
        let earlier = self.earlier_builds(group, tops);
        let mut handed: Vec<(usize, bool)> = Vec::new();
        for &top in tops {
            let Some(view) = self.tasks.get(top) else {
                continue;
            };
            if view.parent.is_some() {
                handed.push((top, true));
                continue;
            }
            let by_id = !earlier.contains(&top);
            if let Some(parts) = children.get(view.id.as_str()) {
                handed.extend(parts.iter().map(|&part| (part, by_id)));
            }
        }
        // A task doing the same todo as the task a todo names sits under
        // that todo too.
        for &(part, by_id) in &handed {
            let Some(view) = self.tasks.get(part) else {
                continue;
            };
            let on_plan = plan.is_some_and(|plan| view.plan.as_deref() == Some(plan.path.as_str()));
            let todo = view
                .plan_todo
                .as_deref()
                .filter(|_| by_id && on_plan)
                .and_then(|todo_id| todo_ids.get(todo_id));
            if let Some(&todo) = todo {
                todo_links.entry(view.id.as_str()).or_insert(todo);
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

        // The tasks under the plan's todos, oldest first.
        let mut under_todos: Vec<Vec<usize>> = vec![Vec::new(); todos.len()];
        for &(part, _) in &handed {
            let todo = self
                .tasks
                .get(part)
                .and_then(|part| todo_links.get(part.id.as_str()))
                .and_then(|&todo| under_todos.get_mut(todo));
            if let Some(under) = todo {
                under.push(part);
            }
        }
        for under in &mut under_todos {
            under.sort_unstable_by(|a, b| b.cmp(a));
            under.dedup();
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
                let link = todos
                    .get(todo)
                    .and_then(|todo| todo.task.as_deref())
                    .and_then(|id| index.get(id).copied());
                let under = group.map(|plan| Under {
                    doing: Doing::Todo { plan, todo },
                    link,
                });
                self.task_rows(&tree, children, under, &mut rows);
            }
            trees.push(tree);
        }
        for agent in agents {
            let Some(requests) = requests.get(agent) else {
                continue;
            };
            let block = self.agent_items(requests, group, (&todo_links, &earlier), index, children);
            trees.extend(self.item_rows(&block.items, false, children, seen, &mut rows));
            for round in &block.rounds {
                trees.extend(self.round_rows(round, children, seen, &mut rows));
            }
            let tree = self.walk(&block.loose, 0, children, seen);
            self.task_rows(&tree, children, None, &mut rows);
            trees.push(tree);
        }
        let tree = self.walk(&orphans, 0, children, seen);
        self.task_rows(&tree, children, None, &mut rows);
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

    /// The highlighted row, by what it shows.
    pub(super) fn place(&self) -> Option<RowKey> {
        self.selected_key()
    }

    /// Highlight the row `key` names again; with none, or one no longer
    /// listed, the highlight stays where it is.
    pub(super) fn put_back(&mut self, key: Option<&RowKey>) {
        if let Some(at) =
            key.and_then(|key| self.rows.iter().position(|&row| self.row_is(row, key)))
        {
            self.select(at);
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
            let open = self.reveal || self.no_plan_open.unwrap_or(true);
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
        self.tasks_scroll = follow_selection(
            self.tasks_scroll,
            self.task_list.selected,
            visible,
            self.task_list.rows.len(),
        );
        self.tasks_scroll
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
    /// the agent of a task, of the request an item is on, or of a round's
    /// first request. The No Plan header has nothing to open, so it folds.
    fn open_selected_task_row(&mut self) {
        match self.task_list.rows.get(self.task_list.selected) {
            Some(TaskRow::Plan { plan: None, .. }) => self.task_list.toggle_selected(),
            Some(TaskRow::Plan { .. } | TaskRow::Todo { .. }) => self.open_selected_plan(),
            Some(TaskRow::Task { .. } | TaskRow::Item { .. } | TaskRow::Round { .. }) => {
                self.open_selected_task_agent();
            }
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
            self.notifications.current = Some((
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
        self.notifications.current = Some((message.into(), NotificationType::Warning));
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
mod tests;
