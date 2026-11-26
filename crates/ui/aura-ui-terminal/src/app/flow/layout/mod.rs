//! Where everything on the GRAPH tab's canvas goes, in cells.
//!
//! A turn is a block: its main card (the prompt, with the turn's replies
//! as rows) and, under it on a spine, a subcard for each model call with
//! the tool calls, spawns, finished processes and events that followed it
//! as rows. A message from another agent, and what a `task` call came back
//! with when no row stands for it, are small subcards of their own; a
//! result a `task` row links to is that row.
//!
//! An agent's turns go down a column, each under the one before, until
//! the column has no room for the next; it starts the next column. A turn
//! is right of every turn of another agent whose wire comes into it, so
//! those wires always run forward: from a `task` row to the main card of
//! the turn it started. Each agent's turns are in a band; agents that
//! worked one after the other share one, and the bands share the height of
//! the view.
//!
//! ```text
//!   ┌ ▾ ✓ turn ─────────────── 2 calls ┐
//!   │ add a greeting                   │
//!   │ ↳ greeting added                 │
//!   └─┬────────────────────────────────┘
//!     │ ┌ ✓ model ───── 1.2k/300 · 4s ┐
//!     ├─┤ sonnet                      │   coder #1
//!     │ │ ✓ task coder · greet() 4.2s ├───┌ ✓ turn ─── coder #1 ┐
//!     │ └─────────────────────────────┘   │ write greet()       │
//! ```
//!
//! The terminal draws the layout with box characters and the desktop
//! window in pixels; both place it from these cells.

mod build;
mod cards;
mod elements;
mod geometry;
mod place;
mod turns;

pub use elements::{Band, Block, Card, CardKind, CellRect, LaneSpan, Spot, Wire, WireKind};

use crate::events::FlowNode;
use std::collections::{HashMap, HashSet};

/// Stands for "none" where an index is expected.
pub(super) const NONE: usize = usize::MAX;

/// Columns of a turn's main card, borders included.
pub const CARD_WIDTH: usize = 34;
/// Columns a turn's subcards are right of its main card.
pub const SUB_INDENT: usize = 4;
/// Columns right of a block's left edge its spine runs down.
pub const SPINE: usize = 2;
/// Columns between two blocks side by side, where wires turn.
pub const GUTTER: usize = 6;
/// Columns from one block column to the next.
pub const PITCH: usize = CARD_WIDTH + GUTTER;
/// Empty columns left of the first block column.
pub const MARGIN: usize = 2;
/// Lines a card has at most below its head. A card with more rows lists
/// one fewer and says how many it leaves out on the last.
pub const MAX_LINES: usize = 10;
/// Columns of a gutter a wire can turn down or up in.
pub const TRACKS: usize = 4;
/// Rows between a card and the one under it, where the spine runs.
pub const GAP: usize = 1;

/// What is folded, by the id of the head of what folds.
pub(super) struct Folds<'a> {
    /// Turns whose subcards are folded away
    pub turns: &'a HashSet<String>,
    /// Model calls whose rows are folded away
    pub calls: &'a HashSet<String>,
    /// Agents shown as one card, by hex id
    pub lanes: &'a HashSet<String>,
}

/// Where every card of the flow goes.
#[derive(Debug, Default)]
pub struct FlowLayout {
    /// Cards by block, blocks in time order
    pub cards: Vec<Card>,
    /// Turns in time order
    pub blocks: Vec<Block>,
    pub bands: Vec<Band>,
    /// Lanes with a turn, by band then first column
    pub spans: Vec<LaneSpan>,
    /// Wires, sorted by the card they go to
    pub wires: Vec<Wire>,
    /// Agents' reports back to the agent that asked them, which run back
    /// in time and so are not wired: the card the report leaves from and
    /// the node it went to, by card
    pub reports: Vec<(usize, usize)>,
    /// Block columns in use
    pub columns: usize,
    /// Size of the canvas, in cells
    pub width: usize,
    pub height: usize,
    /// Each lane's main cards, in order
    lane_turns: Vec<Vec<usize>>,
    /// Each column's cards, top to bottom
    by_column: Vec<Vec<usize>>,
    /// Where each wire is drawn, in the order of `wires`
    wire_boxes: Vec<CellRect>,
    /// The card each node is shown in: its own, or the card a fold put it
    /// in
    card_of: Vec<usize>,
    /// Each node's row in its card; [`NONE`] for heads and folded nodes
    row_of: Vec<usize>,
    /// The head of each node's turn, and of its model call; [`NONE`] for
    /// nodes outside one
    turn_head: Vec<usize>,
    call_head: Vec<usize>,
    /// The newest node shown, by when it happened
    newest: usize,
}

/// Group the nodes into turns and place them, to fit a view `room` rows
/// tall; 0 when there is no view yet, and columns go down as far as they
/// need.
pub(super) fn build_layout(
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    index: &HashMap<String, usize>,
    folds: &Folds<'_>,
    room: usize,
) -> FlowLayout {
    let lane_count = lane_index.len();
    let answered = turns::answered_rows(nodes, index);
    let (drafts, clock_of) = turns::group_turns(lane_index, nodes, &answered);
    let mut placed = cards::Placed::of_turns(lane_index, nodes, folds, &drafts);
    placed.follow_answers(&answered);
    placed.size_blocks();
    let (links, returns) = build::collect_links(nodes, index, &placed);
    let incoming = build::incoming_links(&links, &placed.cards, placed.blocks.len());
    let (bands, spans) = place::fit_to_room(&mut placed.blocks, &incoming, lane_count, room);
    let (columns, by_column) = build::settle_cards(&mut placed, &spans, lane_count);
    let wires = build::collect_wires(&placed, links);
    let reports = build::collect_reports(&placed.cards, returns);
    let newest = build::newest_node(nodes, &placed.card_of, &clock_of);
    build::finish(build::Settled {
        placed,
        bands,
        spans,
        wires,
        reports,
        columns,
        by_column,
        newest,
    })
}
