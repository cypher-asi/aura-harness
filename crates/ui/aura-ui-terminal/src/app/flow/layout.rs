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

use crate::events::{FlowKind, FlowNode, FlowUsage};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

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
    /// Index into [`FlowLayout::blocks`]
    pub block: usize,
    pub column: usize,
    /// Index into [`FlowLayout::bands`]
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
    /// Indices into [`FlowLayout::cards`]: the main card, then its subcards
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
    fn of(link: &str) -> Self {
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
/// ([`FlowLayout::stacked`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wire {
    /// Index into [`FlowLayout::cards`]
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
    fn wire_box(&self, wire: &Wire) -> CellRect {
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

/// A subcard before it is placed.
struct SubDraft {
    kind: CardKind,
    head: usize,
    rows: Vec<usize>,
}

/// A turn before it is placed.
struct TurnDraft {
    head: usize,
    rows: Vec<usize>,
    subs: Vec<SubDraft>,
    lane: usize,
    /// When it started, then its lane and record order
    key: (u64, usize, u64),
}

/// What the model calls of turn `draft` used together, and how long the
/// turn has taken: from its prompt to the last of its steps. `None`
/// before its first call is done.
fn turn_stats(draft: &TurnDraft, nodes: &[FlowNode]) -> Option<FlowUsage> {
    let mut total: Option<FlowUsage> = None;
    let mut last = nodes[draft.head].at_ms;
    for sub in &draft.subs {
        for &node in std::iter::once(&sub.head).chain(&sub.rows) {
            last = last.max(nodes[node].at_ms);
        }
        if let Some(usage) = nodes[sub.head].usage {
            total = Some(total.map_or(usage, |total| total.plus(usage)));
        }
    }
    for &node in &draft.rows {
        last = last.max(nodes[node].at_ms);
    }
    let wall = last.saturating_sub(nodes[draft.head].at_ms);
    total.map(|total| FlowUsage {
        duration_ms: Some(total.duration_ms.unwrap_or(0).max(wall)),
        ..total
    })
}

/// Whether `kind` is a model call the harness made around the agent's
/// work: Gate's, or the chat's title.
const fn harness_call(kind: FlowKind) -> bool {
    matches!(kind, FlowKind::Gate | FlowKind::Judge | FlowKind::Title)
}

/// Group the nodes into turns and place them, to fit a view `room` rows
/// tall; 0 when there is no view yet, and columns go down as far as they
/// need.
#[allow(clippy::too_many_lines)]
pub(super) fn build_layout(
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    index: &HashMap<String, usize>,
    folds: &Folds<'_>,
    room: usize,
) -> FlowLayout {
    let lane_count = lane_index.len();
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lane_count];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }

    // The row of the `task` call each result came back to.
    let mut answered = vec![NONE; nodes.len()];
    for (at, node) in nodes.iter().enumerate() {
        if node.kind != FlowKind::Tool {
            continue;
        }
        for (kind, other) in &node.links {
            if let Some(&result) = index.get(other).filter(|_| kind == "result") {
                if nodes[result].kind == FlowKind::Result {
                    answered[result] = at;
                }
            }
        }
    }

    // Within a lane the record's order holds; across lanes, time does. A
    // lane's clock never runs backwards, so the two cannot disagree.
    let mut drafts: Vec<TurnDraft> = Vec::new();
    let mut clock_of = vec![0u64; nodes.len()];
    for (lane, list) in per_lane.iter_mut().enumerate() {
        list.sort_by_key(|&at| (nodes[at].seq, at));
        let first = drafts.len();
        let mut clock = 0;
        let call = |head| SubDraft {
            kind: CardKind::Call,
            head,
            rows: Vec::new(),
        };
        // Gate classifies a prompt before the prompt is recorded; its call
        // is the first step of the turn the prompt starts.
        let mut routing: Vec<usize> = Vec::new();
        for &at in list.iter() {
            let node = &nodes[at];
            clock = clock.max(node.at_ms);
            clock_of[at] = clock;
            if answered[at] != NONE {
                continue;
            }
            if node.kind == FlowKind::Gate {
                routing.push(at);
                continue;
            }
            if node.kind == FlowKind::Prompt || drafts.len() == first {
                drafts.push(TurnDraft {
                    head: at,
                    rows: Vec::new(),
                    subs: routing.drain(..).map(call).collect(),
                    lane,
                    key: (clock, lane, node.seq),
                });
                continue;
            }
            let turn = drafts.last_mut().expect("a turn was started");
            match node.kind {
                FlowKind::Reply => turn.rows.push(at),
                kind if kind.is_model_call() => turn.subs.push(call(at)),
                FlowKind::Message | FlowKind::Result => turn.subs.push(SubDraft {
                    kind: CardKind::Note,
                    head: at,
                    rows: Vec::new(),
                }),
                // What the agent did goes under the agent's own last step,
                // never under a call the harness made around its work.
                _ => match turn
                    .subs
                    .iter_mut()
                    .rev()
                    .find(|sub| !harness_call(nodes[sub.head].kind))
                {
                    Some(sub) => sub.rows.push(at),
                    None => turn.rows.push(at),
                },
            }
        }
        // A prompt classified and not yet recorded: until it is, its call
        // waits in the turn before it.
        if !routing.is_empty() {
            match drafts.get_mut(first..).and_then(<[TurnDraft]>::last_mut) {
                Some(turn) => turn.subs.extend(routing.drain(..).map(call)),
                None => {
                    let head = routing.remove(0);
                    drafts.push(TurnDraft {
                        head,
                        rows: Vec::new(),
                        subs: routing.drain(..).map(call).collect(),
                        lane,
                        key: (clock_of[head], lane, nodes[head].seq),
                    });
                }
            }
        }
    }
    drafts.sort_by_key(|draft| (draft.key, draft.head));

    let mut cards: Vec<Card> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut lane_turns: Vec<Vec<usize>> = vec![Vec::new(); lane_count];
    let mut card_of = vec![NONE; nodes.len()];
    let mut row_of = vec![NONE; nodes.len()];
    let mut turn_head = vec![NONE; nodes.len()];
    let mut call_head = vec![NONE; nodes.len()];
    let mut lane_folded = vec![false; lane_count];
    for (agent, &lane) in lane_index {
        lane_folded[lane] = folds.lanes.contains(agent);
    }
    // The card a folded lane's turns are rows of, once it has one.
    let mut agent_card = vec![NONE; lane_count];
    for draft in &drafts {
        if lane_folded[draft.lane] {
            let stats = turn_stats(draft, nodes);
            let at = match agent_card[draft.lane] {
                NONE => {
                    let at = cards.len();
                    cards.push(Card {
                        kind: CardKind::Agent,
                        head: draft.head,
                        rows: Vec::new(),
                        lane: draft.lane,
                        block: blocks.len(),
                        column: 0,
                        band: 0,
                        top: 0,
                        steps: 0,
                        folded: true,
                        stats: None,
                    });
                    blocks.push(Block {
                        lane: draft.lane,
                        cards: at..at + 1,
                        column: 0,
                        top: 0,
                        height: 0,
                    });
                    lane_turns[draft.lane].push(at);
                    agent_card[draft.lane] = at;
                    at
                }
                at => at,
            };
            let card = &mut cards[at];
            card.rows.push(draft.head);
            card.steps += 1;
            card.stats = match (card.stats, stats) {
                (Some(a), Some(b)) => Some(a.plus(b)),
                (a, b) => a.or(b),
            };
            row_of[draft.head] = card.rows.len() - 1;
            let subs = draft
                .subs
                .iter()
                .flat_map(|sub| std::iter::once(&sub.head).chain(&sub.rows));
            for &node in std::iter::once(&draft.head).chain(&draft.rows).chain(subs) {
                card_of[node] = at;
                turn_head[node] = draft.head;
            }
            for sub in draft.subs.iter().filter(|sub| sub.kind == CardKind::Call) {
                for &node in std::iter::once(&sub.head).chain(&sub.rows) {
                    call_head[node] = sub.head;
                }
            }
            continue;
        }
        let folded = !draft.subs.is_empty() && folds.turns.contains(&nodes[draft.head].id);
        let main = cards.len();
        let block = blocks.len();
        let card = |kind, head, rows: Vec<usize>, steps, folded, stats| Card {
            kind,
            head,
            rows,
            lane: draft.lane,
            block,
            column: 0,
            band: 0,
            top: 0,
            steps,
            folded,
            stats,
        };
        cards.push(card(
            CardKind::Turn,
            draft.head,
            draft.rows.clone(),
            draft.subs.len(),
            folded,
            turn_stats(draft, nodes),
        ));
        lane_turns[draft.lane].push(main);
        card_of[draft.head] = main;
        for (row, &node) in draft.rows.iter().enumerate() {
            (card_of[node], row_of[node]) = (main, row);
        }
        for &node in std::iter::once(&draft.head).chain(&draft.rows) {
            turn_head[node] = draft.head;
        }
        for sub in &draft.subs {
            let all = std::iter::once(&sub.head).chain(&sub.rows);
            for &node in all.clone() {
                turn_head[node] = draft.head;
                if sub.kind == CardKind::Call {
                    call_head[node] = sub.head;
                }
            }
            if folded {
                for &node in all {
                    card_of[node] = main;
                }
                continue;
            }
            let rows_folded = sub.kind == CardKind::Call
                && !sub.rows.is_empty()
                && folds.calls.contains(&nodes[sub.head].id);
            let at = cards.len();
            card_of[sub.head] = at;
            for (row, &node) in sub.rows.iter().enumerate() {
                card_of[node] = at;
                if !rows_folded {
                    row_of[node] = row;
                }
            }
            let rows = if rows_folded {
                Vec::new()
            } else {
                sub.rows.clone()
            };
            let stats = nodes[sub.head].usage.filter(|_| sub.kind == CardKind::Call);
            cards.push(card(
                sub.kind,
                sub.head,
                rows,
                sub.rows.len(),
                rows_folded,
                stats,
            ));
        }
        blocks.push(Block {
            lane: draft.lane,
            cards: main..cards.len(),
            column: 0,
            top: 0,
            height: 0,
        });
    }
    for (result, &row) in answered.iter().enumerate() {
        if row != NONE {
            card_of[result] = card_of[row];
            row_of[result] = row_of[row];
            turn_head[result] = turn_head[row];
            call_head[result] = call_head[row];
        }
    }
    for block in &mut blocks {
        block.height = cards[block.cards.clone()]
            .iter()
            .map(Card::height)
            .sum::<usize>()
            + GAP * (block.cards.len() - 1);
    }

    // Links between cards, as wires before they have a track.
    let mut links: Vec<Wire> = Vec::new();
    let mut returns: Vec<(usize, usize, usize)> = Vec::new();
    for (at, node) in nodes.iter().enumerate() {
        for (kind, other) in &node.links {
            let Some(&other) = index.get(other) else {
                continue;
            };
            let (from, to) = (card_of[at], card_of[other]);
            if from == NONE || to == NONE || cards[from].lane == cards[to].lane {
                continue;
            }
            if WireKind::of(kind) == WireKind::Return {
                returns.push((from, to, other));
            }
            let row = (row_of[at] != NONE).then_some(row_of[at]);
            links.push(Wire {
                from,
                row,
                to,
                kind: WireKind::of(kind),
                track: 0,
            });
        }
    }
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for link in &links {
        let (from, to) = (cards[link.from].block, cards[link.to].block);
        // A link back in time is drawn when it can be, but places nothing.
        if from < to {
            incoming[to].push(from);
        }
    }

    let place = |blocks: &mut [Block], budget: usize| {
        let lane_blocks = place_columns(blocks, &incoming, lane_count, budget);
        place_bands(blocks, &lane_blocks)
    };
    let tall = |bands: &[Band]| bands.last().map_or(0, |band| band.y + band.height);
    // A band has its names above and a free row below.
    let full = match room {
        0 => usize::MAX,
        room => room.saturating_sub(2).max(1),
    };
    let (mut bands, mut spans) = place(&mut blocks, full);
    if room > 0 && bands.len() > 1 && tall(&bands) > room {
        // Bands too tall for the view together get the tallest columns
        // they all fit it with; a turn a column when none do.
        let (mut low, mut high) = (1, full);
        while high - low > 1 {
            let mid = (low + high) / 2;
            if tall(&place(&mut blocks, mid).0) <= room {
                low = mid;
            } else {
                high = mid;
            }
        }
        (bands, spans) = place(&mut blocks, low);
    }
    let mut lane_band = vec![0; lane_count];
    for span in &spans {
        lane_band[span.lane] = span.band;
    }
    let columns = blocks
        .iter()
        .map(|block| block.column + 1)
        .max()
        .unwrap_or(0);
    let mut by_column: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for block in &blocks {
        let band = lane_band[block.lane];
        let mut top = block.top;
        for card in block.cards.clone() {
            (cards[card].column, cards[card].band, cards[card].top) = (block.column, band, top);
            top += cards[card].height() + GAP;
            by_column[block.column].push(card);
        }
    }
    for list in &mut by_column {
        list.sort_by_key(|&card| (cards[card].band, cards[card].top));
    }

    // Down a column the order of turns is plain; only a wrap is wired.
    let mut wires: Vec<Wire> = Vec::new();
    for list in &lane_turns {
        for pair in list
            .windows(2)
            .filter(|pair| cards[pair[0]].column != cards[pair[1]].column)
        {
            wires.push(Wire {
                from: pair[0],
                row: None,
                to: pair[1],
                kind: WireKind::Next,
                track: 0,
            });
        }
    }
    for block in &blocks {
        for sub in block.cards.clone().skip(1) {
            wires.push(Wire {
                from: block.main(),
                row: None,
                to: sub,
                kind: WireKind::Spine,
                track: 0,
            });
        }
    }
    wires.extend(
        links
            .into_iter()
            .filter(|link| cards[link.from].column < cards[link.to].column),
    );
    wires.sort_unstable_by_key(|wire| (wire.to, wire.from, wire.row, wire.kind));
    wires.dedup_by_key(|wire| (wire.to, wire.from, wire.row));
    let mut reports: Vec<(usize, usize)> = returns
        .into_iter()
        .filter(|&(from, to, _)| cards[from].column >= cards[to].column)
        .map(|(from, _, node)| (from, node))
        .collect();
    reports.sort_unstable();
    reports.dedup_by_key(|report| report.0);

    let newest = (0..nodes.len())
        .filter(|&node| card_of[node] != NONE)
        .max_by_key(|&node| (clock_of[node], nodes[node].seq, node))
        .unwrap_or(NONE);
    let mut layout = FlowLayout {
        cards,
        blocks,
        bands,
        spans,
        wires,
        reports,
        columns,
        width: 0,
        height: 0,
        lane_turns,
        by_column,
        wire_boxes: Vec::new(),
        card_of,
        row_of,
        turn_head,
        call_head,
        newest,
    };
    layout.width = if columns == 0 {
        0
    } else {
        FlowLayout::column_x(columns) - GUTTER + MARGIN
    };
    layout.height = tall(&layout.bands);
    assign_tracks(&mut layout);
    layout.wire_boxes = layout
        .wires
        .iter()
        .map(|wire| layout.wire_box(wire))
        .collect();
    layout
}

