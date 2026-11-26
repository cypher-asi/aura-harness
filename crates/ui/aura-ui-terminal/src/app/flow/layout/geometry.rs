//! Where a laid-out flow's cards and wires are on the canvas, and what
//! is at a cell of it.

use super::{CardKind, CellRect, FlowLayout, Spot, Wire, CARD_WIDTH, MARGIN, NONE, PITCH, SPINE};
use std::ops::Range;

impl FlowLayout {
    /// Left edge of block column `column`.
    #[must_use]
    pub const fn column_x(column: usize) -> usize {
        MARGIN + column * PITCH
    }

    /// Where card `card` is.
    #[must_use]
    pub fn card_rect(&self, card: usize) -> CellRect {
        let placed = &self.cards[card];
        CellRect {
            x: Self::column_x(placed.column) + placed.indent(),
            y: self.bands[placed.band].y + 1 + placed.top,
            width: CARD_WIDTH - placed.indent(),
            height: placed.height(),
        }
    }

    /// The canvas column the spine of card `card`'s block runs down.
    #[must_use]
    pub fn spine_x(&self, card: usize) -> usize {
        Self::column_x(self.cards[card].column) + SPINE
    }

    /// Whether `wire` runs down the spine of its column rather than
    /// across a gutter.
    #[must_use]
    pub fn stacked(&self, wire: &Wire) -> bool {
        self.cards[wire.from].column == self.cards[wire.to].column
    }

    /// Canvas cell where a wire leaves card `card` from `row`, on its
    /// right border.
    #[must_use]
    pub fn port_out(&self, card: usize, row: Option<usize>) -> (usize, usize) {
        let rect = self.card_rect(card);
        (rect.right() - 1, rect.y + self.cards[card].line_of(row))
    }

    /// Canvas cell where a wire comes into card `card`, on its left
    /// border.
    #[must_use]
    pub fn port_in(&self, card: usize) -> (usize, usize) {
        let rect = self.card_rect(card);
        (rect.x, rect.y + 1)
    }

    /// The card node `node` is shown in.
    #[must_use]
    pub fn card_of(&self, node: usize) -> Option<usize> {
        self.card_of.get(node).copied().filter(|&c| c != NONE)
    }

    /// The row of its card node `node` is; `None` for a card's head and a
    /// node folded away.
    #[must_use]
    pub fn row_of(&self, node: usize) -> Option<usize> {
        self.row_of.get(node).copied().filter(|&r| r != NONE)
    }

    /// The heads of what node `node` is in that folds: its turn, and its
    /// model call.
    #[must_use]
    pub fn folds_over(&self, node: usize) -> (Option<usize>, Option<usize>) {
        let of = |heads: &[usize]| heads.get(node).copied().filter(|&h| h != NONE);
        (of(&self.turn_head), of(&self.call_head))
    }

    /// The node that stands for node `node` while a fold hides it: the
    /// head of its folded turn, or its turn's row on a folded agent's
    /// card. Itself when it is shown.
    #[must_use]
    pub fn stand_in(&self, node: usize) -> usize {
        let Some(card) = self.card_of(node) else {
            return node;
        };
        let placed = &self.cards[card];
        if self.row_of(node).is_some() || placed.head == node {
            return node;
        }
        match self.turn_head.get(node).copied() {
            Some(turn) if placed.kind == CardKind::Agent && turn != NONE => turn,
            _ => placed.head,
        }
    }

    /// The newest node shown.
    #[must_use]
    pub fn newest(&self) -> Option<usize> {
        (self.newest != NONE).then_some(self.newest)
    }

    /// Lane `lane`'s main cards, in order.
    #[must_use]
    pub fn lane_turns(&self, lane: usize) -> &[usize] {
        self.lane_turns.get(lane).map_or(&[], Vec::as_slice)
    }

    /// The main card of card `card`'s turn.
    #[must_use]
    pub fn main_of(&self, card: usize) -> usize {
        self.blocks[self.cards[card].block].main()
    }

    /// The cards that meet columns `xs` and rows `ys` of the canvas.
    #[must_use]
    pub fn visible_cards(&self, xs: &Range<usize>, ys: &Range<usize>) -> Vec<usize> {
        let first = xs.start.saturating_sub(MARGIN + CARD_WIDTH) / PITCH;
        let last = (xs.end / PITCH + 1).min(self.by_column.len());
        (first.min(last)..last)
            .flat_map(|column| &self.by_column[column])
            .copied()
            .filter(|&card| self.card_rect(card).meets(xs, ys))
            .collect()
    }

    /// The wires that meet columns `xs` and rows `ys` of the canvas, as
    /// indices into [`Self::wires`].
    #[must_use]
    pub fn visible_wires(&self, xs: &Range<usize>, ys: &Range<usize>) -> Vec<usize> {
        self.wire_boxes
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.meets(xs, ys))
            .map(|(at, _)| at)
            .collect()
    }

    /// What the canvas cell at `(x, y)` is part of.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn spot_at(&self, x: f32, y: f32) -> Option<Spot> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let (x, y) = (x.floor() as usize, y.floor() as usize);
        if let Some(span) = self.spans.iter().find(|span| {
            span.lane != 0
                && self.bands[span.band].y == y
                && (Self::column_x(span.first)..Self::column_x(span.last) + CARD_WIDTH).contains(&x)
        }) {
            return Some(Spot::Lane(span.lane));
        }
        let column = x.checked_sub(MARGIN)? / PITCH;
        let card = self
            .by_column
            .get(column)?
            .iter()
            .copied()
            .find(|&card| self.card_rect(card).meets(&(x..x + 1), &(y..y + 1)))?;
        let rect = self.card_rect(card);
        let placed = &self.cards[card];
        Some(match y - rect.y {
            0 => Spot::Title {
                card,
                fold: placed.folds() && (rect.x + 1..=rect.x + 3).contains(&x),
            },
            line if (placed.first_row_line()..placed.first_row_line() + placed.lines())
                .contains(&line) =>
            {
                Spot::Row(card, line - placed.first_row_line())
            }
            _ => Spot::Head(card),
        })
    }

    /// The cells wire `wire` is drawn in.
    pub(super) fn wire_box(&self, wire: &Wire) -> CellRect {
        let (from, to) = (self.card_rect(wire.from), self.card_rect(wire.to));
        if self.stacked(wire) {
            let x = self.spine_x(wire.from);
            let bottom = to.y + 2;
            return CellRect {
                x,
                y: from.bottom() - 1,
                width: to.x.max(x + 1) - x + 1,
                height: bottom.saturating_sub(from.bottom() - 1),
            };
        }
        let (out_x, out_y) = self.port_out(wire.from, wire.row);
        let (in_x, in_y) = self.port_in(wire.to);
        CellRect {
            x: out_x,
            y: out_y.min(in_y),
            width: in_x.saturating_sub(out_x) + 1,
            height: out_y.abs_diff(in_y) + 1,
        }
    }
}
