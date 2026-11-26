//! The rows under a group's header: the plan's todos with the tasks
//! doing them, then each agent's items, rounds and loose tasks.

use std::collections::{HashMap, HashSet};

use super::{Doing, TaskList, TaskRow};
use crate::events::PlanView;

/// The todo or item tasks sit under, and the newest of them doing it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Under {
    pub(super) doing: Doing,
    pub(super) link: Option<usize>,
}

/// The rows of a group under its header, and whether any of its work is
/// still open.
pub(super) struct GroupBody {
    pub(super) rows: Vec<TaskRow>,
    pub(super) has_open_task: bool,
}

/// A group's requests by agent, and its tasks whose request is left out.
struct AgentRequests<'a> {
    /// The agents, the one last at work first
    agents: Vec<&'a str>,
    /// Each agent's requests, newest first
    requests: HashMap<&'a str, Vec<usize>>,
    /// Tasks handed out by a task the scope leaves out, doing no todo
    orphans: Vec<usize>,
    /// Whether any of the group's tops is still open
    has_open_task: bool,
}

impl TaskList {
    /// Every task of the trees rooted at `tops`, in drawing order, with
    /// its depth, the tops at `depth`, whether folded or not. `seen`
    /// guards against parent loops and tasks listed twice.
    pub(super) fn walk(
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
    pub(super) fn task_rows(
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

    /// The rows under the header of plan `group`'s group (`None` for No
    /// Plan), whose requests are `tops`, newest first.
    pub(super) fn group_body(
        &self,
        group: Option<usize>,
        tops: &[usize],
        (index, children): (&HashMap<&str, usize>, &HashMap<&str, Vec<usize>>),
        seen: &mut HashSet<usize>,
    ) -> GroupBody {
        let plan = group.and_then(|plan| self.plans.get(plan));
        let (mut todo_links, todo_ids) = self.todo_links(plan, index);
        let earlier = self.earlier_builds(group, tops);
        let handed = self.handed_out(tops, &earlier, children);
        self.link_same_todo(plan, &handed, &todo_ids, &mut todo_links);
        let agents = self.agent_requests(tops, &todo_links);
        let todo_count = plan.map_or(0, |plan| plan.todos.len());
        let under_todos = self.under_todos(&handed, &todo_links, todo_count);
        let (mut rows, mut trees) = self.todo_rows(group, &under_todos, (index, children), seen);
        let links = (&todo_links, &earlier);
        trees.extend(self.agent_rows(&agents, group, links, (index, children), seen, &mut rows));
        let tree = self.walk(&agents.orphans, 0, children, seen);
        self.task_rows(&tree, children, None, &mut rows);
        trees.push(tree);
        let has_open_task = agents.has_open_task
            || trees
                .iter()
                .flatten()
                .any(|&(task, _)| self.tasks.get(task).is_some_and(|t| !t.state.is_closed()));
        GroupBody {
            rows,
            has_open_task,
        }
    }

    /// The todos of `plan` by the task each names, and by the plan todo
    /// that task does.
    fn todo_links<'a>(
        &'a self,
        plan: Option<&'a PlanView>,
        index: &HashMap<&str, usize>,
    ) -> (HashMap<&'a str, usize>, HashMap<&'a str, usize>) {
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
        (todo_links, todo_ids)
    }

    /// The tasks handed out in the group: what a request handed out,
    /// wherever the request is listed, and such a task whose request
    /// is left out. Each says whether it may do a todo by its id: not
    /// when its request built a list the plan no longer has, whose
    /// ids may since name other todos.
    fn handed_out(
        &self,
        tops: &[usize],
        earlier: &HashSet<usize>,
        children: &HashMap<&str, Vec<usize>>,
    ) -> Vec<(usize, bool)> {
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
        handed
    }

    /// A task doing the same todo as the task a todo names sits under
    /// that todo too.
    fn link_same_todo<'a>(
        &'a self,
        plan: Option<&PlanView>,
        handed: &[(usize, bool)],
        todo_ids: &HashMap<&str, usize>,
        todo_links: &mut HashMap<&'a str, usize>,
    ) {
        for &(part, by_id) in handed {
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
    }

    /// Each agent's requests, newest first, the agent last at work
    /// first. A task handed out by a task the scope leaves out is a
    /// task still, listed after the items.
    fn agent_requests<'a>(
        &'a self,
        tops: &[usize],
        todo_links: &HashMap<&str, usize>,
    ) -> AgentRequests<'a> {
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
        AgentRequests {
            agents,
            requests,
            orphans,
            has_open_task,
        }
    }

    /// The tasks under each of the plan's `todos`, oldest first.
    fn under_todos(
        &self,
        handed: &[(usize, bool)],
        todo_links: &HashMap<&str, usize>,
        todos: usize,
    ) -> Vec<Vec<usize>> {
        let mut under_todos: Vec<Vec<usize>> = vec![Vec::new(); todos];
        for &(part, _) in handed {
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
        under_todos
    }

    /// The rows of plan `group`'s todos, each with the tasks under it
    /// while open, and the trees of those tasks.
    fn todo_rows(
        &self,
        group: Option<usize>,
        under_todos: &[Vec<usize>],
        (index, children): (&HashMap<&str, usize>, &HashMap<&str, Vec<usize>>),
        seen: &mut HashSet<usize>,
    ) -> (Vec<TaskRow>, Vec<Vec<(usize, usize)>>) {
        let plan = group.and_then(|plan| self.plans.get(plan));
        let todos = plan.map_or(&[][..], |plan| &plan.todos);
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
        (rows, trees)
    }

    /// Each agent's items, earlier rounds and loose tasks, onto `rows`.
    /// Returns the trees of the tasks among them.
    fn agent_rows(
        &self,
        agents: &AgentRequests<'_>,
        group: Option<usize>,
        links: (&HashMap<&str, usize>, &HashSet<usize>),
        (index, children): (&HashMap<&str, usize>, &HashMap<&str, Vec<usize>>),
        seen: &mut HashSet<usize>,
        rows: &mut Vec<TaskRow>,
    ) -> Vec<Vec<(usize, usize)>> {
        let mut trees = Vec::new();
        for &agent in &agents.agents {
            let Some(requests) = agents.requests.get(agent) else {
                continue;
            };
            let block = self.agent_items(requests, group, links, index, children);
            trees.extend(self.item_rows(&block.items, false, children, seen, rows));
            for round in &block.rounds {
                trees.extend(self.round_rows(round, children, seen, rows));
            }
            let tree = self.walk(&block.loose, 0, children, seen);
            self.task_rows(&tree, children, None, rows);
            trees.push(tree);
        }
        trees
    }
}
