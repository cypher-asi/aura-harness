//! The banner's font: a bitmap glyph, five pixels tall, for every
//! character a slug may hold (`a-z`, `0-9` and `-`), drawn in block
//! characters at one of two [`Size`]s.
//!
//! A cell is about twice as tall as it is wide, so both sizes keep the
//! pixels square: [`Size::Large`] spends two columns on one row per
//! pixel, [`Size::Small`] one column on half a row, two pixel rows to a
//! cell row in half blocks.

/// Pixel rows of every glyph.
const GLYPH_ROWS: usize = 5;

/// A glyph: its pixel rows, top first, `#` lit and space dark, all as
/// wide as the glyph.
type Glyph = [&'static str; GLYPH_ROWS];

/// Every glyph, by the character it draws. Letters are drawn as
/// capitals, which read better at five pixels.
const GLYPHS: [(char, Glyph); 37] = [
    ('a', [" ## ", "#  #", "####", "#  #", "#  #"]),
    ('b', ["### ", "#  #", "### ", "#  #", "### "]),
    ('c', [" ###", "#   ", "#   ", "#   ", " ###"]),
    ('d', ["### ", "#  #", "#  #", "#  #", "### "]),
    ('e', ["####", "#   ", "### ", "#   ", "####"]),
    ('f', ["####", "#   ", "### ", "#   ", "#   "]),
    ('g', [" ###", "#   ", "# ##", "#  #", " ###"]),
    ('h', ["#  #", "#  #", "####", "#  #", "#  #"]),
    ('i', ["###", " # ", " # ", " # ", "###"]),
    ('j', ["   #", "   #", "   #", "#  #", " ## "]),
    ('k', ["#  #", "# # ", "##  ", "# # ", "#  #"]),
    ('l', ["#   ", "#   ", "#   ", "#   ", "####"]),
    ('m', ["#   #", "## ##", "# # #", "#   #", "#   #"]),
    ('n', ["#  #", "## #", "# ##", "#  #", "#  #"]),
    ('o', [" ## ", "#  #", "#  #", "#  #", " ## "]),
    ('p', ["### ", "#  #", "### ", "#   ", "#   "]),
    ('q', [" ## ", "#  #", "#  #", "# ##", " ###"]),
    ('r', ["### ", "#  #", "### ", "# # ", "#  #"]),
    ('s', [" ###", "#   ", " ## ", "   #", "### "]),
    ('t', ["###", " # ", " # ", " # ", " # "]),
    ('u', ["#  #", "#  #", "#  #", "#  #", " ## "]),
    ('v', ["#   #", "#   #", "#   #", " # # ", "  #  "]),
    ('w', ["#   #", "#   #", "# # #", "## ##", "#   #"]),
    ('x', ["#   #", " # # ", "  #  ", " # # ", "#   #"]),
    ('y', ["#   #", " # # ", "  #  ", "  #  ", "  #  "]),
    ('z', ["####", "   #", "  # ", " #  ", "####"]),
    ('0', ["###", "# #", "# #", "# #", "###"]),
    ('1', [" # ", "## ", " # ", " # ", "###"]),
    ('2', ["###", "  #", "###", "#  ", "###"]),
    ('3', ["###", "  #", " ##", "  #", "###"]),
    ('4', ["# #", "# #", "###", "  #", "  #"]),
    ('5', ["###", "#  ", "###", "  #", "###"]),
    ('6', ["###", "#  ", "###", "# #", "###"]),
    ('7', ["###", "  #", "  #", "  #", "  #"]),
    ('8', ["###", "# #", "###", "# #", "###"]),
    ('9', ["###", "# #", "###", "  #", "###"]),
    ('-', ["   ", "   ", "###", "   ", "   "]),
];

/// How big the banner's letters are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Size {
    /// A pixel is `██`: two columns, one row
    Large,
    /// A pixel is half a cell: `▀`, `▄` or `█`
    Small,
}

impl Size {
    /// Both sizes, largest first: the order the banner tries them in.
    pub(super) const ALL: [Self; 2] = [Self::Large, Self::Small];

    /// Rows a line of text takes at this size.
    pub(super) const fn rows(self) -> usize {
        match self {
            Self::Large => GLYPH_ROWS,
            Self::Small => GLYPH_ROWS.div_ceil(2),
        }
    }

    /// Columns a pixel takes across.
    const fn pixel_columns(self) -> usize {
        match self {
            Self::Large => 2,
            Self::Small => 1,
        }
    }
}

/// The glyph drawing `c`, uppercase read as lowercase. `None` for a
/// character the font has no glyph for.
pub(super) fn glyph(c: char) -> Option<&'static Glyph> {
    let c = c.to_ascii_lowercase();
    GLYPHS
        .iter()
        .find(|(known, _)| *known == c)
        .map(|(_, glyph)| glyph)
}

/// Pixels across `glyph`.
fn glyph_width(glyph: &Glyph) -> usize {
    glyph[0].len()
}

/// The glyphs of `text`, or `None` when one of its characters has none.
fn glyphs(text: &str) -> Option<Vec<&'static Glyph>> {
    text.chars().map(glyph).collect()
}

/// Columns `text` takes at `size`: its glyphs with a pixel of space
/// between each two. `None` when a character has no glyph.
pub(super) fn text_width(text: &str, size: Size) -> Option<usize> {
    let glyphs = glyphs(text)?;
    let pixels: usize = glyphs.iter().map(|glyph| glyph_width(glyph)).sum();
    let gaps = glyphs.len().saturating_sub(1);
    Some((pixels + gaps) * size.pixel_columns())
}

/// `text` drawn at `size`: [`Size::rows`] rows, each
/// [`text_width`] columns wide. `None` when a character has no glyph.
pub(super) fn draw(text: &str, size: Size) -> Option<Vec<String>> {
    let glyphs = glyphs(text)?;
    let pixel_rows: Vec<Vec<bool>> = (0..GLYPH_ROWS).map(|row| pixel_row(&glyphs, row)).collect();
    Some(match size {
        Size::Large => pixel_rows.iter().map(|row| large_row(row)).collect(),
        Size::Small => pixel_rows
            .chunks(2)
            .map(|pair| small_row(pair.first(), pair.get(1)))
            .collect(),
    })
}

/// Pixel row `row` across `glyphs`, a dark pixel between each two.
fn pixel_row(glyphs: &[&Glyph], row: usize) -> Vec<bool> {
    let mut pixels = Vec::new();
    for (index, glyph) in glyphs.iter().enumerate() {
        if index > 0 {
            pixels.push(false);
        }
        pixels.extend(glyph[row].chars().map(|pixel| pixel == '#'));
    }
    pixels
}

/// A pixel row at [`Size::Large`]: `██` per lit pixel.
fn large_row(pixels: &[bool]) -> String {
    pixels
        .iter()
        .map(|&lit| if lit { "██" } else { "  " })
        .collect()
}

/// Two pixel rows at [`Size::Small`], `top` over `bottom`, in one row of
/// half blocks. A missing row is dark.
fn small_row(top: Option<&Vec<bool>>, bottom: Option<&Vec<bool>>) -> String {
    let width = top.map_or(0, Vec::len);
    let lit = |row: Option<&Vec<bool>>, x: usize| row.and_then(|row| row.get(x)) == Some(&true);
    (0..width)
        .map(|x| match (lit(top, x), lit(bottom, x)) {
            (true, true) => '█',
            (true, false) => '▀',
            (false, true) => '▄',
            (false, false) => ' ',
        })
        .collect()
}
