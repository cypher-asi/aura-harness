//! Which tasks and plans are listed, the group each task's tree goes
//! in, and the rows of every group.

use std::collections::{HashMap, HashSet};

use super::{RowKey, TaskList, TaskRow};
use crate::events::{PlanView, TaskView};

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

/// The listed plans by what places a request in their group: their path,
/// and the tasks their todos name. Where two plans match, the first
/// listed wins.
#[derive(Debug, Default)]
pub(super) struct PlanIndex<'a> {
    by_path: HashMap<&'a str, usize>,
    by_todo_task: HashMap<&'a str, usize>,
}

impl PlanIndex<'_> {
    /// The listed plan whose group the tree rooted at `root` goes in:
    /// the plan it names, else the plan whose todo it does; `None` for
    /// the No Plan group. A named plan that is not listed puts the tree
    /// under No Plan even when a listed plan's todo names it.
    pub(super) fn group_of(&self, root: &TaskView) -> Option<usize> {
        match root.plan.as_deref() {
            Some(path) => self.by_path.get(path).copied(),
            None => self.by_todo_task.get(root.id.as_str()).copied(),
        }
    }
}

/// The listed tasks by where their rows go: under the listed task that
/// handed them out, or at the top of a plan's group or of No Plan.
struct Placed<'a> {
    /// The first task of each id
    index: HashMap<&'a str, usize>,
    /// The listed tasks each listed task handed out, in the order given
    children: HashMap<&'a str, Vec<usize>>,
    /// The tops of each plan's group, newest first
    groups: Vec<Vec<usize>>,
    /// The tops of the No Plan group, newest first
    no_plan: Vec<usize>,
}

impl TaskList {
    /// Whether `root`, a request in plan `group`'s group, is the request
    /// building that plan, whose list the plan's todos are.
    pub(super) fn hides(&self, root: &TaskView, group: Option<usize>) -> bool {
        root.builds
            && group
                .and_then(|plan| self.plans.get(plan))
                .is_some_and(|plan| root.plan.as_deref() == Some(plan.path.as_str()))
    }

    /// The requests among `tops` (newest first) that built a todo list
    /// plan `group` no longer has. An agent's newest build is never one:
    /// the plan mirrors the list it builds.
    pub(super) fn earlier_builds(&self, group: Option<usize>, tops: &[usize]) -> HashSet<usize> {
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
    pub(super) fn listed_root(&self, id: &str) -> Option<usize> {
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
    pub(super) fn plan_index(&self) -> PlanIndex<'_> {
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

    pub(super) fn rebuild(&mut self, keep: Option<RowKey>) {
        self.scoped = self.scope();
        self.plans_shown = self.shown_plans();
        let rows = self.build_rows();
        self.rows = rows;
        let kept = keep.and_then(|key| self.rows.iter().position(|&row| self.row_is(row, &key)));
        self.select(kept.unwrap_or(self.selected));
    }

    pub(super) fn build_rows(&self) -> Vec<TaskRow> {
        let placed = self.place_tasks();
        let lookups = (&placed.index, &placed.children);
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for plan in self.plan_order() {
            let body = self.group_body(Some(plan), &placed.groups[plan], lookups, &mut seen);
            let open = self.is_plan_open(&self.plans[plan], body.has_open_task);
            rows.push(TaskRow::Plan {
                plan: Some(plan),
                open,
            });
            if open {
                rows.extend(body.rows);
            }
        }
        let body = self.group_body(None, &placed.no_plan, lookups, &mut seen);
        if !body.rows.is_empty() {
            let open = self.reveal || self.no_plan_open.unwrap_or(true);
            rows.push(TaskRow::Plan { plan: None, open });
            if open {
                rows.extend(body.rows);
            }
        }
        rows
    }

    /// The listed tasks by where their rows go.
    fn place_tasks(&self) -> Placed<'_> {
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
        Placed {
            index,
            children,
            groups,
            no_plan,
        }
    }

    /// The listed plans in drawing order, the one being worked on first.
    fn plan_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.plans.len())
            .filter(|&plan| self.plans_shown[plan])
            .collect();
        order.sort_by_key(|&plan| !self.is_current(&self.plans[plan]));
        order
    }
}
