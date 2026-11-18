//! Where everything on the GRAPH tab's canvas goes, in cells.
//!
//! A model call is one card, and the tool calls, spawns, finished
//! processes and events that follow it are its rows. Prompts, replies,
//! messages from other agents and the results of `task` calls are cards
//! of their own. Time runs left to right: a card is a column right of the
//! card before it in its agent's lane and of every card whose wire comes
//! into it, so wires always run forward. Each agent's cards run along a
//! band; agents that worked one after the other share one.
//!
//! ```text
//!   Lead
//!   ╭ ✓ prompt ──────────╮      ╭ ✓ model ── 1.2k · 4s ╮
//!   │ add a greeting     ●─────▶│ sonnet               ●──▶
//!   ╰────────────────────╯      │ ✓ task coder         ●─╮
//!                               ╰──────────────────────╯ │
//!   coder #1                                             │
//!                                                        ╰▶╭ ✓ prompt ─╮
//! ```
//!
//! The terminal draws the layout with box characters and the desktop
//! window with curves in pixels; both place it from these cells.

use crate::events::{FlowKind, FlowNode};
use std::collections::{HashMap, HashSet};

/// Stands for "none" where an index is expected.
pub(super) const NONE: usize = usize::MAX;

/// Columns of a card, borders included.
pub const CARD_WIDTH: usize = 34;
/// Columns between two cards side by side, where wires turn.
pub const GUTTER: usize = 6;
/// Columns from one card column to the next.
pub const PITCH: usize = CARD_WIDTH + GUTTER;
/// Empty columns left of the first card column.
pub const MARGIN: usize = 2;
/// Lines a card has at most below its head. A card with more rows lists
/// one fewer and says how many it leaves out on the last.
pub const MAX_LINES: usize = 10;
/// Columns of a gutter a wire can turn down or up in.
pub const TRACKS: usize = 4;

/// Whether a node of this kind starts a card of its own.
const fn heads_card(kind: FlowKind) -> bool {
    matches!(
        kind,
        FlowKind::Prompt | FlowKind::Model | FlowKind::Reply | FlowKind::Message | FlowKind::Result
    )
}

/// One card: a node and the rows that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Index into the flow's nodes
    pub head: usize,
    /// Indices into the flow's nodes, in record order
    pub rows: Vec<usize>,
    /// Index into the flow's lanes
    pub lane: usize,
    pub column: usize,
    /// Index into [`FlowLayout::bands`]
    pub band: usize,
    /// Other cards of the turn this prompt started; 0 for every other card
    pub steps: usize,
    /// Whether those cards are folded into this one
    pub folded: bool,
}

impl Card {
    /// Lines below the head.
    #[must_use]
    pub fn lines(&self) -> usize {
        self.rows.len().min(MAX_LINES)
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
    /// its lines and its bottom border.
    #[must_use]
    pub fn height(&self) -> usize {
        self.lines() + 3
    }

    /// The line, counted from the card's top, of row `row`, or of the
    /// head for `None`. Rows without a line of their own are on the last.
    #[must_use]
    pub fn line_of(&self, row: Option<usize>) -> usize {
        row.map_or(1, |row| 2 + row.min(self.lines().saturating_sub(1)))
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
}

/// Where one agent's cards run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneSpan {
    /// Index into the flow's lanes
    pub lane: usize,
    pub band: usize,
    /// First and last card column
    pub first: usize,
    pub last: usize,
}

/// A row of cards across the canvas, with the names of its agents above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Canvas row of the names; the cards start on the row below
    pub y: usize,
    pub height: usize,
}

/// What a wire stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WireKind {
    /// To the next card of the same agent
    Next,
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

/// A wire from a card's row (or head) to another card's head.
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
    /// A card's title; `fold` when it is on the fold marker of a turn
    Title { card: usize, fold: bool },
    /// A card's head
    Head(usize),
    /// A card's row
    Row(usize, usize),
}

impl Spot {
    #[must_use]
    pub const fn card(self) -> usize {
        match self {
            Self::Title { card, .. } | Self::Head(card) | Self::Row(card, _) => card,
        }
    }
}

/// Where every card of the flow goes.
#[derive(Debug, Default)]
pub struct FlowLayout {
    /// Cards in time order
    pub cards: Vec<Card>,
    pub bands: Vec<Band>,
    /// Lanes with a card, by band then first column
    pub spans: Vec<LaneSpan>,
    /// Wires, sorted by the card they go to
    pub wires: Vec<Wire>,
    /// Card columns in use
    pub columns: usize,
    /// Size of the canvas, in cells
    pub width: usize,
    pub height: usize,
    /// Each lane's cards, left to right
    lane_cards: Vec<Vec<usize>>,
    /// The card each node is shown in: its own, or for a node folded
    /// away the prompt of its turn
    card_of: Vec<usize>,
    /// Each node's row in its card; [`NONE`] for heads and folded nodes
    row_of: Vec<usize>,
    /// The prompt that started each node's turn; [`NONE`] for prompts
    /// and for nodes before a lane's first prompt
    turn_of: Vec<usize>,
}

