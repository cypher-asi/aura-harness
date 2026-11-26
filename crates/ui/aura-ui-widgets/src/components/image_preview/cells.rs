//! Laying an image's pixels onto terminal cells: half-blocks or octants.

use ratatui::{
    style::{Color, Style},
    symbols::pixel::OCTANTS,
    text::{Line, Span},
};

/// Squared color distance under which a cell's pixels count as one color.
const FLAT: u32 = 24 * 24;

/// A `w`×`h` image, whose pixel at `(x, y)` is `color(x, y)` (`None` when
/// transparent), as `▀` cells: 1×2 pixels each.
pub(super) fn half_block_lines(
    w: u32,
    h: u32,
    color: impl Fn(u32, u32) -> Option<[u8; 3]>,
) -> Vec<Line<'static>> {
    let color = |x, y| color(x, y).map(rgb);
    (0..h.div_ceil(2))
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..w)
                .map(|x| match (color(x, row * 2), color(x, row * 2 + 1)) {
                    (Some(top), Some(bottom)) => {
                        Span::styled("▀", Style::default().fg(top).bg(bottom))
                    }
                    (Some(top), None) => Span::styled("▀", Style::default().fg(top)),
                    (None, Some(bottom)) => Span::styled("▄", Style::default().fg(bottom)),
                    (None, None) => Span::raw(" "),
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// A `w`×`h` image, as for [`half_block_lines`], in octant cells: 2×4
/// pixels each.
pub(super) fn octant_lines(
    w: u32,
    h: u32,
    color: impl Fn(u32, u32) -> Option<[u8; 3]>,
) -> Vec<Line<'static>> {
    (0..h.div_ceil(4))
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..w.div_ceil(2))
                .map(|col| {
                    let mut cell = [None; 8];
                    for (i, pixel) in (0u32..).zip(cell.iter_mut()) {
                        *pixel = color(col * 2 + i % 2, row * 4 + i / 2);
                    }
                    octant_cell(&cell)
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// One cell of 2×4 pixels, in row-major order with `None` transparent,
/// drawn in the two colors that best split them: the octant character
/// whose set pixels are the foreground group, over the background group.
/// Transparent pixels are left as the terminal's background, and the
/// opaque ones drawn in their average.
pub(super) fn octant_cell(pixels: &[Option<[u8; 3]>; 8]) -> Span<'static> {
    let opaque: Vec<(usize, [u8; 3])> = pixels
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.map(|c| (i, c)))
        .collect();
    let Some(&(_, first)) = opaque.first() else {
        return Span::raw(" ");
    };
    if opaque.len() < pixels.len() {
        let bits = mask(opaque.iter().map(|&(i, _)| i));
        let fg = mean(opaque.iter().map(|&(_, c)| c));
        return Span::styled(
            String::from(OCTANTS[usize::from(bits)]),
            Style::default().fg(rgb(fg)),
        );
    }
    let (a, b, widest) = most_different(&opaque, first);
    if widest < FLAT {
        let bg = mean(opaque.iter().map(|&(_, c)| c));
        return Span::styled(" ", Style::default().bg(rgb(bg)));
    }
    let (bits, a, b) = split_colors(&opaque, a, b);
    Span::styled(
        String::from(OCTANTS[usize::from(bits)]),
        Style::default().fg(rgb(a)).bg(rgb(b)),
    )
}

/// The two most different of `opaque`'s colors and their distance,
/// `(first, first, 0)` when none differ.
fn most_different(opaque: &[(usize, [u8; 3])], first: [u8; 3]) -> ([u8; 3], [u8; 3], u32) {
    let (mut a, mut b, mut widest) = (first, first, 0);
    for (n, &(_, p)) in opaque.iter().enumerate() {
        for &(_, q) in &opaque[n + 1..] {
            let d = distance(p, q);
            if d > widest {
                (a, b, widest) = (p, q, d);
            }
        }
    }
    (a, b, widest)
}

/// Split `opaque` into the group nearer `a` (the set bits) and the one
/// nearer `b`, seeded by the two most different pixels; one refinement
/// pass then moves each group's center to its average. Returns the mask
/// and the two groups' colors.
fn split_colors(
    opaque: &[(usize, [u8; 3])],
    mut a: [u8; 3],
    mut b: [u8; 3],
) -> (u8, [u8; 3], [u8; 3]) {
    let mut bits = 0u8;
    for _ in 0..2 {
        bits = mask(
            opaque
                .iter()
                .filter(|&&(_, c)| distance(c, a) <= distance(c, b))
                .map(|&(i, _)| i),
        );
        let group = |set: bool| {
            opaque
                .iter()
                .filter(move |&&(i, _)| (bits & (1 << i) != 0) == set)
                .map(|&(_, c)| c)
        };
        if bits == 0 || bits == u8::MAX {
            break;
        }
        (a, b) = (mean(group(true)), mean(group(false)));
    }
    (bits, a, b)
}

/// The octant pattern with pixels `indices` set.
fn mask(indices: impl Iterator<Item = usize>) -> u8 {
    indices.fold(0, |bits, i| bits | (1 << i))
}

/// Squared distance between two colors.
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(&x, y)| u32::from(x.abs_diff(y)).pow(2))
        .sum()
}

/// Average of `colors`; black when there are none.
fn mean(colors: impl Iterator<Item = [u8; 3]>) -> [u8; 3] {
    let (mut sum, mut n) = ([0u32; 3], 0u32);
    for c in colors {
        for (s, v) in sum.iter_mut().zip(c) {
            *s += u32::from(v);
        }
        n += 1;
    }
    sum.map(|s| u8::try_from(s / n.max(1)).unwrap_or(u8::MAX))
}

/// A ratatui color from RGB.
const fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::Rgb(r, g, b)
}
