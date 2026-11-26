//! What the cards of the GRAPH tab say: titles, facts, stats, reports.

use super::layout::{self, CardKind, FlowLayout, Spot};
use super::{format_ms, FlowState};
use crate::app::format::{format_cost, format_tokens};
use crate::events::{FlowKind, FlowLane, FlowNode, TraceNodeStatus};

impl FlowState {
    /// Whether the GRAPH tab is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub(in crate::app) fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Whether the flow asked for has arrived.
    #[must_use]
    pub const fn loaded(&self) -> bool {
        self.loaded
    }

    /// Hex id of the agent the flow is of.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// The agents in the flow, the one it was asked for first.
    #[must_use]
    pub fn lanes(&self) -> &[FlowLane] {
        &self.lanes
    }

    /// All nodes, in arrival order.
    #[must_use]
    pub fn nodes(&self) -> &[FlowNode] {
        &self.nodes
    }

    /// The selected node.
    #[must_use]
    pub fn selected_node(&self) -> Option<&FlowNode> {
        let index = *self.index.get(self.selected.as_deref()?)?;
        self.nodes.get(index)
    }

    /// The layout as of the last [`Self::ensure_layout`].
    #[must_use]
    pub const fn layout(&self) -> &FlowLayout {
        &self.layout
    }

    /// The selected card, and its row (`None` for its head).
    #[must_use]
    pub fn selected_place(&self) -> Option<(usize, Option<usize>)> {
        let node = *self.index.get(self.selected.as_deref()?)?;
        let card = self.layout.card_of(node)?;
        Some((card, self.layout.row_of(node)))
    }

    /// The selected card.
    #[must_use]
    pub fn selected_card(&self) -> Option<usize> {
        self.selected_place().map(|(card, _)| card)
    }

    /// Canvas cell at the top left of the view.
    #[must_use]
    pub const fn pan(&self) -> (f32, f32) {
        self.pan
    }

