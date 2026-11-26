//! Placing blocks in columns and lanes in bands, to fit the view.

use super::{Band, Block, LaneSpan, GAP};

/// Rows the bands take together.
pub(super) fn bands_height(bands: &[Band]) -> usize {
    bands.last().map_or(0, |band| band.y + band.height)
}

/// Place the blocks in columns at most `budget` rows tall, and the lanes
/// in bands.
fn place(
    blocks: &mut [Block],
    incoming: &[Vec<usize>],
    lane_count: usize,
    budget: usize,
) -> (Vec<Band>, Vec<LaneSpan>) {
    let lane_blocks = place_columns(blocks, incoming, lane_count, budget);
    place_bands(blocks, &lane_blocks)
}

/// Place the blocks to fit a view `room` rows tall; 0 when there is no
/// view yet, and columns go down as far as they need.
pub(super) fn fit_to_room(
    blocks: &mut [Block],
    incoming: &[Vec<usize>],
    lane_count: usize,
    room: usize,
) -> (Vec<Band>, Vec<LaneSpan>) {
    // A band has its names above and a free row below.
    let full = match room {
        0 => usize::MAX,
        room => room.saturating_sub(2).max(1),
    };
    let (mut bands, mut spans) = place(blocks, incoming, lane_count, full);
    if room > 0 && bands.len() > 1 && bands_height(&bands) > room {
        // Bands too tall for the view together get the tallest columns
        // they all fit it with; a turn a column when none do.
        let (mut low, mut high) = (1, full);
        while high - low > 1 {
            let mid = (low + high) / 2;
            if bands_height(&place(blocks, incoming, lane_count, mid).0) <= room {
                low = mid;
            } else {
                high = mid;
            }
        }
        (bands, spans) = place(blocks, incoming, lane_count, low);
    }
    (bands, spans)
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
