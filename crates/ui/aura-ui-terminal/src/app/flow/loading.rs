//! Which agent's flow is shown, its nodes as they arrive, and their layout.

use super::layout::{build_layout, Folds};
use super::timeline::TimelineState;
use super::{FlowState, Kept, MAX_KEPT, MAX_NODES};
use crate::events::{FlowLane, FlowNode};
use std::collections::HashSet;

impl FlowState {
    /// Show the flow of `agent_id` from now on. Another agent's flow is
    /// dropped, though not its folds, selection and view, which come back
    /// with it; the same agent's stays on screen until the new one arrives.
    pub(super) fn watch(&mut self, agent_id: &str) {
        if self.agent_id == agent_id {
            return;
        }
        let mut kept = std::mem::take(&mut self.kept);
        if !self.agent_id.is_empty() {
            let state = Kept {
                turn_folded: std::mem::take(&mut self.turn_folded),
                call_folded: std::mem::take(&mut self.call_folded),
                lane_toggled: std::mem::take(&mut self.lane_toggled),
                big: self.big,
                selected: self.selected.take(),
                follow_tail: self.follow_tail,
                pan: self.pan,
                zoom: self.zoom,
                detached: self.detached,
                timeline: std::mem::take(&mut self.timeline),
            };
            kept.push((std::mem::take(&mut self.agent_id), state));
        }
        let state = kept
            .iter()
            .position(|(id, _)| id == agent_id)
            .map(|at| kept.remove(at).1)
            .unwrap_or_default();
        if kept.len() > MAX_KEPT {
            kept.drain(..kept.len() - MAX_KEPT);
        }
        *self = Self {
            open: self.open,
            agent_id: agent_id.to_string(),
            turn_folded: state.turn_folded,
            call_folded: state.call_folded,
            lane_toggled: state.lane_toggled,
            big: state.big,
            selected: state.selected,
            follow_tail: state.follow_tail,
            pan: state.pan,
            zoom: state.zoom,
            detached: state.detached,
            mode: self.mode,
            minimap_hidden: self.minimap_hidden,
            timeline: TimelineState {
                stale: true,
                ..state.timeline
            },
            kept,
            ..Self::default()
        };
    }

    pub(super) fn load(&mut self, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
        self.lanes.clear();
        self.lane_index.clear();
        self.nodes.clear();
        self.index.clear();
        self.loaded = true;
        self.upsert(lanes, nodes);
        let index = &self.index;
        self.turn_folded.retain(|id| index.contains_key(id));
        self.call_folded.retain(|id| index.contains_key(id));
        let lanes = &self.lane_index;
        self.lane_toggled.retain(|id| lanes.contains_key(id));
    }

    pub(super) fn upsert(&mut self, lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) {
        let known = self.lanes.len();
        for lane in lanes {
            self.add_lane(lane);
        }
        for node in nodes {
            if let Some(&at) = self.index.get(&node.id) {
                self.nodes[at].merge(node);
                continue;
            }
            if !self.lane_index.contains_key(&node.agent_id) {
                // The lane always comes first; name one that did not
                // rather than drop its nodes.
                self.add_lane(FlowLane {
                    agent_id: node.agent_id.clone(),
                    name: format!("agent {}", node.agent_id.get(..8).unwrap_or(&node.agent_id)),
                    ..FlowLane::default()
                });
            }
            self.index.insert(node.id.clone(), self.nodes.len());
            self.nodes.push(node);
        }
        if self.nodes.len() > MAX_NODES {
            let excess = self.nodes.len() - MAX_NODES * 3 / 4;
            self.nodes.drain(..excess);
            self.index = self
                .nodes
                .iter()
                .enumerate()
                .map(|(at, node)| (node.id.clone(), at))
                .collect();
        }
        self.update_big(known);
        self.stale = true;
        self.timeline.stale = true;
    }

    pub(super) fn add_lane(&mut self, lane: FlowLane) {
        match self.lane_index.get(&lane.agent_id) {
            Some(&at) => self.lanes[at] = lane,
            None => {
                self.lane_index
                    .insert(lane.agent_id.clone(), self.lanes.len());
                self.lanes.push(lane);
            }
        }
        self.stale = true;
        self.timeline.stale = true;
    }

    /// Fit the columns of cards to a view `rows` tall from the next
    /// layout on.
    pub fn set_room(&mut self, rows: usize) {
        if self.room != rows {
            self.room = rows;
            self.stale = true;
        }
    }

    /// Lay the flow out again if it changed, and keep the selection on
    /// something shown: the newest node while following the end, the head
    /// of what was folded over it.
    pub fn ensure_layout(&mut self) {
        if !self.stale {
            return;
        }
        self.stale = false;
        let lanes: HashSet<String> = self
            .lanes
            .iter()
            .map(|lane| &lane.agent_id)
            .filter(|agent| self.agent_is_folded(agent))
            .cloned()
            .collect();
        let folds = Folds {
            turns: &self.turn_folded,
            calls: &self.call_folded,
            lanes: &lanes,
        };
        self.layout = build_layout(
            &self.lane_index,
            &self.nodes,
            &self.index,
            &folds,
            self.room,
        );
        let selected = self
            .selected
            .as_deref()
            .and_then(|id| self.index.get(id).copied());
        let Some(newest) = self.newest() else {
            self.selected = None;
            return;
        };
        let node = match selected {
            Some(node) if !self.follow_tail => match self.layout.card_of(node) {
                Some(_) if self.is_folded_away(node) => self.layout.stand_in(node),
                Some(_) => node,
                None => newest,
            },
            Some(_) => newest,
            None => {
                self.follow_tail = true;
                newest
            }
        };
        self.selected = Some(self.nodes[node].id.clone());
    }

    /// The newest node shown.
    pub(super) fn newest(&self) -> Option<usize> {
        self.layout.newest()
    }

    /// Cards with a wire to or from the selected one.
    #[must_use]
    pub fn linked_cards(&self) -> HashSet<usize> {
        let Some(selected) = self.selected_card() else {
            return HashSet::new();
        };
        self.layout
            .wires
            .iter()
            .filter_map(|wire| {
                if wire.from == selected {
                    Some(wire.to)
                } else if wire.to == selected {
                    Some(wire.from)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Links of node `id` in both directions: (edge kind, other node id).
    /// Kinds of edges pointing at `id` are prefixed with `←`.
    #[must_use]
    pub fn links_of(&self, id: &str) -> Vec<(String, String)> {
        let mut links: Vec<(String, String)> = self
            .index
            .get(id)
            .map(|&at| self.nodes[at].links.clone())
            .unwrap_or_default();
        for node in &self.nodes {
            for (kind, other) in &node.links {
                if other == id {
                    links.push((format!("←{kind}"), node.id.clone()));
                }
            }
        }
        links.retain(|(_, other)| self.index.contains_key(other));
        links
    }
}
