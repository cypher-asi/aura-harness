//! Agents' todo-list items merged across their requests, their earlier
//! rounds, and the rows of both.

use std::collections::{HashMap, HashSet};

use super::group_body::Under;
use super::{Doing, TaskList, TaskRow};

/// Merged items, with the tasks nested under each.
pub(super) struct Items {
    /// `(task, item, link)` of each item, in the order first listed
    items: Vec<(usize, usize, Option<usize>)>,
    /// The tasks under each item, by its place in `items`, oldest first
    nested: Vec<Vec<usize>>,
}

/// An earlier round of a plan's build: its first request and its items.
pub(super) struct RoundItems {
    first: usize,
    items: Items,
}

/// One agent's merged items in a group, its earlier rounds, and the
/// tasks under none.
pub(super) struct AgentItems {
    pub(super) items: Items,
    /// Newest first
    pub(super) rounds: Vec<RoundItems>,
    /// Tasks doing none of the items or todos, oldest first
    pub(super) loose: Vec<usize>,
}

/// Builds in a row whose lists share an item: the builds, their items'
/// contents, and the tasks they handed out.
type PendingRound<'a> = (Vec<usize>, HashSet<&'a str>, Vec<usize>);

/// An agent's requests by where their lists go, oldest first.
struct SortedRequests<'a> {
    /// Requests whose items are listed
    own: Vec<usize>,
    /// Tasks handed out outside the earlier rounds
    parts: Vec<usize>,
    /// The earlier rounds
    rounds: Vec<PendingRound<'a>>,
}

impl TaskList {
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
    pub(super) fn agent_items(
        &self,
        requests: &[usize],
        group: Option<usize>,
        links: (&HashMap<&str, usize>, &HashSet<usize>),
        index: &HashMap<&str, usize>,
        children: &HashMap<&str, Vec<usize>>,
    ) -> AgentItems {
        let SortedRequests {
            own,
            mut parts,
            rounds,
        } = self.sort_requests(requests, group, links, children);
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

    /// `requests` (newest first) by where their lists go: listed as
    /// items, a build whose list the plan has, or an earlier round.
    fn sort_requests<'a>(
        &'a self,
        requests: &[usize],
        group: Option<usize>,
        (todo_links, earlier): (&HashMap<&str, usize>, &HashSet<usize>),
        children: &HashMap<&str, Vec<usize>>,
    ) -> SortedRequests<'a> {
        let mut own: Vec<usize> = Vec::new();
        let mut parts: Vec<usize> = Vec::new();
        let mut rounds: Vec<PendingRound<'a>> = Vec::new();
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
        SortedRequests { own, parts, rounds }
    }

    /// Rows of `items` and the tasks nested under them, onto `rows`.
    /// Returns the trees of those tasks.
    pub(super) fn item_rows(
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
    pub(super) fn round_rows(
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
}
