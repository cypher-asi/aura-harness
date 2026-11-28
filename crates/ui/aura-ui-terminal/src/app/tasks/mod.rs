//! The TASKS side tab: the work agents did or are doing in the active
//! agent's workspace, grouped under the plan it works on, and the groups
//! under the day their work last moved on, newest first, headed as the
//! Archive heads its days. Each plan is a foldable header with its todos;
//! within a day the plan being worked on comes first, then the other
//! plans by when they last moved on. Work on no listed plan goes last in
//! each day, under a `No Plan` header, shown only when it has rows.
//!
//! A plan's group goes under the day its file or a task in it last
//! changed, whole, however many days its work took. So do an agent's
//! requests of no plan whose todo lists share an item, as one list
//! carried over from request to request. Work of no known time goes
//! last, under `EARLIER`. The highlight passes over the days' headers.
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
//! TODAY                               a day
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
//! YESTERDAY
//! ▸  Parser cleanup           ✓ 5/5   a plan done that day
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
//!
//! While a channel is open TASKS is the channel's last tab, after QUEUE,
//! and lists every plan and task in the channel's folder, whatever the
//! side panel's scope.

use std::collections::HashMap;

use super::processes::SIDE_PAGE;
use super::side_scope::AgentFilter;
use super::{follow_selection, App};
use crate::events::{PlanView, TaskView, TodoView};
use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

mod actions;
mod days;
mod group_body;
mod grouping;
mod items;
mod selection;

/// One row of the tab. `fold` is whether what is under the row shows,
/// `None` when nothing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRow {
    /// The header of the local day the groups under it last moved on;
    /// `None` for work of no known time
    Day { day: Option<NaiveDate> },
    /// A group's header, under `day`: a plan, or `None` for that day's
    /// No Plan group
    Plan {
        plan: Option<usize>,
        day: Option<NaiveDate>,
        open: bool,
    },
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
    /// `None` on days, plans and todos.
    #[must_use]
    pub const fn task(self) -> Option<usize> {
        match self {
            Self::Task { task, .. } | Self::Item { task, .. } => Some(task),
            Self::Round { first, .. } => Some(first),
            Self::Day { .. } | Self::Plan { .. } | Self::Todo { .. } => None,
        }
    }

    /// Index of the plan of a plan header or todo; `None` on days, the
    /// No Plan header, rounds, items and tasks.
    #[must_use]
    pub const fn plan(self) -> Option<usize> {
        match self {
            Self::Plan { plan, .. } => plan,
            Self::Todo { plan, .. } => Some(plan),
            Self::Day { .. } | Self::Round { .. } | Self::Task { .. } | Self::Item { .. } => None,
        }
    }

    /// Levels the row is indented: only tasks under a todo, an item or
    /// another task are.
    #[must_use]
    pub const fn depth(self) -> usize {
        match self {
            Self::Task { depth, .. } => depth,
            Self::Day { .. }
            | Self::Plan { .. }
            | Self::Round { .. }
            | Self::Todo { .. }
            | Self::Item { .. } => 0,
        }
    }

    /// Whether what is under the row shows; `None` when nothing is, as
    /// under a day's header, which does not fold.
    #[must_use]
    pub const fn fold(self) -> Option<bool> {
        match self {
            Self::Plan { open, .. } | Self::Round { open, .. } => Some(open),
            Self::Todo { fold, .. } | Self::Item { fold, .. } | Self::Task { fold, .. } => fold,
            Self::Day { .. } => None,
        }
    }
}

/// What a row shows, by path, id and content rather than index, so the
/// highlight can find it again after the lists change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowKey {
    Plan(String),
    /// A day's No Plan group
    NoPlan(Option<NaiveDate>),
    Todo(String, usize),
    /// An item, by agent id and content
    Item(String, String),
    /// A round, by its first request's id
    Round(String),
    Task(String),
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
    /// No Plan groups the user folded (`false`) or opened (`true`), by
    /// day
    no_plan_folds: HashMap<Option<NaiveDate>, bool>,
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

    /// Replace the listed copy of `plan`, or add it on top, without
    /// marking it as the one being worked on: a plan another agent wrote
    /// or moved on.
    pub fn upsert_plan(&mut self, plan: PlanView) {
        let keep = self.selected_key();
        self.put_plan(plan);
        self.rebuild(keep);
    }

    /// Mark `plan` as the one being worked on, replacing its listed copy,
    /// or adding it on top when it isn't listed. `None` leaves the plans
    /// as they are with none marked.
    pub fn set_current_plan(&mut self, plan: Option<&PlanView>) {
        let keep = self.selected_key();
        self.current_plan = plan.map(|plan| plan.path.clone());
        if let Some(plan) = plan {
            self.put_plan(plan.clone());
        }
        self.rebuild(keep);
    }

    /// Replace the listed copy of `plan`, or add it on top.
    fn put_plan(&mut self, plan: PlanView) {
        match self.plans.iter_mut().find(|p| p.path == plan.path) {
            Some(listed) => *listed = plan,
            None => self.plans.insert(0, plan),
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
}

impl App {
    /// The TASKS tab's plans and tasks.
    #[must_use]
    pub const fn task_list(&self) -> &TaskList {
        &self.task_list
    }

    /// Keep the highlighted row inside `visible` rows, returning the first
    /// row to draw. When it is the first row of a day and comes in from
    /// above, the day's header comes in with it.
    pub fn sync_tasks_scroll(&mut self, visible: usize) -> usize {
        let list = &self.task_list;
        let selected = list.selected;
        let scroll = follow_selection(self.side.tasks_scroll, selected, visible, list.rows.len());
        let header_above = selected
            .checked_sub(1)
            .filter(|&above| matches!(list.rows.get(above), Some(TaskRow::Day { .. })));
        self.side.tasks_scroll = match header_above {
            Some(header) if scroll == selected && visible > 1 => header,
            _ => scroll,
        };
        self.side.tasks_scroll
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
}

#[cfg(test)]
mod tests;
