//! Channels, their posts, and a coordinator's chat timeline.

use crate::events::{AgentSummary, MessageData};
use serde::{Deserialize, Serialize};

/// A channel: a shared conversation between the user and a coordinator
/// agent, the agents it spawned, and the project they work in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSummary {
    /// Channel id
    pub id: String,
    /// Short unique name it is addressed by, such as `release`
    pub slug: String,
    /// What it is called
    pub title: String,
    /// What it is for
    #[serde(default)]
    pub description: String,
    /// Agent that coordinates the channel (hex id)
    pub coordinator_id: String,
    /// The project the channel's agents work in
    #[serde(default)]
    pub project: Option<ChannelProjectView>,
    /// Coordinator first, then its agents, each followed by its subagents
    /// (depth-first by parent); the coordinator and its agents are at
    /// depth 0. See [`nest_members`].
    #[serde(default)]
    pub members: Vec<AgentSummary>,
    /// Whether it was archived
    #[serde(default)]
    pub archived: bool,
    /// Where the members it starts run
    #[serde(default)]
    pub placement: crate::host::PlacementView,
}

/// The project of a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelProjectView {
    /// Project id
    pub id: String,
    /// What it is called: its folder's name
    pub name: String,
    /// Full path of its folder
    pub folder: String,
}

/// Who wrote a channel post.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPostAuthor {
    /// The user
    #[default]
    User,
    /// An agent of the channel
    Agent {
        /// Agent (hex id)
        id: String,
        /// Its short code name, such as `RE36`, when it has one
        #[serde(default)]
        code: Option<String>,
        /// Display name
        name: String,
    },
}

/// Characters past which a message the user posted gets a one-line
/// summary in the channel's HISTORY tab; shorter ones show as they are.
pub const POST_SUMMARY_MIN_CHARS: usize = 80;

/// Whether the user's post `text` is long enough to be summarized
/// ([`POST_SUMMARY_MIN_CHARS`]).
#[must_use]
pub fn wants_summary(text: &str) -> bool {
    text.trim().chars().count() > POST_SUMMARY_MIN_CHARS
}

/// One post in a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPostView {
    /// Position in the channel, counting from 1 with no gaps
    pub seq: u64,
    /// Who wrote it
    pub author: ChannelPostAuthor,
    /// What it says
    pub text: String,
    /// Agents it mentions (hex ids)
    #[serde(default)]
    pub mentions: Vec<String>,
    /// `seq` of the post it answers
    #[serde(default)]
    pub reply_to: Option<u64>,
    /// When it was posted, in Unix milliseconds
    pub at_ms: u64,
}

/// One entry of a coordinator's chat: a message of its conversation or a
/// post of its channel.
#[derive(Debug, Clone, Copy)]
pub enum TimelineItem<'a> {
    /// A message of the conversation
    Message(&'a MessageData),
    /// A post of the channel
    Post(&'a ChannelPostView),
}

/// `history` and `posts` in one list ordered by time. A message without a
/// timestamp counts as sent now, after every post. On equal times the
/// message comes first; otherwise each list keeps its own order, so
/// entries out of order within a list stay where they were relative to
/// each other.
#[must_use]
pub fn merge_timeline<'a>(
    history: &'a [MessageData],
    posts: &'a [ChannelPostView],
) -> Vec<TimelineItem<'a>> {
    let mut out = Vec::with_capacity(history.len() + posts.len());
    let mut messages = history.iter().peekable();
    let mut posts = posts.iter().peekable();
    loop {
        match (messages.peek(), posts.peek()) {
            (Some(message), Some(post)) => {
                if message.timestamp_ms.is_some_and(|at| at <= post.at_ms) {
                    out.extend(messages.next().map(TimelineItem::Message));
                } else {
                    out.extend(posts.next().map(TimelineItem::Post));
                }
            }
            (Some(_), None) => out.extend(messages.next().map(TimelineItem::Message)),
            (None, Some(_)) => out.extend(posts.next().map(TimelineItem::Post)),
            (None, None) => break,
        }
    }
    out
}

