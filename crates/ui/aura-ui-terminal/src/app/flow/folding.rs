//! Folding turns, model calls and agents of the GRAPH tab's flow.

use super::layout::CardKind;
use super::{FlowMode, FlowState, BIG_FLOW_LANES};

impl FlowState {
    /// Whether lane `lane` is shown as one card.
    #[must_use]
    pub fn lane_is_folded(&self, lane: usize) -> bool {
        self.lanes
            .get(lane)
            .is_some_and(|it| self.agent_is_folded(&it.agent_id))
    }

    /// Whether the flow is big enough that its agents start folded.
    #[must_use]
    pub const fn is_big(&self) -> bool {
        self.big
    }

    /// What agent `agent` starts as: folded in a big flow, but for the
    /// agent on screen.
    pub(super) fn folds_by_default(&self, agent: &str) -> bool {
        self.big && agent != self.agent_id
    }

    pub(super) fn agent_is_folded(&self, agent: &str) -> bool {
        self.folds_by_default(agent) != self.lane_toggled.contains(agent)
    }

    pub(super) fn set_agent_folded(&mut self, agent: &str, folded: bool) {
        if folded == self.folds_by_default(agent) {
            self.lane_toggled.remove(agent);
        } else {
            self.lane_toggled.insert(agent.to_string());
        }
    }

    /// The flow is big once it has many agents, or is of a benchmark.
    /// Agents that were on screen when it turned big stay as they were;
    /// the first `known` lanes are those.
    pub(super) fn update_big(&mut self, known: usize) {
        let big =
            self.lanes.len() > BIG_FLOW_LANES || self.lanes.iter().any(|lane| lane.bench.is_some());
        if big == self.big {
            return;
        }
        let shown: Vec<(String, bool)> = self.lanes[..known.min(self.lanes.len())]
            .iter()
            .map(|lane| (lane.agent_id.clone(), self.agent_is_folded(&lane.agent_id)))
            .collect();
        self.big = big;
        for (agent, folded) in shown {
            self.set_agent_folded(&agent, folded);
        }
    }

    /// Fold or unfold what the selection is in: the rows of a model call,
    /// else its turn. The selection goes to the head of what folds.
    pub(super) fn toggle_fold(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let placed = &self.layout.cards[card];
        if placed.kind == CardKind::Call && placed.folds() {
            self.toggle_card(card);
        } else {
            self.toggle_card(self.layout.main_of(card));
        }
    }

    /// Fold or unfold card `card`: a turn's subcards, a call's rows, a
    /// folded agent's turns.
    pub(super) fn toggle_card(&mut self, card: usize) {
        let placed = &self.layout.cards[card];
        if !placed.folds() {
            return;
        }
        if placed.kind == CardKind::Agent {
            let agent = self.lanes[placed.lane].agent_id.clone();
            let node = self.selected_node_index().unwrap_or(placed.head);
            return self.unfold_lane(&agent, node);
        }
        let id = self.nodes[placed.head].id.clone();
        let folded = match placed.kind {
            CardKind::Turn => &mut self.turn_folded,
            _ => &mut self.call_folded,
        };
        if !folded.remove(&id) {
            folded.insert(id.clone());
        }
        self.selected = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Fold every turn, or unfold them all when none is left open.
    pub(super) fn toggle_fold_all(&mut self) {
        let turns: Vec<(String, bool)> = self
            .layout
            .cards
            .iter()
            .filter(|card| card.kind == CardKind::Turn && card.folds())
            .map(|card| (self.nodes[card.head].id.clone(), card.folded))
            .collect();
        if turns.iter().all(|(_, folded)| *folded) {
            self.turn_folded.clear();
        } else {
            self.turn_folded.extend(turns.into_iter().map(|(id, _)| id));
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    pub(super) fn selected_node_index(&self) -> Option<usize> {
        self.index.get(self.selected.as_deref()?).copied()
    }

    /// Show agent `agent`'s turns again, with node `node` selected.
    pub(super) fn unfold_lane(&mut self, agent: &str, node: usize) {
        self.set_agent_folded(agent, false);
        self.selected = Some(self.nodes[node].id.clone());
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Show the agent node `node` is of as one card, or its turns again.
    pub(super) fn toggle_lane_of(&mut self, node: usize) {
        let agent = self.nodes[node].agent_id.clone();
        if self.agent_is_folded(&agent) {
            return self.unfold_lane(&agent, node);
        }
        self.set_agent_folded(&agent, true);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Fold lane `lane`, or unfold it, from its name above its band.
    pub(super) fn toggle_lane(&mut self, lane: usize) {
        let Some(&head) = self
            .layout
            .lane_turns(lane)
            .first()
            .map(|&card| &self.layout.cards[card].head)
        else {
            return;
        };
        self.toggle_lane_of(head);
        self.selected = Some(self.nodes[head].id.clone());
    }

    /// Show every agent as one card but the selected node's and the
    /// agent that spawned it.
    pub(super) fn fold_other_lanes(&mut self) {
        let Some(node) = self.selected_node_index() else {
            return;
        };
        self.fold_lanes_but(&self.nodes[node].agent_id.clone());
    }

    pub(super) fn fold_lanes_but(&mut self, agent: &str) {
        let parent = self
            .lane_index
            .get(agent)
            .and_then(|&lane| self.lanes[lane].parent.clone());
        let agents: Vec<String> = self
            .lanes
            .iter()
            .map(|lane| lane.agent_id.clone())
            .collect();
        for id in agents {
            let folded = id != agent && Some(&id) != parent.as_ref();
            self.set_agent_folded(&id, folded);
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// Show every agent's turns.
    pub(super) fn unfold_lanes(&mut self) {
        let agents: Vec<String> = self
            .lanes
            .iter()
            .map(|lane| lane.agent_id.clone())
            .collect();
        for id in agents {
            self.set_agent_folded(&id, false);
        }
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
    }

    /// From the timeline to the cards of the turn node `node` starts:
    /// every other agent folded but the one that spawned it.
    pub(super) fn drill_in(&mut self, node: usize) {
        let agent = self.nodes[node].agent_id.clone();
        self.fold_lanes_but(&agent);
        self.turn_folded.remove(&self.nodes[node].id);
        self.selected = Some(self.nodes[node].id.clone());
        self.set_mode(FlowMode::Canvas);
        self.from_timeline = true;
        self.came_from = None;
    }

    /// Unfold whatever hides node `node`.
    pub(super) fn unfold_to(&mut self, node: usize) {
        if let Some(agent) = self.nodes.get(node).map(|n| n.agent_id.clone()) {
            self.set_agent_folded(&agent, false);
        }
        let (turn, call) = self.layout.folds_over(node);
        if let Some(turn) = turn {
            self.turn_folded.remove(&self.nodes[turn].id);
        }
        if let Some(call) = call {
            self.call_folded.remove(&self.nodes[call].id);
        }
    }
}