/// Give each turn a column and its place down it: under the turn before
/// it in its lane while that column has room for it in `budget` rows,
/// else at the top of the next. A turn goes right of every turn whose
/// wire comes into it. Returns each lane's turns, in order.
fn place_columns(
    blocks: &mut [Block],
    incoming: &[Vec<usize>],
    lanes: usize,
    budget: usize,
) -> Vec<Vec<usize>> {
    let mut lane_blocks: Vec<Vec<usize>> = vec![Vec::new(); lanes];
    let mut previous_column = 0;
    for block in 0..blocks.len() {
        let lane = blocks[block].lane;
        let after_wires = incoming[block]
            .iter()
            .map(|&from| blocks[from].column + 1)
            .max();
        let height = blocks[block].height;
        let (column, top) = match lane_blocks[lane].last() {
            Some(&last) => {
                let column = blocks[last].column;
                let below = blocks[last].top + blocks[last].height + GAP;
                if after_wires.is_none_or(|after| after <= column) && below + height <= budget {
                    (column, below)
                } else {
                    (
                        after_wires.map_or(column + 1, |after| after.max(column + 1)),
                        0,
                    )
                }
            }
            // An agent nothing points to starts about when it started.
            None => (after_wires.unwrap_or(previous_column), 0),
        };
        (blocks[block].column, blocks[block].top) = (column, top);
        previous_column = column;
        lane_blocks[lane].push(block);
    }
    lane_blocks
}

