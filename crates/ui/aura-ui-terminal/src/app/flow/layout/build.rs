//! The steps of building a layout after its cards are made: links
//! between cards, where cards end up, and the wires drawn.

use super::cards::Placed;
use super::place::bands_height;
use super::{Band, Card, FlowLayout, LaneSpan, Wire, WireKind, GAP, GUTTER, MARGIN, NONE, TRACKS};
use crate::events::FlowNode;
use std::collections::HashMap;

/// Reports back to an agent: the card it leaves from, the card it goes
/// to and the node it went to.
pub(super) type Returns = Vec<(usize, usize, usize)>;

/// Links between cards of different lanes, as wires before they have a
/// track, and the reports among them.
pub(super) fn collect_links(
    nodes: &[FlowNode],
    index: &HashMap<String, usize>,
    placed: &Placed,
) -> (Vec<Wire>, Returns) {
    let (cards, card_of, row_of) = (&placed.cards, &placed.card_of, &placed.row_of);
    let mut links: Vec<Wire> = Vec::new();
    let mut returns: Returns = Vec::new();
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
    (links, returns)
}

/// The blocks whose links come into each block, forward in time.
pub(super) fn incoming_links(links: &[Wire], cards: &[Card], blocks: usize) -> Vec<Vec<usize>> {
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); blocks];
    for link in links {
        let (from, to) = (cards[link.from].block, cards[link.to].block);
        // A link back in time is drawn when it can be, but places nothing.
        if from < to {
            incoming[to].push(from);
        }
    }
    incoming
}

/// Put each card where its block went; returns the block columns in use
/// and each column's cards, top to bottom.
pub(super) fn settle_cards(
    placed: &mut Placed,
    spans: &[LaneSpan],
    lane_count: usize,
) -> (usize, Vec<Vec<usize>>) {
    let cards = &mut placed.cards;
    let mut lane_band = vec![0; lane_count];
    for span in spans {
        lane_band[span.lane] = span.band;
    }
    let columns = placed
        .blocks
        .iter()
        .map(|block| block.column + 1)
        .max()
        .unwrap_or(0);
    let mut by_column: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for block in &placed.blocks {
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
    (columns, by_column)
}

/// The wires drawn: a lane's wraps to the next column, each block's
/// spine, and the links that run forward, sorted by the card they go to.
pub(super) fn collect_wires(placed: &Placed, links: Vec<Wire>) -> Vec<Wire> {
    let cards = &placed.cards;
    // Down a column the order of turns is plain; only a wrap is wired.
    let mut wires: Vec<Wire> = Vec::new();
    for list in &placed.lane_turns {
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
    for block in &placed.blocks {
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
    wires
}

/// The reports that run back in time, one per card they leave from.
pub(super) fn collect_reports(cards: &[Card], returns: Returns) -> Vec<(usize, usize)> {
    let mut reports: Vec<(usize, usize)> = returns
        .into_iter()
        .filter(|&(from, to, _)| cards[from].column >= cards[to].column)
        .map(|(from, _, node)| (from, node))
        .collect();
    reports.sort_unstable();
    reports.dedup_by_key(|report| report.0);
    reports
}

/// The newest node shown, by its lane's clock; [`NONE`] when none is.
pub(super) fn newest_node(nodes: &[FlowNode], card_of: &[usize], clock_of: &[u64]) -> usize {
    (0..nodes.len())
        .filter(|&node| card_of[node] != NONE)
        .max_by_key(|&node| (clock_of[node], nodes[node].seq, node))
        .unwrap_or(NONE)
}

/// What [`finish`] puts together.
pub(super) struct Settled {
    pub(super) placed: Placed,
    pub(super) bands: Vec<Band>,
    pub(super) spans: Vec<LaneSpan>,
    pub(super) wires: Vec<Wire>,
    pub(super) reports: Vec<(usize, usize)>,
    pub(super) columns: usize,
    pub(super) by_column: Vec<Vec<usize>>,
    pub(super) newest: usize,
}

/// The layout of `settled`, sized, with its wires' tracks and boxes.
pub(super) fn finish(settled: Settled) -> FlowLayout {
    let Settled {
        placed,
        bands,
        spans,
        wires,
        reports,
        columns,
        by_column,
        newest,
    } = settled;
    let mut layout = FlowLayout {
        cards: placed.cards,
        blocks: placed.blocks,
        bands,
        spans,
        wires,
        reports,
        columns,
        width: 0,
        height: 0,
        lane_turns: placed.lane_turns,
        by_column,
        wire_boxes: Vec::new(),
        card_of: placed.card_of,
        row_of: placed.row_of,
        turn_head: placed.turn_head,
        call_head: placed.call_head,
        newest,
    };
    layout.width = if columns == 0 {
        0
    } else {
        FlowLayout::column_x(columns) - GUTTER + MARGIN
    };
    layout.height = bands_height(&layout.bands);
    assign_tracks(&mut layout);
    layout.wire_boxes = layout
        .wires
        .iter()
        .map(|wire| layout.wire_box(wire))
        .collect();
    layout
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
