//! The TRACE side tab: a live feed of recorded steps (model calls, tool
//! calls, harness decisions, rewards), newest last. Every agent's steps
//! are kept, in the order they arrived; the tab lists those of the agents
//! the side panel's scope lets through. Enter or a click opens the
//! highlighted step as JSON; `g` opens it in the run graph.

use std::collections::VecDeque;

use super::file_view::FileView;
use super::graph::node_title;
use super::processes::SIDE_PAGE;
use super::side_scope::AgentFilter;
use super::trace_json::{node_json, NodeLink};
use super::{follow_selection, App, DetailView};
use crate::events::{TraceNode, TraceNodeStatus, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Entries kept in the feed for each agent.
const FEED_CAP: usize = 300;

/// Node kinds shown in the feed; runs and turns are structure only.
const FEED_KINDS: [&str; 4] = ["iteration", "tool_call", "decision", "reward"];

/// One feed row.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    /// Hex id of the agent whose step it is.
    pub agent_id: String,
    /// Run the node belongs to.
    pub run_id: String,
    /// The node as last updated.
    pub node: TraceNode,
}

impl FeedEntry {
    fn is(&self, (agent_id, run_id, node_id): (&str, &str, &str)) -> bool {
        self.agent_id == agent_id && self.run_id == run_id && self.node.id == node_id
    }

    fn key(&self) -> (String, String, String) {
        (
            self.agent_id.clone(),
            self.run_id.clone(),
            self.node.id.clone(),
        )
    }
}

/// Live feed state.
#[derive(Debug, Default)]
pub struct TraceFeed {
    /// Every agent's steps, oldest first
    entries: VecDeque<FeedEntry>,
    /// Agents whose steps are listed
    filter: AgentFilter,
    /// Indices in `entries` of the rows listed, oldest first
    shown: Vec<usize>,
    /// Highlighted row, in `shown`
    selected: usize,
    /// Keep the newest entry highlighted as entries arrive.
    pinned: bool,
}

impl TraceFeed {
    /// Rows listed, oldest first.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &FeedEntry> {
        self.shown.iter().map(|&index| &self.entries[index])
    }

    /// Number of rows listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// Steps of agents the filter leaves out.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.entries.len() - self.shown.len()
    }

    /// Highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Rows listed with a warning or error.
    #[must_use]
    pub fn issues(&self) -> usize {
        self.entries()
            .filter(|e| {
                matches!(
                    e.node.status,
                    TraceNodeStatus::Warn | TraceNodeStatus::Error
                )
            })
            .count()
    }

    /// Add or update nodes of `agent_id`'s run `run_id`.
    pub fn push(&mut self, agent_id: &str, run_id: &str, nodes: &[TraceNode]) {
        let follow = self.pinned || self.shown.is_empty() || self.at_end();
        let keep = self.selected_entry().map(FeedEntry::key);
        for node in nodes {
            if !FEED_KINDS.contains(&node.kind.as_str()) {
                continue;
            }
            if let Some(entry) = self
                .entries
                .iter_mut()
                .rev()
                .find(|e| e.is((agent_id, run_id, &node.id)))
            {
                entry.node = node.clone();
                continue;
            }
            self.entries.push_back(FeedEntry {
                agent_id: agent_id.to_string(),
                run_id: run_id.to_string(),
                node: node.clone(),
            });
            if self
                .entries
                .iter()
                .filter(|e| e.agent_id == agent_id)
                .count()
                > FEED_CAP
            {
                if let Some(oldest) = self.entries.iter().position(|e| e.agent_id == agent_id) {
                    self.entries.remove(oldest);
                }
            }
        }
        self.reshow(follow, keep);
    }

    /// List only the steps of the agents `filter` lets through, keeping
    /// the highlight on the same step, or on the newest one when it was.
    pub fn set_filter(&mut self, filter: AgentFilter) {
        if filter == self.filter {
            return;
        }
        let follow = self.pinned || self.at_end();
        let keep = self.selected_entry().map(FeedEntry::key);
        self.filter = filter;
        self.reshow(follow, keep);
    }

    /// Work out the rows listed, and highlight the newest when `follow`,
    /// else the step `keep` names while it is listed.
    fn reshow(&mut self, follow: bool, keep: Option<(String, String, String)>) {
        self.shown = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                self.filter
                    .as_ref()
                    .is_none_or(|agents| agents.contains(&e.agent_id))
            })
            .map(|(index, _)| index)
            .collect();
        let last = self.shown.len().saturating_sub(1);
        let kept = keep.and_then(|(agent_id, run_id, node_id)| {
            self.entries()
                .position(|e| e.is((&agent_id, &run_id, &node_id)))
        });
        self.selected = if follow {
            last
        } else {
            kept.unwrap_or(self.selected).min(last)
        };
    }

    fn at_end(&self) -> bool {
        self.selected + 1 >= self.shown.len()
    }

    /// Move the highlight by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.pinned = self.selected == last;
    }

    /// Highlight `index` (clamped).
    pub fn select(&mut self, index: usize) {
        let last = self.shown.len().saturating_sub(1);
        self.selected = index.min(last);
        self.pinned = self.selected == last;
    }

    /// Highlighted entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&FeedEntry> {
        self.shown
            .get(self.selected)
            .map(|&index| &self.entries[index])
    }

    /// Where the highlight is: following the newest step, or on a step.
    pub(super) fn place(&self) -> FeedPlace {
        match self.selected_entry() {
            Some(entry) if !self.pinned && !self.at_end() => FeedPlace::Step(entry.key()),
            _ => FeedPlace::Newest,
        }
    }

    /// Put the highlight back at `place`; on a step no longer listed it
    /// stays where it is.
    pub(super) fn put_back(&mut self, place: &FeedPlace) {
        match place {
            FeedPlace::Newest => self.select(usize::MAX),
            FeedPlace::Step((agent_id, run_id, node_id)) => {
                let at = self
                    .entries()
                    .position(|e| e.is((agent_id, run_id, node_id)));
                if let Some(at) = at {
                    self.select(at);
                }
            }
        }
    }
}