/// Give each lane a band and each band its height. The agent the flow is
/// of has the first band to itself; every other lane takes the first band
/// whose last lane ended a column before it starts, so the two never touch.
fn place_bands(blocks: &[Block], lane_blocks: &[Vec<usize>]) -> (Vec<Band>, Vec<LaneSpan>) {
    let mut spans: Vec<LaneSpan> = lane_blocks
        .iter()
        .enumerate()
        .filter_map(|(lane, list)| {
            Some(LaneSpan {
                lane,
                band: 0,
                first: blocks[*list.first()?].column,
                last: blocks[*list.last()?].column,
            })
        })
        .collect();
    spans.sort_by_key(|span| (span.lane != 0, span.first, span.lane));
    let start = usize::from(spans.first().is_some_and(|span| span.lane == 0));
    let mut ends: Vec<Option<usize>> = Vec::new();
    for span in &mut spans {
        let band = if span.lane == 0 {
            0
        } else {
            (start..ends.len())
                .find(|&band| ends[band].is_none_or(|end| end + 1 < span.first))
                .unwrap_or(ends.len())
        };
        if band >= ends.len() {
            ends.resize(band + 1, None);
        }
        ends[band] = Some(span.last);
        span.band = band;
    }
    spans.sort_by_key(|span| (span.band, span.first));

    let mut heights = vec![0usize; ends.len()];
    for span in &spans {
        for &block in &lane_blocks[span.lane] {
            let block = &blocks[block];
            heights[span.band] = heights[span.band].max(block.top + block.height);
        }
    }
    let mut y = 0;
    let bands = heights
        .into_iter()
        .map(|tallest| {
            let band = Band {
                y,
                height: tallest + 2,
            };
            y += band.height;
            band
        })
        .collect();
    (bands, spans)
}

/// Give each wire that turns a column of the gutter before its card, so
/// wires into the same column turn side by side.
fn assign_tracks(layout: &mut FlowLayout) {
    let mut used: HashMap<usize, usize> = HashMap::new();
    for at in 0..layout.wires.len() {
        let wire = layout.wires[at];
        let (_, from_y) = layout.port_out(wire.from, wire.row);
        let (_, to_y) = layout.port_in(wire.to);
        if from_y == to_y || layout.stacked(&wire) {
            continue;
        }
        let count = used.entry(layout.cards[wire.to].column).or_default();
        layout.wires[at].track = *count % TRACKS;
        *count += 1;
    }
}