/// Orders a channel's members for display: the coordinator first, then
/// its agents beside it, each followed by the agents it spawned,
/// depth-first. Siblings keep their order in `members`. Each member comes
/// with the hex id of the agent that brought it in; members whose parent
/// is not among `members` count as the coordinator's agents. `depth` is 0
/// for the coordinator and its agents, and one more for each level of
/// subagents below them.
///
/// A coordinator missing from `members` is left out; its agents still
/// start at depth 0. Members caught in a parent cycle, unreachable from
/// the coordinator, are put at the top level too so none is dropped.
#[must_use]
pub fn nest_members(
    coordinator_id: &str,
    members: Vec<(AgentSummary, Option<String>)>,
) -> Vec<AgentSummary> {
    let parents = tree_parents(coordinator_id, &members);
    let slots: Vec<Option<AgentSummary>> =
        members.into_iter().map(|(agent, _)| Some(agent)).collect();
    let mut walk = MemberWalk {
        coordinator_id,
        out: Vec::with_capacity(slots.len()),
        parents,
        slots,
    };
    walk.visit_from_coordinator();
    walk.visit_unreached();
    walk.out
}

/// Parent of each member as placed in the tree: the coordinator when its
/// own parent is unknown. The coordinator itself has none.
fn tree_parents(
    coordinator_id: &str,
    members: &[(AgentSummary, Option<String>)],
) -> Vec<Option<String>> {
    use std::collections::HashSet;

    let ids: HashSet<String> = members.iter().map(|(a, _)| a.id.clone()).collect();
    members
        .iter()
        .map(|(agent, parent)| {
            if agent.id == coordinator_id {
                None
            } else {
                match parent {
                    Some(p) if p != &agent.id && ids.contains(p) => Some(p.clone()),
                    _ => Some(coordinator_id.to_string()),
                }
            }
        })
        .collect()
}

/// State of [`nest_members`]' walk: members not placed yet stay in
/// `slots`, placed ones are in `out` in display order.
struct MemberWalk<'a> {
    /// The channel's coordinator, whose agents sit beside it
    coordinator_id: &'a str,
    /// Parent of each member as placed in the tree ([`tree_parents`])
    parents: Vec<Option<String>>,
    slots: Vec<Option<AgentSummary>>,
    out: Vec<AgentSummary>,
}

impl MemberWalk<'_> {
    /// Depth-first walk with an explicit stack of (index, depth).
    fn visit(&mut self, root: usize, depth: usize) {
        let mut stack = vec![(root, depth)];
        while let Some((index, depth)) = stack.pop() {
            let Some(mut agent) = self.slots[index].take() else {
                continue;
            };
            let children: Vec<usize> = (0..self.slots.len())
                .filter(|&i| {
                    self.slots[i].is_some() && self.parents[i].as_deref() == Some(&agent.id)
                })
                .collect();
            let below = if agent.id == self.coordinator_id {
                depth
            } else {
                depth + 1
            };
            // Reversed so the first sibling is popped first.
            stack.extend(children.into_iter().rev().map(|i| (i, below)));
            agent.depth = depth;
            self.out.push(agent);
        }
    }

    /// Walks from the coordinator, or from its children when it is not a
    /// member.
    fn visit_from_coordinator(&mut self) {
        let coordinator_id = self.coordinator_id;
        let coordinator = self
            .slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|a| a.id == coordinator_id));
        if let Some(index) = coordinator {
            self.visit(index, 0);
        } else {
            let roots: Vec<usize> = (0..self.slots.len())
                .filter(|&i| self.parents[i].as_deref() == Some(coordinator_id))
                .collect();
            for index in roots {
                self.visit(index, 0);
            }
        }
    }

    /// Members in a parent cycle never reached from the coordinator.
    fn visit_unreached(&mut self) {
        for index in 0..self.slots.len() {
            if self.slots[index].is_some() {
                self.visit(index, 0);
            }
        }
    }
}
