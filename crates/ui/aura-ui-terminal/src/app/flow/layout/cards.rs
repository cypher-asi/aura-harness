//! Turning drafted turns into cards and blocks, and noting where each
//! node is shown.

use super::turns::{turn_stats, SubDraft, TurnDraft};
use super::{Block, Card, CardKind, Folds, GAP, NONE};
use crate::events::FlowNode;
use std::collections::HashMap;

/// The cards and blocks of the turns, before they have a place.
pub(super) struct Placed {
    pub(super) cards: Vec<Card>,
    pub(super) blocks: Vec<Block>,
    /// Each lane's main cards, in order
    pub(super) lane_turns: Vec<Vec<usize>>,
    /// The card each node is shown in
    pub(super) card_of: Vec<usize>,
    /// Each node's row in its card
    pub(super) row_of: Vec<usize>,
    /// The head of each node's turn, and of its model call
    pub(super) turn_head: Vec<usize>,
    pub(super) call_head: Vec<usize>,
}

/// A card of lane `lane`'s block `block` with no rows, not folded.
const fn blank_card(kind: CardKind, head: usize, lane: usize, block: usize) -> Card {
    Card {
        kind,
        head,
        rows: Vec::new(),
        lane,
        block,
        column: 0,
        band: 0,
        top: 0,
        steps: 0,
        folded: false,
        stats: None,
    }
}

/// An unplaced block of lane `lane`.
const fn blank_block(lane: usize, cards: std::ops::Range<usize>) -> Block {
    Block {
        lane,
        cards,
        column: 0,
        top: 0,
        height: 0,
    }
}

impl Placed {
    /// Cards for every turn of `drafts`, folded as `folds` says.
    pub(super) fn of_turns(
        lane_index: &HashMap<String, usize>,
        nodes: &[FlowNode],
        folds: &Folds<'_>,
        drafts: &[TurnDraft],
    ) -> Self {
        let lane_count = lane_index.len();
        let mut placed = Self {
            cards: Vec::new(),
            blocks: Vec::new(),
            lane_turns: vec![Vec::new(); lane_count],
            card_of: vec![NONE; nodes.len()],
            row_of: vec![NONE; nodes.len()],
            turn_head: vec![NONE; nodes.len()],
            call_head: vec![NONE; nodes.len()],
        };
        let mut lane_folded = vec![false; lane_count];
        for (agent, &lane) in lane_index {
            lane_folded[lane] = folds.lanes.contains(agent);
        }
        // The card a folded lane's turns are rows of, once it has one.
        let mut agent_card = vec![NONE; lane_count];
        for draft in drafts {
            if lane_folded[draft.lane] {
                placed.add_folded_turn(draft, nodes, &mut agent_card);
            } else {
                placed.add_turn(draft, nodes, folds);
            }
        }
        placed
    }

    /// Card `agent_card` says lane `draft.lane` is folded into; a new one
    /// for its first turn.
    fn agent_card_for(&mut self, draft: &TurnDraft, agent_card: &mut [usize]) -> usize {
        match agent_card[draft.lane] {
            NONE => {
                let at = self.cards.len();
                self.cards.push(Card {
                    folded: true,
                    ..blank_card(CardKind::Agent, draft.head, draft.lane, self.blocks.len())
                });
                self.blocks.push(blank_block(draft.lane, at..at + 1));
                self.lane_turns[draft.lane].push(at);
                agent_card[draft.lane] = at;
                at
            }
            at => at,
        }
    }

