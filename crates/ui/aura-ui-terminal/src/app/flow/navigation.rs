//! Moving the GRAPH tab's selection: by key, along links and by click.

use super::layout::Spot;
use super::FlowState;
use crate::events::FlowKind;

impl FlowState {
    pub(super) fn select_node(&mut self, node: usize) {
        self.selected = Some(self.nodes[node].id.clone());
        self.follow_tail = self.newest() == Some(node);
        self.detached = false;
        self.came_from = None;
    }

    /// Select row `row` of card `card`, or its head for `None`.
    pub(super) fn select_place(&mut self, card: usize, row: Option<usize>) {
        let Some(placed) = self.layout.cards.get(card) else {
            return;
        };
        let node = row
            .and_then(|row| placed.rows.get(row).copied())
            .unwrap_or(placed.head);
        self.select_node(node);
    }

    pub(super) fn select_first(&mut self) {
        if !self.layout.cards.is_empty() {
            self.select_place(0, None);
        }
    }

    pub(super) fn select_newest(&mut self) {
        if let Some(node) = self.newest() {
            self.select_node(node);
        }
    }

    /// Move to the turn before or after the selected card's, in its
    /// agent's lane.
    pub(super) fn move_along(&mut self, forward: bool) {
        let Some(card) = self.selected_card() else {
            return self.select_first();
        };
        let main = self.layout.main_of(card);
        let list = self.layout.lane_turns(self.layout.cards[card].lane);
        let Some(at) = list.iter().position(|&c| c == main) else {
            return;
        };
        let next = if forward {
            list.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|at| list.get(at))
        };
        if let Some(&next) = next {
            self.select_place(next, None);
        }
    }

    /// Move a line down or up the selected card, and past its end to the
    /// card under or over it in its column, else the nearest card of the
    /// band below or above.
    pub(super) fn move_line(&mut self, down: bool) {
        let Some((card, row)) = self.selected_place() else {
            return self.select_first();
        };
        let rows = self.layout.cards[card].rows.len();
        let inside = match (down, row) {
            (true, None) if rows > 0 => Some(Some(0)),
            (true, Some(row)) if row + 1 < rows => Some(Some(row + 1)),
            (false, Some(0)) => Some(None),
            (false, Some(row)) => Some(Some(row - 1)),
            _ => None,
        };
        if let Some(row) = inside {
            return self.select_place(card, row);
        }
        let here = &self.layout.cards[card];
        let (band, column, top) = (here.band, here.column, here.top);
        let target = self
            .layout
            .cards
            .iter()
            .enumerate()
            .filter_map(|(index, other)| {
                let (bands, rows) = if down {
                    (other.band.checked_sub(band)?, other.top)
                } else {
                    // Up into a band, onto the bottom of a column.
                    (band.checked_sub(other.band)?, usize::MAX - other.top)
                };
                let along = other.column.abs_diff(column);
                let rows = match bands {
                    0 if along > 0 => return None,
                    0 if down => other.top.checked_sub(top + 1)?,
                    0 => top.checked_sub(other.top + 1)?,
                    _ => rows,
                };
                Some((bands, along, rows, other.column, index))
            })
            .min();
        if let Some((_, _, _, _, index)) = target {
            let last = self.layout.cards[index].rows.len().checked_sub(1);
            self.select_place(index, if down { None } else { last });
        }
    }

    /// Move to the previous or next prompt, in time order.
    pub(super) fn move_to_prompt(&mut self, forward: bool) {
        let current = self.selected_card().unwrap_or(0);
        let cards = &self.layout.cards;
        let is_prompt = |index: usize| self.nodes[cards[index].head].kind == FlowKind::Prompt;
        let target = if forward {
            (current + 1..cards.len()).find(|&index| is_prompt(index))
        } else {
            (0..current).rev().find(|&index| is_prompt(index))
        };
        if let Some(index) = target {
            self.select_place(index, None);
        }
    }

    /// Jump along a link of the selected node, to a node other than the
    /// one the last jump came from when there is a choice. Returns
    /// whether the node has a link to follow.
    pub(super) fn follow_link(&mut self) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let mut targets: Vec<String> = Vec::new();
        for (_, other) in self.links_of(&id) {
            if !targets.contains(&other) {
                targets.push(other);
            }
        }
        if targets.len() > 1 {
            targets.retain(|other| self.came_from.as_deref() != Some(other));
        }
        let Some(target) = targets.into_iter().next() else {
            return false;
        };
        if let Some(&node) = self
            .index
            .get(&target)
            .filter(|&&node| self.is_folded_away(node))
        {
            // The node is folded away: open what hides it to land on it.
            self.unfold_to(node);
        }
        self.selected = Some(target);
        self.came_from = Some(id);
        self.follow_tail = false;
        self.detached = false;
        self.stale = true;
        true
    }

    /// Whether node `node` is in a folded turn, shown only by its prompt.
    pub(super) fn is_folded_away(&self, node: usize) -> bool {
        match self.layout.card_of(node) {
            Some(card) => {
                self.layout.row_of(node).is_none() && self.layout.cards[card].head != node
            }
            None => false,
        }
    }

    /// What the screen cell `(column, row)` shows of the canvas.
    pub(super) fn spot_at_cell(&self, column: u16, row: u16) -> Option<Spot> {
        let (x, y) = self.view?.canvas_at(column, row)?;
        self.layout.spot_at(x, y)
    }

    /// Act on a click on `spot`: fold a turn at its marker, select what
    /// was clicked, and open it when it was selected already. Returns
    /// whether something opens.
    pub(super) fn click(&mut self, spot: Spot) -> bool {
        let (card, row) = match spot {
            Spot::Title { card, fold: true } => {
                self.toggle_card(card);
                return false;
            }
            Spot::Title { card, .. } | Spot::Head(card) => (card, None),
            Spot::Row(card, row) => (card, Some(row)),
            Spot::Lane(lane) => {
                self.toggle_lane(lane);
                return false;
            }
        };
        if self.selected_place() == Some((card, row)) {
            return true;
        }
        self.select_place(card, row);
        false
    }
}
