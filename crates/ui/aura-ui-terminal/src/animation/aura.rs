//! Circular "aura" shown beside an agent while it is thinking.
//!
//! Two braille cells side by side make a 4x4 dot grid that looks square
//! in a terminal. Leaving out the corners gives a ring of eight dots around
//! a core of four. A short comet spirals out of the core onto the ring, its
//! head runs on until it meets its tail and closes the circle, the circle
//! holds, and then it winds back into the core to start over in a new color.

use crate::themes::{BLUE, CYAN, PURPLE};
use ratatui::style::Color;

/// A dot of the 4x4 grid as (x, y), with x running across both cells.
type Dot = (usize, usize);

/// Inner square clockwise from top-left, where the spiral starts.
const CORE: [Dot; 4] = [(1, 1), (2, 1), (2, 2), (1, 2)];

/// Outer ring clockwise, starting beside the core's last dot so the spiral
/// steps straight out onto it.
const RING: [Dot; 8] = [
    (0, 2),
    (0, 1),
    (1, 0),
    (2, 0),
    (3, 1),
    (3, 2),
    (2, 3),
    (1, 3),
];

/// Dots from the center to one lap of the ring.
const SPIRAL_LEN: usize = CORE.len() + RING.len();

/// Dots lit while the comet winds out.
const COMET_LEN: usize = 3;

/// Steps spent in each phase of one cycle.
const OUT_STEPS: usize = SPIRAL_LEN;
const CLOSE_STEPS: usize = RING.len() - COMET_LEN;
const HOLD_STEPS: usize = 4;
const IN_STEPS: usize = SPIRAL_LEN;
const CYCLE_STEPS: usize = OUT_STEPS + CLOSE_STEPS + HOLD_STEPS + IN_STEPS;

/// Animation ticks per step.
const TICKS_PER_STEP: usize = 2;

/// Colors the aura glows through, one per cycle.
const GLOW: [Color; 3] = [CYAN, BLUE, PURPLE];

/// Display width of [`aura_ring`]'s glyph.
pub const AURA_WIDTH: usize = 2;

/// The `i`th dot along the spiral, which keeps circling the ring past its end.
const fn spiral_dot(i: usize) -> Dot {
    if i < CORE.len() {
        CORE[i]
    } else {
        RING[(i - CORE.len()) % RING.len()]
    }
}

/// The span of spiral dots lit at `step` within a cycle, or `None` for the
/// blank beat between cycles.
fn lit_span(step: usize) -> Option<(usize, usize)> {
    let full_ring = (SPIRAL_LEN - COMET_LEN, SPIRAL_LEN + CLOSE_STEPS - 1);
    if step < OUT_STEPS {
        return Some((step.saturating_sub(COMET_LEN - 1), step));
    }
    let step = step - OUT_STEPS;
    if step < CLOSE_STEPS {
        return Some((full_ring.0, SPIRAL_LEN + step));
    }
    let step = step - CLOSE_STEPS;
    if step < HOLD_STEPS {
        return Some(full_ring);
    }
    let head = (SPIRAL_LEN - 2).checked_sub(step - HOLD_STEPS)?;
    Some((head.saturating_sub(RING.len() - 1), head))
}

/// Braille bit for the dot at column `col` (0 or 1) and row `row` of a cell.
const fn braille_bit(col: usize, row: usize) -> u8 {
    const LEFT: [u8; 4] = [0x01, 0x02, 0x04, 0x40];
    const RIGHT: [u8; 4] = [0x08, 0x10, 0x20, 0x80];
    if col == 0 {
        LEFT[row]
    } else {
        RIGHT[row]
    }
}

/// The aura glyph and its color at animation tick `tick`.
#[must_use]
pub fn aura_ring(tick: usize) -> (String, Color) {
    let step = tick / TICKS_PER_STEP;
    let mut cells = [0u8; AURA_WIDTH];
    if let Some((tail, head)) = lit_span(step % CYCLE_STEPS) {
        for i in tail..=head {
            let (x, y) = spiral_dot(i);
            cells[x / 2] |= braille_bit(x % 2, y);
        }
    }
    let glyph = cells
        .iter()
        .map(|&bits| char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' '))
        .collect();
    let color = GLOW[(step / CYCLE_STEPS) % GLOW.len()];
    (glyph, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(step: usize) -> String {
        aura_ring(step * TICKS_PER_STEP).0
    }

    fn bits(glyph: &str) -> Vec<u8> {
        glyph
            .chars()
            .map(|c| u8::try_from(u32::from(c) - 0x2800).unwrap())
            .collect()
    }

    fn lit_dots(glyph: &str) -> usize {
        bits(glyph).iter().map(|b| b.count_ones() as usize).sum()
    }

    fn glyph_of(dots: &[Dot]) -> String {
        let mut cells = [0u8; AURA_WIDTH];
        for &(x, y) in dots {
            cells[x / 2] |= braille_bit(x % 2, y);
        }
        cells
            .iter()
            .map(|&b| char::from_u32(0x2800 + u32::from(b)).unwrap())
            .collect()
    }

    #[test]
    fn spiral_starts_from_a_single_core_dot() {
        assert_eq!(frame(0), glyph_of(&CORE[..1]));
    }

    #[test]
    fn spiral_closes_into_the_full_ring_and_holds() {
        let ring = glyph_of(&RING);
        let closed = OUT_STEPS + CLOSE_STEPS - 1;
        for step in closed..closed + HOLD_STEPS + 1 {
            assert_eq!(frame(step), ring, "step {step}");
        }
        assert_ne!(frame(closed - 1), ring);
    }

    #[test]
    fn comet_grows_while_closing_the_ring() {
        let counts: Vec<usize> = (OUT_STEPS - 1..OUT_STEPS + CLOSE_STEPS)
            .map(|step| lit_dots(&frame(step)))
            .collect();
        assert_eq!(counts, (COMET_LEN..=RING.len()).collect::<Vec<_>>());
    }

    #[test]
    fn winds_back_into_the_core_then_blinks_out() {
        assert_eq!(frame(CYCLE_STEPS - 2), glyph_of(&CORE[..1]));
        assert_eq!(lit_dots(&frame(CYCLE_STEPS - 1)), 0);
    }

    #[test]
    fn every_frame_stays_in_the_grid_and_skips_corners() {
        let corners = glyph_of(&[(0, 0), (3, 0), (0, 3), (3, 3)]);
        let corner_bits = bits(&corners);
        for step in 0..CYCLE_STEPS {
            let glyph = frame(step);
            assert_eq!(glyph.chars().count(), AURA_WIDTH);
            for (cell, corner) in bits(&glyph).iter().zip(&corner_bits) {
                assert_eq!(cell & corner, 0, "step {step} lights a corner");
            }
        }
    }

    #[test]
    fn cycle_wraps_and_changes_color_each_time() {
        let cycle = CYCLE_STEPS * TICKS_PER_STEP;
        assert_eq!(aura_ring(0).0, aura_ring(cycle).0);
        assert_ne!(aura_ring(0).1, aura_ring(cycle).1);
        assert_eq!(aura_ring(0), aura_ring(cycle * GLOW.len()));
    }

    #[test]
    fn spiral_path_moves_one_neighboring_dot_at_a_time() {
        for i in 1..SPIRAL_LEN + RING.len() {
            let (ax, ay) = spiral_dot(i - 1);
            let (bx, by) = spiral_dot(i);
            assert!(ax.abs_diff(bx) <= 1 && ay.abs_diff(by) <= 1, "{i}");
        }
    }
}