/// Where the TRACE tab's highlight was (see [`TraceFeed::place`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FeedPlace {
    /// On the newest step, following new ones
    Newest,
    /// On the step with this agent, run and node id
    Step((String, String, String)),
}

impl App {
    /// The TRACE tab's feed.
    #[must_use]
    pub const fn trace_feed(&self) -> &TraceFeed {
        &self.trace_feed
    }

    /// Keep the highlighted feed row inside `visible` rows, returning the
    /// first row to draw.
    pub fn sync_trace_scroll(&mut self, visible: usize) -> usize {
        self.trace_scroll = follow_selection(
            self.trace_scroll,
            self.trace_feed.selected,
            visible,
            self.trace_feed.len(),
        );
        self.trace_scroll
    }

    /// Where the feed rows were drawn, for mouse clicks.
    pub fn set_trace_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.trace_rows = Some(area);
        self.areas.trace_scroll = scroll;
    }

    /// Keys of the TRACE tab.
    pub(super) fn handle_trace_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.trace_feed.move_by(-1),
            KeyCode::Down => self.trace_feed.move_by(1),
            KeyCode::PageUp => self.trace_feed.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.trace_feed.move_by(SIDE_PAGE),
            KeyCode::Home => self.trace_feed.select(0),
            KeyCode::End => self.trace_feed.select(usize::MAX),
            KeyCode::Enter => self.open_selected_trace_entry(),
            _ => {}
        }
    }

    /// Open the highlighted step over the chat panel as JSON, the way the
    /// run graph opens a node.
    pub(super) fn open_selected_trace_entry(&mut self) {
        let Some(entry) = self.trace_feed.selected_entry() else {
            return;
        };
        let node = &entry.node;
        let same_run = self.graph.run_id() == entry.run_id;
        let links: Vec<NodeLink> = node
            .links
            .iter()
            .map(|(kind, other)| {
                let label = same_run
                    .then(|| self.graph.node(other).map(|n| n.label.clone()))
                    .flatten();
                (kind.clone(), other.clone(), label)
            })
            .collect();
        let json = node_json(node, &links);
        let view = FileView::snapshot(&node_title(node), &json, "json", 1);
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Open the highlighted entry's run in the graph with the entry
    /// selected.
    pub(super) fn open_selected_trace_in_graph(&mut self) {
        let Some(entry) = self.trace_feed.selected_entry().cloned() else {
            return;
        };
        if self.graph.run_id() == entry.run_id && self.graph.node(&entry.node.id).is_some() {
            self.graph.select_id(&entry.node.id);
            self.open_detail(DetailView::Graph);
            return;
        }
        self.pending_graph_node = Some(entry.node.id.clone());
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::OpenGraph(Some(entry.run_id)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, status: TraceNodeStatus) -> TraceNode {
        TraceNode {
            id: id.into(),
            kind: kind.into(),
            label: id.into(),
            status,
            ..TraceNode::default()
        }
    }

    #[test]
    fn feed_keeps_steps_updates_in_place_and_follows_the_end() {
        let mut feed = TraceFeed::default();
        feed.push(
            "a",
            "r",
            &[
                node("run", "run", TraceNodeStatus::Ok),
                node("t1", "turn", TraceNodeStatus::Pending),
                node("s2", "iteration", TraceNodeStatus::Ok),
                node("tc:a", "tool_call", TraceNodeStatus::Pending),
            ],
        );
        assert_eq!(feed.len(), 2);
        assert_eq!(feed.selected(), 1);

        feed.push(
            "a",
            "r",
            &[node("tc:a", "tool_call", TraceNodeStatus::Error)],
        );
        assert_eq!(feed.len(), 2);
        assert_eq!(feed.issues(), 1);

        feed.move_by(-1);
        feed.push("a", "r", &[node("d5", "decision", TraceNodeStatus::Warn)]);
        assert_eq!(feed.selected(), 0, "a moved highlight stays put");
        feed.select(usize::MAX);
        feed.push("a", "r", &[node("s6", "iteration", TraceNodeStatus::Ok)]);
        assert_eq!(feed.selected(), 3, "the end highlight follows new rows");
    }

    #[test]
    fn feed_is_capped_for_each_agent() {
        let mut feed = TraceFeed::default();
        feed.push("b", "rb", &[node("b0", "iteration", TraceNodeStatus::Ok)]);
        for i in 0..(FEED_CAP + 5) {
            feed.push(
                "a",
                "r",
                &[node(&format!("s{i}"), "iteration", TraceNodeStatus::Ok)],
            );
        }
        assert_eq!(feed.len(), FEED_CAP + 1);
        let ids: Vec<&str> = feed.entries().take(2).map(|e| e.node.id.as_str()).collect();
        assert_eq!(ids, ["b0", "s5"], "the other agent's step stays");
    }

    #[test]
    fn the_filter_lists_its_agents_steps_and_keeps_the_highlight() {
        let mut feed = TraceFeed::default();
        for (agent, id) in [("a", "a1"), ("b", "b1"), ("a", "a2"), ("b", "b2")] {
            feed.push(agent, "r", &[node(id, "iteration", TraceNodeStatus::Ok)]);
        }
        feed.select(1);
        assert_eq!(
            feed.selected_entry().map(|e| e.node.id.as_str()),
            Some("b1")
        );

        feed.set_filter(Some(["b".to_string()].into()));
        let ids: Vec<&str> = feed.entries().map(|e| e.node.id.as_str()).collect();
        assert_eq!(ids, ["b1", "b2"]);
        assert_eq!(feed.hidden(), 2);
        assert_eq!(
            feed.selected_entry().map(|e| e.node.id.as_str()),
            Some("b1")
        );

        feed.set_filter(None);
        assert_eq!(feed.len(), 4);
        assert_eq!(
            feed.selected_entry().map(|e| e.node.id.as_str()),
            Some("b1")
        );
    }

    #[test]
    fn the_same_step_id_in_two_agents_runs_is_two_rows() {
        let mut feed = TraceFeed::default();
        feed.push("a", "r", &[node("s1", "iteration", TraceNodeStatus::Ok)]);
        feed.push("b", "r", &[node("s1", "iteration", TraceNodeStatus::Ok)]);
        assert_eq!(feed.len(), 2);
    }
}
