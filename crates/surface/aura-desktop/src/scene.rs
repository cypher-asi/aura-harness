//! The display list: everything on screen is a [`Quad`], a rectangle on
//! whole pixels that is one color, or takes its shape or its pixels from
//! the atlas ([`crate::text::Atlas`]). A [`Scene`] is the quads of one
//! frame, back to front; the CPU rasterizer and the GPU draw the same
//! list.
//!
//! A row of the grid costs the most to turn into quads (its text is
//! shaped), so [`RowCache`] keeps the quads of each row by what the row
//! holds. A row that scrolled is the same quads at another height, and
//! a row that comes back is not shaped again. Rows are kept in pieces,
//! a panel's stretch of the row each, so a panel that scrolls leaves the
//! pieces of the panels beside it as they were.

use crate::geom::{rgba, Color};
use aura_ui_terminal::ProseRow;
use ratatui::buffer::Cell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

/// What fills a quad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// The quad's color
    Solid,
    /// The quad's color, as opaque as the atlas is under it: glyphs and
    /// drawn shapes
    Mask,
    /// The atlas's own colors, faded by the color's alpha: color glyphs
    Image,
}

/// A rectangle of pixels. `color` is RGBA, not premultiplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quad {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub color: [u8; 4],
    pub fill: Fill,
    /// Top-left corner of the quad's pixels in the atlas, unless solid
    pub u: u32,
    pub v: u32,
}

impl Quad {
    /// The pixels whose centers are inside the rectangle from `(x0, y0)`
    /// to `(x1, y1)`; `None` when there are none.
    pub fn solid(x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) -> Option<Self> {
        let (left, right) = (snap(x0), snap(x1));
        let (top, bottom) = (snap(y0), snap(y1));
        (right > left && bottom > top && color[3] > 0).then(|| Self {
            x: left,
            y: top,
            w: right.abs_diff(left),
            h: bottom.abs_diff(top),
            color,
            fill: Fill::Solid,
            u: 0,
            v: 0,
        })
    }

    /// The first column right of the quad.
    #[cfg(test)]
    pub const fn right(&self) -> i32 {
        self.x.saturating_add_unsigned(self.w)
    }

    /// The first row below the quad.
    #[cfg(test)]
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add_unsigned(self.h)
    }

    /// The quad `rows` pixels lower.
    #[must_use]
    pub const fn lowered(mut self, rows: i32) -> Self {
        self.y += rows;
        self
    }

    /// The part of the quad from pixel row `top` to before `bottom`, and
    /// the atlas pixels under that part; `None` when it is all outside.
    #[must_use]
    pub fn clipped(mut self, top: i32, bottom: i32) -> Option<Self> {
        let end = self.y.saturating_add_unsigned(self.h).min(bottom);
        let start = self.y.max(top);
        if end <= start {
            return None;
        }
        if self.fill != Fill::Solid {
            self.v += start.abs_diff(self.y);
        }
        self.y = start;
        self.h = end.abs_diff(start);
        Some(self)
    }
}

/// A rectangle from its corner and its size, in `color`.
pub fn fill(out: &mut Vec<Quad>, x: f32, y: f32, width: f32, height: f32, color: Color) {
    out.extend(Quad::solid(x, y, x + width, y + height, rgba(color)));
}

/// The first pixel whose center is at or after `edge`.
#[allow(clippy::cast_possible_truncation)]
fn snap(edge: f32) -> i32 {
    (edge + 0.5).floor() as i32
}

/// One frame: the quads over a background, back to front.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub background: [u8; 4],
    pub quads: Vec<Quad>,
}

impl Scene {
    /// Start a frame of this size over `background`, keeping the list's
    /// allocation.
    pub fn begin(&mut self, width: u32, height: u32, background: [u8; 4]) {
        self.width = width;
        self.height = height;
        self.background = background;
        self.quads.clear();
    }
}

/// The parts of a prose row its quads depend on: everything but where on
/// the screen the row is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Flow {
    x: u16,
    width: u16,
    indent: u16,
    code: Vec<Range<u16>>,
}

impl Flow {
    fn of(prose: &ProseRow) -> Self {
        Self {
            x: prose.x,
            width: prose.width,
            indent: prose.indent,
            code: prose.code.clone(),
        }
    }

    fn is(&self, prose: &ProseRow) -> bool {
        (self.x, self.width, self.indent) == (prose.x, prose.width, prose.indent)
            && self.code == prose.code
    }
}