impl FlowLayout {
    /// Left edge of card column `column`.
    #[must_use]
    pub const fn column_x(column: usize) -> usize {
        MARGIN + column * PITCH
    }

    /// Where card `card` is.
    #[must_use]
    pub fn card_rect(&self, card: usize) -> CellRect {
        let placed = &self.cards[card];
        CellRect {
            x: Self::column_x(placed.column),
            y: self.bands[placed.band].y + 1,
            width: CARD_WIDTH,
            height: placed.height(),
        }
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

    /// The prompt that started node `node`'s turn.
    #[must_use]
    pub fn turn_of(&self, node: usize) -> Option<usize> {
        self.turn_of.get(node).copied().filter(|&p| p != NONE)
    }

    /// Lane `lane`'s cards, left to right.
    #[must_use]
    pub fn lane_cards(&self, lane: usize) -> &[usize] {
        self.lane_cards.get(lane).map_or(&[], Vec::as_slice)
    }

    /// Whether row `row` of card `card` (or its head) has a wire leaving
    /// from it.
    #[must_use]
    pub fn has_port(&self, card: usize, row: Option<usize>) -> bool {
        let line = self.cards[card].line_of(row);
        self.wires
            .iter()
            .any(|wire| wire.from == card && self.cards[card].line_of(wire.row) == line)
    }

    /// What the canvas cell at `(x, y)` is part of.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn spot_at(&self, x: f32, y: f32) -> Option<Spot> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let (x, y) = (x.floor() as usize, y.floor() as usize);
        let column = x.checked_sub(MARGIN)? / PITCH;
        let card = self.cards.iter().enumerate().find_map(|(index, card)| {
            if card.column != column {
                return None;
            }
            let rect = self.card_rect(index);
            (x < rect.right() && y >= rect.y && y < rect.bottom()).then_some(index)
        })?;
        let rect = self.card_rect(card);
        let placed = &self.cards[card];
        Some(match y - rect.y {
            0 => Spot::Title {
                card,
                fold: placed.steps > 0 && x == rect.x + 2,
            },
            line if line >= 2 && line < 2 + placed.lines() => Spot::Row(card, line - 2),
            _ => Spot::Head(card),
        })
    }
}

/// A card before it is placed.
struct Draft {
    head: usize,
    rows: Vec<usize>,
    lane: usize,
    /// When it happened, then its lane and record order
    key: (u64, usize, u64),
    /// The draft of the prompt that started its turn
    turn: usize,
}

