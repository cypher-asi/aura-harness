//! A tiny built-in pixel font for banners: every character a slug can
//! hold (`a-z`, `0-9`, `-`), 3 pixels wide and 5 tall.
//!
//! A pixel is drawn square either way a terminal cell is cut: two cells
//! wide and one tall in the [`Size::Large`] banner (`██`), or one cell
//! wide and half a cell tall in the [`Size::Small`] one (`▀`, `▄`, `█`).

/// How big a banner is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Size {
    /// Each pixel two cells wide and one tall: 5 rows.
    Large,
    /// Each pixel one cell wide and half a cell tall: 3 rows.
    Small,
}

/// Pixel rows of each glyph.
const GLYPH_ROWS: usize = 5;

/// One glyph: its pixel rows, `#` on and `.` off, each three wide.
type Glyph = [&'static str; GLYPH_ROWS];

const GLYPHS: &[(char, Glyph)] = &[
    ('a', [".#.", "#.#", "###", "#.#", "#.#"]),
    ('b', ["##.", "#.#", "##.", "#.#", "##."]),
    ('c', [".##", "#..", "#..", "#..", ".##"]),
    ('d', ["##.", "#.#", "#.#", "#.#", "##."]),
    ('e', ["###", "#..", "##.", "#..", "###"]),
    ('f', ["###", "#..", "##.", "#..", "#.."]),
    ('g', [".##", "#..", "#.#", "#.#", ".##"]),
    ('h', ["#.#", "#.#", "###", "#.#", "#.#"]),
    ('i', ["###", ".#.", ".#.", ".#.", "###"]),
    ('j', ["..#", "..#", "..#", "#.#", ".#."]),
    ('k', ["#.#", "#.#", "##.", "#.#", "#.#"]),
    ('l', ["#..", "#..", "#..", "#..", "###"]),
    ('m', ["#.#", "###", "###", "#.#", "#.#"]),
    ('n', ["##.", "#.#", "#.#", "#.#", "#.#"]),
    ('o', [".#.", "#.#", "#.#", "#.#", ".#."]),
    ('p', ["##.", "#.#", "##.", "#..", "#.."]),
    ('q', [".#.", "#.#", "#.#", "##.", ".##"]),
    ('r', ["##.", "#.#", "##.", "#.#", "#.#"]),
    ('s', [".##", "#..", ".#.", "..#", "##."]),
    ('t', ["###", ".#.", ".#.", ".#.", ".#."]),
    ('u', ["#.#", "#.#", "#.#", "#.#", "###"]),
    ('v', ["#.#", "#.#", "#.#", "#.#", ".#."]),
    ('w', ["#.#", "#.#", "###", "###", "#.#"]),
    ('x', ["#.#", "#.#", ".#.", "#.#", "#.#"]),
    ('y', ["#.#", "#.#", ".#.", ".#.", ".#."]),
    ('z', ["###", "..#", ".#.", "#..", "###"]),
    ('0', ["###", "#.#", "#.#", "#.#", "###"]),
    ('1', [".#.", "##.", ".#.", ".#.", "###"]),
    ('2', ["##.", "..#", ".#.", "#..", "###"]),
    ('3', ["##.", "..#", ".#.", "..#", "##."]),
    ('4', ["#.#", "#.#", "###", "..#", "..#"]),
    ('5', ["###", "#..", "##.", "..#", "##."]),
    ('6', [".##", "#..", "###", "#.#", "###"]),
    ('7', ["###", "..#", "..#", ".#.", ".#."]),
    ('8', ["###", "#.#", "###", "#.#", "###"]),
    ('9', ["###", "#.#", "###", "..#", "##."]),
    ('-', ["...", "...", "###", "...", "..."]),
];

/// Pixels across each glyph.
const GLYPH_WIDTH: usize = 3;

impl Size {
    /// Cells a pixel takes across.
    const fn pixel_cells(self) -> usize {
        match self {
            Self::Large => 2,
            Self::Small => 1,
        }
    }

    /// Rows a banner of this size takes.
    pub(super) const fn rows(self) -> usize {
        match self {
            Self::Large => GLYPH_ROWS,
            Self::Small => GLYPH_ROWS.div_ceil(2),
        }
    }

    /// Cells across `text` drawn at this size: its glyphs one pixel
    /// apart.
    pub(super) fn width(self, text: &str) -> usize {
        let glyphs = text.chars().count();
        let pixels = (glyphs * (GLYPH_WIDTH + 1)).saturating_sub(1);
        pixels * self.pixel_cells()
    }
}

/// The glyph of `c`, in either case; `None` for one the font lacks.
fn glyph(c: char) -> Option<&'static Glyph> {
    let c = c.to_ascii_lowercase();
    GLYPHS
        .iter()
        .find(|(known, _)| *known == c)
        .map(|(_, glyph)| glyph)
}

/// Whether the font draws every character of `text`.
pub(super) fn draws(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| glyph(c).is_some())
}

/// `text` drawn at `size`, a string per row, each [`Size::width`] cells
/// wide. `None` when the font lacks one of its characters.
pub(super) fn render(text: &str, size: Size) -> Option<Vec<String>> {
    let glyphs: Vec<&Glyph> = text.chars().map(glyph).collect::<Option<_>>()?;
    let pixel_row = |row: usize| -> Vec<bool> {
        let mut pixels = Vec::new();
        for (at, glyph) in glyphs.iter().enumerate() {
            if at > 0 {
                pixels.push(false);
            }
            pixels.extend(glyph[row].chars().map(|p| p == '#'));
        }
        pixels
    };
    let rows = match size {
        Size::Large => (0..GLYPH_ROWS)
            .map(|row| {
                pixel_row(row)
                    .into_iter()
                    .map(|on| if on { "██" } else { "  " })
                    .collect()
            })
            .collect(),
        Size::Small => (0..GLYPH_ROWS)
            .step_by(2)
            .map(|row| {
                let top = pixel_row(row);
                let bottom = (row + 1 < GLYPH_ROWS).then(|| pixel_row(row + 1));
                top.iter()
                    .enumerate()
                    .map(|(at, &up)| half_block(up, bottom.as_ref().is_some_and(|b| b[at])))
                    .collect()
            })
            .collect(),
    };
    Some(rows)
}

/// The cell drawing a pixel `up` over a pixel `down`.
const fn half_block(up: bool, down: bool) -> char {
    match (up, down) {
        (true, true) => '█',
        (true, false) => '▀',
        (false, true) => '▄',
        (false, false) => ' ',
    }
}