/// The quads of one thing drawn, in two layers: every `under` quad on
/// screen is painted before any `over` quad, so text is never covered by
/// the background of the row below or the fill of a box drawn after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layers {
    /// Backgrounds, lines and drawn shapes
    pub under: Vec<Quad>,
    /// Glyphs
    pub over: Vec<Quad>,
}

impl Layers {
    pub fn clear(&mut self) {
        self.under.clear();
        self.over.clear();
    }

    /// Add `other`, `rows` pixels lower.
    pub fn extend_lowered(&mut self, other: &Self, rows: i32) {
        let lowered = |quad: &Quad| quad.lowered(rows);
        self.under.extend(other.under.iter().map(lowered));
        self.over.extend(other.over.iter().map(lowered));
    }

    /// Add `other`, `rows` pixels lower, cut to the pixel rows `clip`.
    pub fn extend_clipped(&mut self, other: &Self, rows: i32, clip: Range<i32>) {
        let lowered = |quad: &Quad| quad.lowered(rows).clipped(clip.start, clip.end);
        self.under.extend(other.under.iter().filter_map(lowered));
        self.over.extend(other.over.iter().filter_map(lowered));
    }
}

struct CachedRow {
    /// The column the piece starts at
    start: usize,
    cells: Vec<Cell>,
    flow: Option<Flow>,
    /// With the row's top at 0
    quads: Layers,
    /// The frame that last drew the row
    used: u64,
}

impl CachedRow {
    fn holds(&self, start: usize, cells: &[Cell], prose: Option<&ProseRow>) -> bool {
        let same_flow = match (&self.flow, prose) {
            (None, None) => true,
            (Some(flow), Some(prose)) => flow.is(prose),
            _ => false,
        };
        same_flow && self.start == start && self.cells == cells
    }
}

/// The quads of pieces of grid rows, by where a piece starts and what it
/// holds. Everything else a piece's quads depend on (the font size, the
/// colors, where the grid starts, the atlas) is the same for every
/// piece, so the owner calls [`Self::clear`] when one of those changes.
pub struct RowCache {
    rows: HashMap<u64, CachedRow>,
    frame: u64,
    /// Pieces kept; the ones not drawn for longest go first
    limit: usize,
    built: usize,
}

impl Default for RowCache {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl RowCache {
    pub fn new(limit: usize) -> Self {
        Self {
            rows: HashMap::new(),
            frame: 0,
            limit: limit.max(1),
            built: 0,
        }
    }

    /// The quads of a piece of a row from column `start`, holding `cells`
    /// and `prose`, with the row's top at 0: `build` makes them the first
    /// time.
    pub fn row(
        &mut self,
        start: usize,
        cells: &[Cell],
        prose: Option<&ProseRow>,
        build: impl FnOnce(&mut Layers),
    ) -> &Layers {
        let mut hasher = DefaultHasher::new();
        start.hash(&mut hasher);
        cells.hash(&mut hasher);
        if let Some(prose) = prose {
            (prose.x, prose.width, prose.indent, &prose.code).hash(&mut hasher);
        }
        let key = hasher.finish();

        let frame = self.frame;
        let hit = self
            .rows
            .get(&key)
            .is_some_and(|row| row.holds(start, cells, prose));
        if !hit {
            let mut quads = Layers::default();
            build(&mut quads);
            self.built += 1;
            self.rows.insert(
                key,
                CachedRow {
                    start,
                    cells: cells.to_vec(),
                    flow: prose.map(Flow::of),
                    quads,
                    used: frame,
                },
            );
        }
        let row = self.rows.get_mut(&key).expect("just looked up or stored");
        row.used = frame;
        &row.quads
    }

    /// End the frame: forget the rows not drawn for longest when there
    /// are too many, and return how many rows this frame had to build.
    pub fn finish_frame(&mut self) -> usize {
        if self.rows.len() > self.limit {
            let mut used: Vec<u64> = self.rows.values().map(|row| row.used).collect();
            used.sort_unstable();
            // Keep the newer three quarters, and always this frame's rows.
            let keep = (self.limit * 3 / 4).max(1);
            let cutoff = used[used.len() - keep].min(self.frame);
            self.rows.retain(|_, row| row.used >= cutoff);
        }
        self.frame += 1;
        std::mem::take(&mut self.built)
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    const RED: [u8; 4] = [255, 0, 0, 255];

    fn row(text: &str) -> Vec<Cell> {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 1));
        buffer.set_string(0, 0, text, Style::default());
        buffer.content
    }

