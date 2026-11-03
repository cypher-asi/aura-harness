//! Circular "aura" shown beside an agent while it is thinking.
//!
//! Two braille cells side by side make a 4x4 dot grid that looks square
//! in a terminal. Leaving out the corners gives a ring of eight dots, and
//! a short comet of lit dots chases around it.

use crate::themes::{BLUE, CYAN, PURPLE};
use ratatui::style::Color;

/// Ring dots clockwise from the top, as (cell, braille bit) pairs where
/// cell 0 is the left cell and 1 the right.
const RING: [(usize, u8); 8] = [
    (0, 0x08),
    (1, 0x01),
    (1, 0x10),
    (1, 0x20),
    (1, 0x40),
    (0, 0x80),
    (0, 0x04),
    (0, 0x02),
];

/// Dots lit at once: the head and the tail behind it.
const COMET_LEN: usize = 3;

/// Animation ticks per step around the ring.
const TICKS_PER_STEP: usize = 2;

/// Colors the ring glows through, one per lap.
const GLOW: [Color; 3] = [CYAN, BLUE, PURPLE];

/// Display width of [`aura_ring`]'s glyph.
pub const AURA_WIDTH: usize = 2;

/// The ring glyph and its color at animation tick `tick`.
#[must_use]
pub fn aura_ring(tick: usize) -> (String, Color) {
    let step = tick / TICKS_PER_STEP;
    let head = step % RING.len();
    let mut cells = [0u8; AURA_WIDTH];
    for back in 0..COMET_LEN {
        let (cell, bit) = RING[(head + RING.len() - back) % RING.len()];
        cells[cell] |= bit;
    }
    let glyph = cells
        .iter()
        .map(|&bits| char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' '))
        .collect();
    let color = GLOW[(step / RING.len()) % GLOW.len()];
    (glyph, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn lit_dots(glyph: &str) -> usize {
        glyph
            .chars()
            .map(|c| (u32::from(c) - 0x2800).count_ones() as usize)
            .sum()
    }

    #[test]
    fn one_lap_has_eight_distinct_frames_of_three_dots() {
        let frames: Vec<String> = (0..RING.len())
            .map(|step| aura_ring(step * TICKS_PER_STEP).0)
            .collect();
        assert_eq!(frames.iter().collect::<HashSet<_>>().len(), RING.len());
        for frame in &frames {
            assert_eq!(frame.chars().count(), AURA_WIDTH);
            assert_eq!(lit_dots(frame), COMET_LEN);
        }
    }

    #[test]
    fn ring_wraps_and_changes_color_each_lap() {
        let lap = RING.len() * TICKS_PER_STEP;
        assert_eq!(aura_ring(0).0, aura_ring(lap).0);
        assert_ne!(aura_ring(0).1, aura_ring(lap).1);
        assert_eq!(aura_ring(0), aura_ring(lap * GLOW.len()));
    }

    #[test]
    fn ring_leaves_out_the_corners() {
        let corners = [(0usize, 0x01u8), (1, 0x08), (0, 0x40), (1, 0x80)];
        for (cell, bit) in corners {
            assert!(!RING.contains(&(cell, bit)));
        }
    }
}
