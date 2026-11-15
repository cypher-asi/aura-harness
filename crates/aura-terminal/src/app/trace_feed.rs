//! The TRACE side tab: a live feed of recorded steps (model calls, tool
//! calls, harness decisions, rewards), newest last. Enter opens the
//! highlighted step in the run graph.

use std::collections::VecDeque;

use super::{follow_selection, App, DetailView};
use crate::events::{TraceNode, TraceNodeStatus, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Entries kept in the feed.
const FEED_CAP: usize = 300;

/// Node kinds shown in the feed; runs and turns are structure only.
const FEED_KINDS: [&str; 4] = ["iteration", "tool_call", "decision", "reward"];

/// One feed row.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    /// Run the node belongs to.
    pub run_id: String,
    /// The node as last updated.
    pub node: TraceNode,
}

/// Live feed state.
#[derive(Debug, Default)]
pub struct TraceFeed {
    entries: VecDeque<FeedEntry>,
    selected: usize,
    /// Keep the newest entry highlighted as entries arrive.
    pinned: bool,
}

impl TraceFeed {
    /// Rows, oldest first.
    #[must_use]
    pub const fn entries(&self) -> &VecDeque<FeedEntry> {
        &self.entries
    }

    /// Highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Rows with a warning or error.
    #[must_use]
    pub fn issues(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                matches!(
                    e.node.status,
                    TraceNodeStatus::Warn | TraceNodeStatus::Error
                )
            })
            .count()
    }

    /// Add or update nodes of `run_id`.
    pub fn push(&mut self, run_id: &str, nodes: &[TraceNode]) {
        let follow = self.pinned || self.entries.is_empty() || self.at_end();
        for node in nodes {
            if !FEED_KINDS.contains(&node.kind.as_str()) {
                continue;
            }
            if let Some(entry) = self
                .entries
                .iter_mut()
                .rev()
                .find(|e| e.run_id == run_id && e.node.id == node.id)
            {
                entry.node = node.clone();
                continue;
            }
            self.entries.push_back(FeedEntry {
                run_id: run_id.to_string(),
                node: node.clone(),
            });
            if self.entries.len() > FEED_CAP {
                self.entries.pop_front();
                self.selected = self.selected.saturating_sub(1);
            }
        }
        if follow {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    fn at_end(&self) -> bool {
        self.selected + 1 >= self.entries.len()
    }

    /// Move the highlight by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.pinned = self.selected == last;
    }

    /// Highlight `index` (clamped).
    pub fn select(&mut self, index: usize) {
        let last = self.entries.len().saturating_sub(1);
        self.selected = index.min(last);
        self.pinned = self.selected == last;
    }

    /// Highlighted entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&FeedEntry> {
        self.entries.get(self.selected)
    }
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
        follow_selection(
            self.areas.trace_scroll,
            self.trace_feed.selected,
            visible,
            self.trace_feed.entries.len(),
        )
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
            KeyCode::PageUp => self.trace_feed.move_by(-10),
            KeyCode::PageDown => self.trace_feed.move_by(10),
            KeyCode::Home => self.trace_feed.select(0),
            KeyCode::End => self.trace_feed.select(usize::MAX),
            KeyCode::Enter => self.open_selected_trace_entry(),
            KeyCode::Left => self.show_side_tab(self.side_tab.prev()),
            KeyCode::Right => self.show_side_tab(self.side_tab.next()),
            _ => {}
        }
    }

    /// Open the highlighted entry's run in the graph with the entry
    /// selected.
    pub(super) fn open_selected_trace_entry(&mut self) {
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
            "r",
            &[
                node("run", "run", TraceNodeStatus::Ok),
                node("t1", "turn", TraceNodeStatus::Pending),
                node("s2", "iteration", TraceNodeStatus::Ok),
                node("tc:a", "tool_call", TraceNodeStatus::Pending),
            ],
        );
        assert_eq!(feed.entries().len(), 2);
        assert_eq!(feed.selected(), 1);

        feed.push("r", &[node("tc:a", "tool_call", TraceNodeStatus::Error)]);
        assert_eq!(feed.entries().len(), 2);
        assert_eq!(feed.issues(), 1);

        feed.move_by(-1);
        feed.push("r", &[node("d5", "decision", TraceNodeStatus::Warn)]);
        assert_eq!(feed.selected(), 0, "a moved highlight stays put");
        feed.select(usize::MAX);
        feed.push("r", &[node("s6", "iteration", TraceNodeStatus::Ok)]);
        assert_eq!(feed.selected(), 3, "the end highlight follows new rows");
    }

    #[test]
    fn feed_is_capped() {
        let mut feed = TraceFeed::default();
        for i in 0..(FEED_CAP + 5) {
            feed.push(
                "r",
                &[node(&format!("s{i}"), "iteration", TraceNodeStatus::Ok)],
            );
        }
        assert_eq!(feed.entries().len(), FEED_CAP);
        assert_eq!(feed.entries()[0].node.id, "s5");
    }
}
