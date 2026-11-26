//! What a layout is made of: cards, blocks, bands, wires and the spots
//! of the canvas they cover.

use super::{MAX_LINES, SUB_INDENT};
use crate::events::FlowUsage;
use std::ops::Range;

/// What a card stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// A turn's main card: its prompt, with its replies as rows
    Turn,
    /// A model call, with the tool calls it made as rows
    Call,
    /// A message from another agent, or a result no row stands for
    Note,
    /// A folded agent: all its turns on one card, a row each
    Agent,
}

/// One card: a node and the rows that follow it.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub kind: CardKind,
    /// Index into the flow's nodes
    pub head: usize,
    /// Indices into the flow's nodes, in record order; none while folded
    pub rows: Vec<usize>,
    /// Index into the flow's lanes
    pub lane: usize,
    /// Index into [`FlowLayout::blocks`](super::FlowLayout::blocks)
    pub block: usize,
    pub column: usize,
    /// Index into [`FlowLayout::bands`](super::FlowLayout::bands)
    pub band: usize,
    /// Rows above it in its column, from the band's first card row
    pub top: usize,
    /// What folding it hides: a turn's subcards, a call's rows
    pub steps: usize,
    /// Whether those are folded away
    pub folded: bool,
    /// What a call used, or a turn's calls together and how long it
    /// took; on a line of its own under the head
    pub stats: Option<FlowUsage>,
}

impl Card {
    /// Lines of rows.
    #[must_use]
    pub fn lines(&self) -> usize {
        self.rows.len().min(MAX_LINES)
    }

    /// The line, counted from the card's top, of its first row.
    #[must_use]
    pub fn first_row_line(&self) -> usize {
        2 + usize::from(self.stats.is_some())
    }

    /// Rows that have a line of their own; the rest share the last.
    #[must_use]
    pub fn shown_rows(&self) -> usize {
        if self.rows.len() > MAX_LINES {
            MAX_LINES - 1
        } else {
            self.rows.len()
        }
    }

    /// Rows of the canvas the card takes: its title border, its head,
    /// its stats, its lines and its bottom border.
    #[must_use]
    pub fn height(&self) -> usize {
        self.first_row_line() + self.lines() + 1
    }

    /// The line, counted from the card's top, of row `row`, or of the
    /// head for `None`. Rows without a line of their own are on the last.
    #[must_use]
    pub fn line_of(&self, row: Option<usize>) -> usize {
        row.map_or(1, |row| {
            self.first_row_line() + row.min(self.lines().saturating_sub(1))
        })
    }

    /// Columns right of its block's left edge.
    #[must_use]
    pub const fn indent(&self) -> usize {
        match self.kind {
            CardKind::Turn | CardKind::Agent => 0,
            CardKind::Call | CardKind::Note => SUB_INDENT,
        }
    }

    /// Whether it folds: a turn with subcards, a call with rows, a
    /// folded agent.
    #[must_use]
    pub const fn folds(&self) -> bool {
        self.steps > 0 && !matches!(self.kind, CardKind::Note)
    }
}

/// A turn: its main card and its subcards, placed as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Index into the flow's lanes
    pub lane: usize,
    /// Indices into [`FlowLayout::cards`](super::FlowLayout::cards): the
    /// main card, then its subcards
    pub cards: Range<usize>,
    pub column: usize,
    pub top: usize,
    pub height: usize,
}

impl Block {
    /// The turn's main card.
    #[must_use]
    pub const fn main(&self) -> usize {
        self.cards.start
    }
}

/// A rectangle of the canvas, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl CellRect {
    #[must_use]
    pub const fn right(&self) -> usize {
        self.x + self.width
    }

    #[must_use]
    pub const fn bottom(&self) -> usize {
        self.y + self.height
    }

    /// Whether it overlaps columns `xs` and rows `ys`.
    #[must_use]
    pub const fn meets(&self, xs: &Range<usize>, ys: &Range<usize>) -> bool {
        self.x < xs.end && self.right() > xs.start && self.y < ys.end && self.bottom() > ys.start
    }
}

/// Where one agent's turns are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneSpan {
    /// Index into the flow's lanes
    pub lane: usize,
    pub band: usize,
    /// First and last block column
    pub first: usize,
    pub last: usize,
}

/// A row of block columns across the canvas, with the names of its agents
/// above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Canvas row of the names; the cards start on the row below
    pub y: usize,
    pub height: usize,
}

/// What a wire stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WireKind {
    /// To the agent's next turn
    Next,
    /// From a turn's main card down its spine to one of its subcards
    Spine,
    /// From a `task` call or a spawn to the agent it started
    Spawn,
    /// From an agent's last step back to the agent that asked it
    Return,
    /// A message from one agent to another
    Message,
    /// Any other link between agents
    Link,
}

impl WireKind {
    pub(super) fn of(link: &str) -> Self {
        match link {
            "spawn" => Self::Spawn,
            "return" => Self::Return,
            "message" => Self::Message,
            _ => Self::Link,
        }
    }
}

/// A wire from a card's row (or head) to another card's head. Within a
/// column it runs down its block's spine into a subcard's side
/// ([`FlowLayout::stacked`](super::FlowLayout::stacked)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wire {
    /// Index into [`FlowLayout::cards`](super::FlowLayout::cards)
    pub from: usize,
    /// Row of `from` it leaves from; `None` for its head
    pub row: Option<usize>,
    pub to: usize,
    pub kind: WireKind,
    /// Column of the gutter before `to` it turns in, counted leftwards
    pub track: usize,
}

/// What a cell of the canvas is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// A card's title; `fold` when it is on the card's fold button, the
    /// marker and the cell each side of it
    Title { card: usize, fold: bool },
    /// A card's head
    Head(usize),
    /// A card's row
    Row(usize, usize),
    /// The name of a lane above its band, which folds the lane
    Lane(usize),
}

impl Spot {
    /// The card it is on; `None` off every card.
    #[must_use]
    pub const fn card(self) -> Option<usize> {
        match self {
            Self::Title { card, .. } | Self::Head(card) | Self::Row(card, _) => Some(card),
            Self::Lane(_) => None,
        }
    }
}