/// Group the nodes into cards and place them.
pub(super) fn build_layout(
    lane_index: &HashMap<String, usize>,
    nodes: &[FlowNode],
    index: &HashMap<String, usize>,
    collapsed: &HashSet<String>,
) -> FlowLayout {
    let lane_count = lane_index.len();
    let mut per_lane: Vec<Vec<usize>> = vec![Vec::new(); lane_count];
    for (at, node) in nodes.iter().enumerate() {
        if let Some(&lane) = lane_index.get(&node.agent_id) {
            per_lane[lane].push(at);
        }
    }

    // Within a lane the record's order holds; across lanes, time does. A
    // lane's clock never runs backwards, so the two cannot disagree.
    let mut drafts: Vec<Draft> = Vec::new();
    let mut draft_of = vec![NONE; nodes.len()];
    let mut turn_of = vec![NONE; nodes.len()];
    for (lane, list) in per_lane.iter_mut().enumerate() {
        list.sort_by_key(|&at| (nodes[at].seq, at));
        let mut clock = 0;
        let mut current = NONE;
        let mut turn = NONE;
        for &at in list.iter() {
            let node = &nodes[at];
            clock = clock.max(node.at_ms);
            if heads_card(node.kind) || current == NONE {
                current = drafts.len();
                if node.kind == FlowKind::Prompt {
                    turn = current;
                }
                drafts.push(Draft {
                    head: at,
                    rows: Vec::new(),
                    lane,
                    key: (clock, lane, node.seq),
                    turn,
                });
            } else {
                drafts[current].rows.push(at);
            }
            draft_of[at] = current;
            if turn != NONE && drafts[turn].head != at {
                turn_of[at] = drafts[turn].head;
            }
        }
    }

    let mut steps = vec![0usize; drafts.len()];
    for (at, draft) in drafts.iter().enumerate() {
        if draft.turn != NONE && draft.turn != at {
            steps[draft.turn] += 1;
        }
    }
    let folded = |draft: usize| collapsed.contains(&nodes[drafts[draft].head].id);
    let hidden = |draft: usize| {
        let turn = drafts[draft].turn;
        turn != NONE && turn != draft && folded(turn)
    };

    let mut shown: Vec<usize> = (0..drafts.len()).filter(|&d| !hidden(d)).collect();
    shown.sort_by_key(|&d| (drafts[d].key, drafts[d].head));
    let mut card_of_draft = vec![NONE; drafts.len()];
    for (card, &draft) in shown.iter().enumerate() {
        card_of_draft[draft] = card;
    }
    let mut card_of = vec![NONE; nodes.len()];
    let mut row_of = vec![NONE; nodes.len()];
    for (draft_index, draft) in drafts.iter().enumerate() {
        let card = match card_of_draft[draft_index] {
            NONE => card_of_draft[draft.turn],
            card => {
                for (row, &node) in draft.rows.iter().enumerate() {
                    row_of[node] = row;
                }
                card
            }
        };
        card_of[draft.head] = card;
        for &node in &draft.rows {
            card_of[node] = card;
        }
    }
    let mut cards: Vec<Card> = shown
        .iter()
        .map(|&d| Card {
            head: drafts[d].head,
            rows: drafts[d].rows.clone(),
            lane: drafts[d].lane,
            column: 0,
            band: 0,
            steps: steps[d],
            folded: steps[d] > 0 && folded(d),
        })
        .collect();

    // Links between cards, as wires before they have a track.
    let mut links: Vec<Wire> = Vec::new();
    for (at, node) in nodes.iter().enumerate() {
        for (kind, other) in &node.links {
            let Some(&other) = index.get(other) else {
                continue;
            };
            let (from, to) = (card_of[at], card_of[other]);
            if from == NONE || to == NONE || from == to {
                continue;
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
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    for link in &links {
        // A link back in time is drawn when it can be, but places nothing.
        if link.from < link.to {
            incoming[link.to].push(link.from);
        }
    }

    let mut lane_cards: Vec<Vec<usize>> = vec![Vec::new(); lane_count];
    let mut previous_column = 0;
    for card in 0..cards.len() {
        let lane = cards[card].lane;
        let after_lane = lane_cards[lane].last().map(|&c| cards[c].column + 1);
        let after_wires = incoming[card]
            .iter()
            .map(|&from| cards[from].column + 1)
            .max();
        let column = match (after_lane, after_wires) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            // An agent nothing points to starts about when it started.
            (None, None) => previous_column,
        };
        cards[card].column = column;
        previous_column = column;
        lane_cards[lane].push(card);
    }
    let columns = cards.iter().map(|card| card.column + 1).max().unwrap_or(0);

    let (bands, spans) = place_bands(&mut cards, &lane_cards);

    let mut wires: Vec<Wire> = Vec::new();
    for list in &lane_cards {
        for pair in list.windows(2) {
            wires.push(Wire {
                from: pair[0],
                row: None,
                to: pair[1],
                kind: WireKind::Next,
                track: 0,
            });
        }
    }
    wires.extend(links.into_iter().filter(|link| {
        cards[link.from].lane != cards[link.to].lane
            && cards[link.from].column < cards[link.to].column
    }));
    wires.sort_unstable_by_key(|wire| (wire.to, wire.from, wire.row, wire.kind));
    wires.dedup_by_key(|wire| (wire.to, wire.from, wire.row));

    let mut layout = FlowLayout {
        cards,
        bands,
        spans,
        wires,
        columns,
        width: 0,
        height: 0,
        lane_cards,
        card_of,
        row_of,
        turn_of,
    };
    layout.width = if columns == 0 {
        0
    } else {
        FlowLayout::column_x(columns) - GUTTER + MARGIN
    };
    layout.height = layout.bands.last().map_or(0, |band| band.y + band.height);
    assign_tracks(&mut layout);
    layout
}

/// Give each lane a band and each band its height. The agent the flow is
/// of has the first band to itself; every other lane takes the first band
/// whose last lane ended a column before it starts, so the two never touch.
fn place_bands(cards: &mut [Card], lane_cards: &[Vec<usize>]) -> (Vec<Band>, Vec<LaneSpan>) {
    let mut spans: Vec<LaneSpan> = lane_cards
        .iter()
        .enumerate()
        .filter_map(|(lane, list)| {
            Some(LaneSpan {
                lane,
                band: 0,
                first: cards[*list.first()?].column,
                last: cards[*list.last()?].column,
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
        for &card in &lane_cards[span.lane] {
            cards[card].band = span.band;
            heights[span.band] = heights[span.band].max(cards[card].height());
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
        if from_y == to_y {
            continue;
        }
        let count = used.entry(layout.cards[wire.to].column).or_default();
        layout.wires[at].track = *count % TRACKS;
        *count += 1;
    }
}