    /// Turn `draft` as a row of its folded lane's card.
    fn add_folded_turn(&mut self, draft: &TurnDraft, nodes: &[FlowNode], agent_card: &mut [usize]) {
        let stats = turn_stats(draft, nodes);
        let at = self.agent_card_for(draft, agent_card);
        let card = &mut self.cards[at];
        card.rows.push(draft.head);
        card.steps += 1;
        card.stats = match (card.stats, stats) {
            (Some(a), Some(b)) => Some(a.plus(b)),
            (a, b) => a.or(b),
        };
        self.row_of[draft.head] = card.rows.len() - 1;
        let subs = draft
            .subs
            .iter()
            .flat_map(|sub| std::iter::once(&sub.head).chain(&sub.rows));
        for &node in std::iter::once(&draft.head).chain(&draft.rows).chain(subs) {
            self.card_of[node] = at;
            self.turn_head[node] = draft.head;
        }
        for sub in draft.subs.iter().filter(|sub| sub.kind == CardKind::Call) {
            for &node in std::iter::once(&sub.head).chain(&sub.rows) {
                self.call_head[node] = sub.head;
            }
        }
    }

    /// Turn `draft` as a block: its main card and its subcards.
    fn add_turn(&mut self, draft: &TurnDraft, nodes: &[FlowNode], folds: &Folds<'_>) {
        let folded = !draft.subs.is_empty() && folds.turns.contains(&nodes[draft.head].id);
        let main = self.cards.len();
        let block = self.blocks.len();
        self.cards.push(Card {
            rows: draft.rows.clone(),
            steps: draft.subs.len(),
            folded,
            stats: turn_stats(draft, nodes),
            ..blank_card(CardKind::Turn, draft.head, draft.lane, block)
        });
        self.lane_turns[draft.lane].push(main);
        self.card_of[draft.head] = main;
        for (row, &node) in draft.rows.iter().enumerate() {
            (self.card_of[node], self.row_of[node]) = (main, row);
        }
        for &node in std::iter::once(&draft.head).chain(&draft.rows) {
            self.turn_head[node] = draft.head;
        }
        for sub in &draft.subs {
            self.add_sub(draft, sub, main, folded, nodes, folds);
        }
        self.blocks
            .push(blank_block(draft.lane, main..self.cards.len()));
    }

    /// Subcard `sub` of turn `draft`, whose main card is `main`; only its
    /// nodes noted when the turn is `folded`.
    fn add_sub(
        &mut self,
        draft: &TurnDraft,
        sub: &SubDraft,
        main: usize,
        folded: bool,
        nodes: &[FlowNode],
        folds: &Folds<'_>,
    ) {
        let all = std::iter::once(&sub.head).chain(&sub.rows);
        for &node in all.clone() {
            self.turn_head[node] = draft.head;
            if sub.kind == CardKind::Call {
                self.call_head[node] = sub.head;
            }
        }
        if folded {
            for &node in all {
                self.card_of[node] = main;
            }
            return;
        }
        let rows_folded = sub.kind == CardKind::Call
            && !sub.rows.is_empty()
            && folds.calls.contains(&nodes[sub.head].id);
        let at = self.cards.len();
        self.card_of[sub.head] = at;
        for (row, &node) in sub.rows.iter().enumerate() {
            self.card_of[node] = at;
            if !rows_folded {
                self.row_of[node] = row;
            }
        }
        let rows = if rows_folded {
            Vec::new()
        } else {
            sub.rows.clone()
        };
        let stats = nodes[sub.head].usage.filter(|_| sub.kind == CardKind::Call);
        self.cards.push(Card {
            rows,
            steps: sub.rows.len(),
            folded: rows_folded,
            stats,
            ..blank_card(sub.kind, sub.head, draft.lane, self.blocks.len())
        });
    }

    /// Show each result where the `task` row it came back to is.
    pub(super) fn follow_answers(&mut self, answered: &[usize]) {
        for (result, &row) in answered.iter().enumerate() {
            if row != NONE {
                self.card_of[result] = self.card_of[row];
                self.row_of[result] = self.row_of[row];
                self.turn_head[result] = self.turn_head[row];
                self.call_head[result] = self.call_head[row];
            }
        }
    }

    /// Give each block the height of its cards and the gaps between them.
    pub(super) fn size_blocks(&mut self) {
        for block in &mut self.blocks {
            block.height = self.cards[block.cards.clone()]
                .iter()
                .map(Card::height)
                .sum::<usize>()
                + GAP * (block.cards.len() - 1);
        }
    }
}
