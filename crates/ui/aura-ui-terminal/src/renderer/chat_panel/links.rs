//! The chat's links on screen: moved to the cells packing put their text
//! in, and the one under the pointer underlined.
//!
//! Links are laid out on the grid, a display column per cell. A front
//! end that sets prose in a proportional face packs each prose row's
//! text into fewer or more cells (see [`super::prose`]), so a link's
//! columns on such a row are moved to the cells its text went to, where
//! the pointer finds it.

use super::prose::ColumnMap;
use crate::app::{HoverTarget, LinkHit};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

/// The rows of `hits` among the `height` from conversation line `start`,
/// each on a packed row (by its row on screen, in `packed`) moved to the
/// cells its text went to.
pub(super) fn place(
    hits: &[LinkHit],
    packed: &[(usize, ColumnMap)],
    (start, height): (usize, usize),
) -> Vec<LinkHit> {
    hits.iter()
        .filter(|hit| (start..start + height).contains(&hit.line))
        .map(|hit| {
            let row = hit.line - start;
            let Some((_, map)) = packed.iter().find(|(packed_row, _)| *packed_row == row) else {
                return hit.clone();
            };
            let cells = map.cells(hit.x..hit.x.saturating_add(hit.width));
            LinkHit {
                x: cells.start,
                width: cells.end - cells.start,
                ..hit.clone()
            }
        })
        .filter(|hit| hit.width > 0)
        .collect()
}

/// Underline every row of the link the pointer (`hover`) is on, so a
/// link that wraps reads as one; `start` is the chat line at the top of
/// `padded`.
pub(super) fn underline_hovered(
    buffer: &mut Buffer,
    hits: &[LinkHit],
    hover: Option<HoverTarget>,
    (padded, start): (Rect, usize),
) {
    let Some(HoverTarget::Link(link)) = hover else {
        return;
    };
    for hit in hits.iter().filter(|hit| hit.link == link) {
        let Some(row) = hit
            .line
            .checked_sub(start)
            .and_then(|row| u16::try_from(row).ok())
        else {
            continue;
        };
        let cells = Rect {
            x: padded.x.saturating_add(hit.x),
            y: padded.y.saturating_add(row),
            width: hit.width,
            height: 1,
        }
        .intersection(padded);
        for x in cells.left()..cells.right() {
            if let Some(cell) = buffer.cell_mut((x, cells.y)) {
                cell.modifier.insert(Modifier::UNDERLINED);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn hit(line: usize, x: u16, width: u16) -> LinkHit {
        LinkHit {
            line,
            x,
            width,
            link: 0,
            url: Arc::from("https://x.dev"),
        }
    }

    #[test]
    fn links_off_screen_are_left_out_and_grid_rows_stay_put() {
        let hits = [hit(1, 2, 5), hit(4, 0, 3), hit(9, 0, 3)];
        let placed = place(&hits, &[], (3, 4));
        assert_eq!(placed, [hit(4, 0, 3)]);
    }

    #[test]
    fn a_link_on_a_packed_row_moves_to_the_cells_its_text_went_to() {
        // Text after a 2-column indent: 10 columns packed into 6 cells,
        // then a 6-column link packed into 4.
        let map = ColumnMap::from_runs(vec![(2..12, 2..8), (12..18, 8..12)]);
        let placed = place(&[hit(5, 12, 6)], &[(0, map)], (5, 2));
        assert_eq!(placed, [hit(5, 8, 4)]);
    }
}
