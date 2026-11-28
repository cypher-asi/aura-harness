//! The days the groups are listed under: a plan's group goes under the
//! day its work last moved on, whole, and the work of no plan is split
//! by day, an agent's requests whose lists share an item kept together.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use super::TaskList;
use crate::app::days::day_of;

/// A group listed under a day: a plan's (`group`), or the day's share
/// of the No Plan group (`None`), with the tops of its trees, newest
/// first.
#[derive(Debug)]
pub(super) struct Section {
    pub(super) day: Option<NaiveDate>,
    pub(super) group: Option<usize>,
    pub(super) tops: Vec<usize>,
    /// When its work last moved on, in ms since the epoch
    newest: Option<u64>,
}

/// The first of the requests whose lists the list of request `at` is
/// merged with. Each of `links` names an earlier request merged with
/// it, or itself when first; the names are shortened on the way.
fn first_linked(links: &mut [usize], mut at: usize) -> usize {
    while links[at] != at {
        links[at] = links[links[at]];
        at = links[at];
    }
    at
}

impl TaskList {
    /// The groups in drawing order: by day, newest first; within a day,
    /// the plan being worked on, then the other plans by when they last
    /// moved on, and the day's work of no plan last. `order` lists the
    /// plans to show, `groups` holds each plan's tops and `no_plan` those
    /// of the No Plan group.
    pub(super) fn sections(
        &self,
        order: &[usize],
        (groups, no_plan): (&[Vec<usize>], &[usize]),
        children: &HashMap<&str, Vec<usize>>,
    ) -> Vec<Section> {
        let mut sections: Vec<Section> = order
            .iter()
            .filter_map(|&plan| {
                let tops = groups.get(plan)?.clone();
                let file = self.plans.get(plan)?.changed_ms;
                let newest = self.newest_of(&tops, children).max(file);
                Some(Section {
                    day: day_of(newest),
                    group: Some(plan),
                    tops,
                    newest,
                })
            })
            .collect();
        sections.extend(self.no_plan_sections(no_plan, children));
        sections.sort_by_key(|section| {
            let current = section
                .group
                .and_then(|plan| self.plans.get(plan))
                .is_some_and(|plan| self.is_current(plan));
            (
                Reverse(section.day),
                !current,
                section.group.is_none(),
                Reverse(section.newest),
            )
        });
        sections
    }

    /// When the newest task in the trees rooted at `tops` last moved on;
    /// `None` when none of them says.
    fn newest_of(&self, tops: &[usize], children: &HashMap<&str, Vec<usize>>) -> Option<u64> {
        self.walk(tops, 0, children, &mut HashSet::new())
            .into_iter()
            .filter_map(|(task, _)| self.tasks.get(task))
            .map(|task| task.updated_ms)
            .filter(|&ms| ms > 0)
            .max()
    }

    /// The No Plan group's `tops` (newest first) by day. An agent's items
    /// are merged across its requests by content and listed once, so the
    /// requests whose lists share an item go together, under the newest
    /// day among them.
    fn no_plan_sections(
        &self,
        tops: &[usize],
        children: &HashMap<&str, Vec<usize>>,
    ) -> Vec<Section> {
        let firsts = self.shared_lists(tops);
        let mut newest: Vec<Option<u64>> = vec![None; tops.len()];
        for (&top, &first) in tops.iter().zip(&firsts) {
            newest[first] = newest[first].max(self.newest_of(&[top], children));
        }
        let mut sections: Vec<Section> = Vec::new();
        for (&top, &first) in tops.iter().zip(&firsts) {
            let day = day_of(newest[first]);
            match sections.iter_mut().find(|section| section.day == day) {
                Some(section) => {
                    section.tops.push(top);
                    section.newest = section.newest.max(newest[first]);
                }
                None => sections.push(Section {
                    day,
                    group: None,
                    tops: vec![top],
                    newest: newest[first],
                }),
            }
        }
        sections
    }

    /// For each of `tops`, the place among them of the first request its
    /// agent's list is merged with: one sharing an item with it, or with
    /// one sharing an item with it, and so on. A task with a parent is
    /// merged with none.
    fn shared_lists(&self, tops: &[usize]) -> Vec<usize> {
        let mut links: Vec<usize> = (0..tops.len()).collect();
        let mut holders: HashMap<(&str, &str), usize> = HashMap::new();
        for (k, &top) in tops.iter().enumerate() {
            let Some(view) = self.tasks.get(top).filter(|view| view.parent.is_none()) else {
                continue;
            };
            for item in &view.items {
                let key = (view.agent_id.as_str(), item.content.as_str());
                let Some(&holder) = holders.get(&key) else {
                    holders.insert(key, k);
                    continue;
                };
                let (a, b) = (
                    first_linked(&mut links, k),
                    first_linked(&mut links, holder),
                );
                links[a.max(b)] = a.min(b);
            }
        }
        (0..tops.len())
            .map(|k| first_linked(&mut links, k))
            .collect()
    }
}
