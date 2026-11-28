//! Folding a channel's subagents on the MEMBERS side tab.
//!
//! The members come depth-first: each member, then the subagents it
//! spawned, deeper. A member with subagents under it starts folded over
//! them, and its id joins [`super::ChannelState`]'s unfolded set once it
//! is opened, by Space or Right on its row or a click on its `▸`.
//!
//! `member_selected` stays an index into the members; the rows on screen
//! are the members shown ([`shown_members`]). The highlight is always on
//! a shown member: folding a member over it parks it there, and
//! highlighting a member from elsewhere (its task, its chat) unfolds the
//! members above it.

use crate::events::AgentSummary;
use std::collections::HashSet;

/// Whether member `index` has subagents listed under it, so it folds.
pub(crate) fn has_subagents(members: &[AgentSummary], index: usize) -> bool {
    match (members.get(index), members.get(index + 1)) {
        (Some(member), Some(next)) => next.depth > member.depth,
        _ => false,
    }
}

/// Index just past member `index` and every subagent under it.
pub(crate) fn subtree_end(members: &[AgentSummary], index: usize) -> usize {
    let Some(depth) = members.get(index).map(|member| member.depth) else {
        return members.len();
    };
    (index + 1..members.len())
        .find(|&i| members[i].depth <= depth)
        .unwrap_or(members.len())
}

/// Whether member `index` is folded over its subagents.
pub(crate) fn is_folded(
    members: &[AgentSummary],
    unfolded: &HashSet<String>,
    index: usize,
) -> bool {
    has_subagents(members, index)
        && members
            .get(index)
            .is_some_and(|member| !unfolded.contains(&member.id))
}

/// The members shown, in order: every member but those under a folded
/// one.
pub(crate) fn shown_members(members: &[AgentSummary], unfolded: &HashSet<String>) -> Vec<usize> {
    let mut shown = Vec::with_capacity(members.len());
    let mut index = 0;
    while index < members.len() {
        shown.push(index);
        index = if is_folded(members, unfolded, index) {
            subtree_end(members, index)
        } else {
            index + 1
        };
    }
    shown
}

/// The member that spawned member `index`: the nearest one above it with
/// a smaller depth.
pub(crate) fn parent_of(members: &[AgentSummary], index: usize) -> Option<usize> {
    let depth = members.get(index)?.depth;
    if depth == 0 {
        return None;
    }
    (0..index).rev().find(|&i| members[i].depth < depth)
}

/// The members member `index` is under, the top one first.
fn ancestors(members: &[AgentSummary], index: usize) -> Vec<usize> {
    let mut chain: Vec<usize> =
        std::iter::successors(parent_of(members, index), |&i| parent_of(members, i)).collect();
    chain.reverse();
    chain
}

/// Member `index` when it is shown, else the folded member it is hidden
/// under: the top one of the folded members above it.
pub(crate) fn shown_member_of(
    members: &[AgentSummary],
    unfolded: &HashSet<String>,
    index: usize,
) -> usize {
    ancestors(members, index)
        .into_iter()
        .find(|&above| is_folded(members, unfolded, above))
        .unwrap_or(index)
}

/// Ids of the members above member `index`: unfolded, it shows.
pub(crate) fn ids_above(members: &[AgentSummary], index: usize) -> Vec<String> {
    ancestors(members, index)
        .into_iter()
        .map(|above| members[above].id.clone())
        .collect()
}

/// Whether a subagent folded under member `index` matches `is_working`.
pub(crate) fn hides_working(
    members: &[AgentSummary],
    unfolded: &HashSet<String>,
    index: usize,
    is_working: impl Fn(&AgentSummary) -> bool,
) -> bool {
    is_folded(members, unfolded, index)
        && members
            .get(index + 1..subtree_end(members, index))
            .is_some_and(|hidden| hidden.iter().any(is_working))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `co`, then `re` with subagents `s1` (which has `s2`) and `s3`, then
    /// `ux`.
    fn members() -> Vec<AgentSummary> {
        [
            ("co", 0),
            ("re", 0),
            ("s1", 1),
            ("s2", 2),
            ("s3", 1),
            ("ux", 0),
        ]
        .into_iter()
        .map(|(id, depth)| AgentSummary {
            id: id.to_string(),
            depth,
            ..AgentSummary::default()
        })
        .collect()
    }

    fn open(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn every_member_with_subagents_starts_folded() {
        let members = members();
        assert_eq!(shown_members(&members, &HashSet::new()), [0, 1, 5]);
        assert!(is_folded(&members, &HashSet::new(), 1));
        assert!(!is_folded(&members, &HashSet::new(), 0), "no subagents");
    }

    #[test]
    fn unfolding_shows_one_level_at_a_time() {
        let members = members();
        assert_eq!(shown_members(&members, &open(&["re"])), [0, 1, 2, 4, 5]);
        let all = open(&["re", "s1"]);
        assert_eq!(shown_members(&members, &all), [0, 1, 2, 3, 4, 5]);
        // A subagent unfolded under a folded member stays hidden.
        assert_eq!(shown_members(&members, &open(&["s1"])), [0, 1, 5]);
    }

    #[test]
    fn a_hidden_member_shows_as_the_top_folded_member_over_it() {
        let members = members();
        assert_eq!(shown_member_of(&members, &HashSet::new(), 3), 1);
        assert_eq!(shown_member_of(&members, &open(&["re"]), 3), 2);
        assert_eq!(shown_member_of(&members, &open(&["re", "s1"]), 3), 3);
        assert_eq!(shown_member_of(&members, &HashSet::new(), 5), 5);
    }

    #[test]
    fn revealing_a_member_unfolds_those_above_it() {
        let members = members();
        let unfolded: HashSet<String> = ids_above(&members, 3).into_iter().collect();
        assert_eq!(unfolded, open(&["re", "s1"]));
        assert!(shown_members(&members, &unfolded).contains(&3));
    }

    #[test]
    fn a_folded_member_knows_a_subagent_under_it_works() {
        let members = members();
        let working = |member: &AgentSummary| member.id == "s2";
        assert!(hides_working(&members, &HashSet::new(), 1, working));
        assert!(!hides_working(&members, &open(&["re", "s1"]), 1, working));
        assert!(!hides_working(&members, &HashSet::new(), 5, working));
    }
}