    /// How large the canvas is drawn: screen cells to a canvas cell.
    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.zoom.0
    }

    /// The kind card `card`'s title names: `turn` for a turn's main card
    /// that starts at a prompt, else its head's kind.
    #[must_use]
    pub fn card_kind_label(&self, card: usize) -> &'static str {
        let placed = &self.layout.cards[card];
        let kind = self.nodes[placed.head].kind;
        if placed.kind == CardKind::Agent {
            "agent"
        } else if placed.kind == CardKind::Turn && kind == FlowKind::Prompt {
            "turn"
        } else {
            kind.label()
        }
    }

    /// Display name of the agent with hex id `agent_id`.
    #[must_use]
    pub fn lane_name(&self, agent_id: &str) -> &str {
        self.lane_index
            .get(agent_id)
            .map_or("agent", |&lane| self.lanes[lane].name.as_str())
    }

    /// What card `card`'s title says beside its kind: the agent for a
    /// turn in a spawned agent's lane, how many model calls a turn has or
    /// how many tool calls a folded call hides, and the head's own detail
    /// (model, tokens, time).
    #[must_use]
    pub fn card_facts(&self, card: usize) -> String {
        let placed = &self.layout.cards[card];
        let node = &self.nodes[placed.head];
        let mut facts: Vec<String> = Vec::new();
        if matches!(placed.kind, CardKind::Turn if placed.lane != 0)
            || placed.kind == CardKind::Agent
        {
            facts.push(self.lanes[placed.lane].name.clone());
        }
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        match placed.kind {
            CardKind::Turn if placed.steps > 0 => {
                let calls = plural(placed.steps, "step", "steps");
                facts.push(if placed.folded {
                    format!("+{calls}")
                } else {
                    calls
                });
            }
            CardKind::Call if placed.folded => {
                facts.push(format!("+{}", plural(placed.steps, "tool", "tools")));
            }
            CardKind::Agent => facts.push(plural(placed.steps, "turn", "turns")),
            _ => {}
        }
        // A call's tokens and time are on its stats line; a folded agent
        // has no detail of its own.
        let on_stats_line = (placed.kind == CardKind::Call && placed.stats.is_some())
            || placed.kind == CardKind::Agent;
        if !node.detail.is_empty() && !on_stats_line {
            facts.push(node.detail.clone());
        }
        facts.join(" · ")
    }

    /// Whether the pointer is over card `card`'s fold button.
    #[must_use]
    pub fn fold_hovered(&self, card: usize) -> bool {
        let head = self.layout.cards[card].head;
        self.fold_hover.as_deref() == Some(self.nodes[head].id.as_str())
    }

    /// The pointer is at canvas point `at`, or off the canvas for `None`.
    /// Returns whether that took it onto or off a fold button.
    pub(super) fn hover(&mut self, at: Option<(f32, f32)>) -> bool {
        let over = at
            .and_then(|(x, y)| self.layout.spot_at(x, y))
            .and_then(|spot| match spot {
                Spot::Title { card, fold: true } => {
                    Some(self.nodes[self.layout.cards[card].head].id.clone())
                }
                _ => None,
            });
        let changed = over != self.fold_hover;
        self.fold_hover = over;
        changed
    }

    /// The stats line of card `card`, when it has one: the tokens in and
    /// out, `↑48.2k ↓516`, and against the right edge its cost and time,
    /// `$0.042 · 3.2s`.
    #[must_use]
    pub fn card_stats(&self, card: usize) -> Option<(String, String)> {
        let usage = self.layout.cards[card].stats?;
        let tokens = format!(
            "↑{} ↓{}",
            format_tokens(usage.input_tokens),
            format_tokens(usage.output_tokens)
        );
        let spent: Vec<String> = usage
            .cost_usd
            .map(format_cost)
            .into_iter()
            .chain(usage.duration_ms.map(format_ms))
            .collect();
        Some((tokens, spent.join(" · ")))
    }

    /// The row the last line of card `card` shows when the card has more
    /// rows than lines: the selected one when it is among those, else
    /// `None` and the line says how many there are.
    #[must_use]
    pub fn last_line_row(&self, card: usize) -> Option<usize> {
        let shown = self.layout.cards[card].shown_rows();
        match self.selected_place() {
            Some((selected, Some(row))) if selected == card && row >= shown => Some(row),
            _ => None,
        }
    }

    /// What card `card`'s bottom border says when its agent reported back
    /// to the agent that asked it from there: `↩ reported to Agent`, or
    /// `↩ failed for Agent`, and whether the work went well.
    #[must_use]
    pub fn card_report(&self, card: usize) -> Option<(String, TraceNodeStatus)> {
        let &(_, node) = self.layout.reports.iter().find(|(from, _)| *from == card)?;
        let to = &self.nodes[node];
        let failed = to.status == TraceNodeStatus::Error
            || (to.kind == FlowKind::Prompt && to.label.contains(" did not finish the task "));
        let parent = self.lane_name(&to.agent_id);
        if failed {
            Some((format!("↩ failed for {parent}"), TraceNodeStatus::Error))
        } else {
            Some((format!("↩ reported to {parent}"), TraceNodeStatus::Ok))
        }
    }

    /// How the `task` call wire `wire` leaves from went, for a wire that
    /// gives an agent its work: pending while the agent works, then done
    /// or failed. `None` for any other wire.
    #[must_use]
    pub fn wire_status(&self, wire: &layout::Wire) -> Option<TraceNodeStatus> {
        if wire.kind != layout::WireKind::Spawn {
            return None;
        }
        let row = wire.row?;
        let node = *self.layout.cards[wire.from].rows.get(row)?;
        let node = &self.nodes[node];
        (node.kind == FlowKind::Tool).then_some(node.status)
    }

    /// What the last line of card `card` says of the rows it leaves out.
    #[must_use]
    pub fn more_label(&self, card: usize) -> String {
        let placed = &self.layout.cards[card];
        format!("⋯ {} more", placed.rows.len() - placed.shown_rows())
    }
}