    fn mark(x: u8) -> Quad {
        let x = f32::from(x);
        Quad::solid(x, 0.0, x + 1.0, 1.0, RED).expect("a pixel")
    }

    #[test]
    fn solid_quads_cover_the_pixels_whose_centers_are_inside() {
        let quad = Quad::solid(1.4, 2.5, 3.6, 4.49, RED).expect("covers pixels");
        assert_eq!((quad.x, quad.y, quad.w, quad.h), (1, 3, 3, 1));
        assert!(Quad::solid(1.6, 0.0, 2.4, 1.0, RED).is_none(), "no center");
        assert!(Quad::solid(0.0, 0.0, 4.0, 4.0, [1, 2, 3, 0]).is_none());
        assert_eq!(quad.lowered(18).y, 21);

        let mut layers = Layers::default();
        layers.over.push(quad);
        let mut frame = Layers::default();
        frame.extend_lowered(&layers, 10);
        assert_eq!((frame.under.len(), frame.over[0].y), (0, 13));
    }

    #[test]
    fn clipping_keeps_the_atlas_pixels_under_what_is_left() {
        let glyph = Quad {
            fill: Fill::Mask,
            u: 40,
            v: 100,
            ..Quad::solid(0.0, 10.0, 4.0, 20.0, RED).expect("a quad")
        };
        let top = glyph.clipped(14, 30).expect("its lower part");
        assert_eq!((top.y, top.h, top.u, top.v), (14, 6, 40, 104));
        let bottom = glyph.clipped(0, 12).expect("its upper part");
        assert_eq!((bottom.y, bottom.h, bottom.v), (10, 2, 100));
        assert_eq!(glyph.clipped(20, 40), None);

        let solid = Quad::solid(0.0, 10.0, 4.0, 20.0, RED).expect("a quad");
        assert_eq!(
            solid.clipped(15, 30).map(|q| (q.y, q.h, q.v)),
            Some((15, 5, 0))
        );

        let mut layers = Layers::default();
        layers.under.push(solid);
        layers.over.push(glyph);
        let mut frame = Layers::default();
        frame.extend_clipped(&layers, -8, 4..10);
        assert_eq!(frame.under.len(), 1);
        assert_eq!(
            (frame.over[0].y, frame.over[0].h, frame.over[0].v),
            (4, 6, 102)
        );
    }

    #[test]
    fn a_row_is_built_once_for_what_it_holds() {
        let mut cache = RowCache::default();
        let (a, b) = (row("hello"), row("world"));
        let mut builds = 0;
        for cells in [&a, &b, &a, &a] {
            let quads = cache.row(0, cells, None, |out| {
                builds += 1;
                out.under.push(mark(7));
            });
            assert_eq!(quads.under, [mark(7)]);
            assert!(quads.over.is_empty());
        }
        assert_eq!(builds, 2);
        assert_eq!(cache.finish_frame(), 2);

        cache.row(0, &a, None, |_| panic!("kept from the last frame"));
        assert_eq!(cache.finish_frame(), 0);

        let mut built = false;
        cache.row(4, &a, None, |_| built = true);
        assert!(built, "the same cells further along are other quads");
    }

    #[test]
    fn rows_not_drawn_for_longest_are_forgotten() {
        let mut cache = RowCache::new(8);
        for frame in 0..40 {
            cache.row(0, &row(&format!("row {frame}")), None, |_| {});
            cache.row(0, &row("always"), None, |_| {});
            cache.finish_frame();
        }
        assert!(cache.len() <= 8, "{} rows kept", cache.len());
        cache.row(0, &row("always"), None, |_| panic!("drawn every frame"));
        cache.row(0, &row("row 39"), None, |_| panic!("drawn last frame"));
        let mut rebuilt = false;
        cache.row(0, &row("row 0"), None, |_| rebuilt = true);
        assert!(rebuilt, "the oldest row was dropped");
    }

    #[test]
    fn clearing_forgets_every_row() {
        let mut cache = RowCache::default();
        cache.row(0, &row("hello"), None, |_| {});
        cache.clear();
        let mut rebuilt = false;
        cache.row(0, &row("hello"), None, |_| rebuilt = true);
        assert!(rebuilt);
    }
}
