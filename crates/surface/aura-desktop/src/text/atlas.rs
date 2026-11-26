//! The glyph and shape atlas.

use cosmic_text::CacheKey;
use rustc_hash::FxHashMap;
use std::ops::Range;

/// Where a glyph or a shape is in the atlas, and where it goes relative
/// to the point it was drawn for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub u: u32,
    pub v: u32,
    pub w: u32,
    pub h: u32,
    /// Offset of the top-left pixel from the anchor
    pub dx: i32,
    pub dy: i32,
    /// Whether the pixels carry their own colors
    pub image: bool,
}

/// Pixels to put in the atlas.
pub struct Bitmap {
    pub dx: i32,
    pub dy: i32,
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

pub enum Pixels {
    /// One byte of coverage per pixel
    Mask(Vec<u8>),
    /// RGBA per pixel, not premultiplied
    Rgba(Vec<u8>),
}

/// A shape drawn into the atlas: which one, at what size, and how far
/// across and down a pixel it starts, in 256ths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapeKey {
    pub shape: u32,
    pub width: u32,
    pub height: u32,
    pub phase: [u8; 2],
}

/// Rows of an [`Atlas`] copied out for a texture of it on another
/// thread.
pub struct AtlasRows {
    /// The atlas's size when they were copied
    pub size: (u32, u32),
    /// The first row copied
    pub start: u32,
    /// RGBA rows from `start` down
    pub pixels: Vec<u8>,
}

impl AtlasRows {
    /// The rows copied.
    pub fn rows(&self) -> Range<u32> {
        let stride = self.size.0 as usize * 4;
        let count = self.pixels.len().checked_div(stride).unwrap_or(0);
        self.start..self.start + u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Whether every row of the atlas was copied.
    pub fn whole(&self) -> bool {
        self.rows() == (0..self.size.1)
    }
}

struct Shelf {
    y: u32,
    height: u32,
    next_x: u32,
}

/// One RGBA texture holding every glyph and drawn shape in use, packed
/// in shelves. Coverage masks are white with the coverage as alpha. It
/// grows downwards until [`Self::set_limit`], then starts over.
pub struct Atlas {
    width: u32,
    height: u32,
    limit: u32,
    pixels: Vec<u8>,
    shelves: Vec<Shelf>,
    /// First row below the shelves
    used: u32,
    pub(super) glyphs: FxHashMap<CacheKey, Option<Slot>>,
    shapes: FxHashMap<ShapeKey, Option<Slot>>,
    generation: u64,
    dirty: Option<Range<u32>>,
}

impl Atlas {
    pub const WIDTH: u32 = 1024;
    const START_HEIGHT: u32 = 512;
    const DEFAULT_LIMIT: u32 = 4096;

    pub(super) fn new() -> Self {
        Self {
            width: Self::WIDTH,
            height: Self::START_HEIGHT,
            limit: Self::DEFAULT_LIMIT,
            pixels: vec![0; (Self::WIDTH * Self::START_HEIGHT * 4) as usize],
            shelves: Vec::new(),
            used: 0,
            glyphs: FxHashMap::default(),
            shapes: FxHashMap::default(),
            generation: 0,
            dirty: None,
        }
    }

    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// RGBA rows, top to bottom.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Changes whenever the atlas started over, which leaves every
    /// [`Slot`] handed out before pointing at other pixels.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The rows written since this was last called.
    pub fn take_dirty(&mut self) -> Option<Range<u32>> {
        self.dirty.take()
    }

    /// The rows written since the last take, copied out for a texture of
    /// the atlas that is `known` in size: every row when the atlas is no
    /// longer that size, since such a texture is made anew.
    pub fn take_rows(&mut self, known: Option<(u32, u32)>) -> AtlasRows {
        let dirty = self.take_dirty();
        let rows = if known == Some(self.size()) {
            dirty.unwrap_or(0..0)
        } else {
            0..self.height
        };
        self.copy_rows(rows)
    }

    /// `newer`, the rows taken for a frame, together with `older`, those
    /// of a frame it replaces before they reached the texture, both read
    /// again as the atlas has them now.
    pub fn merge_rows(&self, newer: AtlasRows, older: &AtlasRows) -> AtlasRows {
        // The atlas changed size since `older` only if `newer` has every
        // row.
        let (a, b) = (newer.rows(), older.rows());
        if newer.whole() || b.is_empty() {
            return newer;
        }
        let rows = if a.is_empty() {
            b
        } else {
            a.start.min(b.start)..a.end.max(b.end)
        };
        self.copy_rows(rows)
    }

    pub(super) fn copy_rows(&self, rows: Range<u32>) -> AtlasRows {
        let (start, end) = (rows.start.min(self.height), rows.end.min(self.height));
        let stride = self.width as usize * 4;
        AtlasRows {
            size: self.size(),
            start,
            pixels: self.pixels[start as usize * stride..end.max(start) as usize * stride].to_vec(),
        }
    }

    /// The tallest the atlas may grow: what the GPU takes as a texture.
    pub fn set_limit(&mut self, height: u32) {
        self.limit = height.max(Self::START_HEIGHT);
        if self.height > self.limit {
            self.height = self.limit;
            self.pixels
                .truncate((self.width * self.height * 4) as usize);
            self.clear();
        }
    }

    /// Drop everything: the font size changed, or the atlas is full.
    pub fn clear(&mut self) {
        self.shelves.clear();
        self.used = 0;
        self.glyphs.clear();
        self.shapes.clear();
        self.pixels.fill(0);
        self.generation += 1;
        self.dirty = Some(0..self.height);
    }

    /// The slot of the shape `key`, drawn by `draw` the first time.
    /// `None` when the shape has no pixels.
    pub fn shape(&mut self, key: ShapeKey, draw: impl FnOnce() -> Option<Bitmap>) -> Option<Slot> {
        if let Some(slot) = self.shapes.get(&key) {
            return *slot;
        }
        let slot = draw().and_then(|bitmap| self.store(&bitmap));
        self.shapes.insert(key, slot);
        slot
    }

    /// Copy `bitmap` in, starting the atlas over when it is full.
    pub(super) fn store(&mut self, bitmap: &Bitmap) -> Option<Slot> {
        let (w, h) = (bitmap.width, bitmap.height);
        if w == 0 || h == 0 || w > self.width || h > self.limit {
            return None;
        }
        let (u, v) = match self.place(w, h) {
            Some(at) => at,
            None => {
                self.clear();
                self.place(w, h)?
            }
        };
        for row in 0..h {
            let start = (((v + row) * self.width + u) * 4) as usize;
            let out = &mut self.pixels[start..start + (w * 4) as usize];
            match &bitmap.pixels {
                Pixels::Mask(mask) => {
                    let mask = &mask[(row * w) as usize..((row + 1) * w) as usize];
                    for (pixel, coverage) in out.chunks_exact_mut(4).zip(mask) {
                        pixel.copy_from_slice(&[255, 255, 255, *coverage]);
                    }
                }
                Pixels::Rgba(rgba) => {
                    out.copy_from_slice(
                        &rgba[(row * w * 4) as usize..((row + 1) * w * 4) as usize],
                    );
                }
            }
        }
        let rows = v..v + h;
        self.dirty = Some(match self.dirty.take() {
            Some(dirty) => dirty.start.min(rows.start)..dirty.end.max(rows.end),
            None => rows,
        });
        Some(Slot {
            u,
            v,
            w,
            h,
            dx: bitmap.dx,
            dy: bitmap.dy,
            image: matches!(bitmap.pixels, Pixels::Rgba(_)),
        })
    }

    /// Room for `w` × `h` pixels: on a shelf about as tall, else on a new
    /// shelf, growing the atlas for it when that is allowed.
    pub(super) fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let width = self.width;
        let fits = |shelf: &Shelf| {
            shelf.height >= h && shelf.height <= h + h / 4 + 2 && shelf.next_x + w <= width
        };
        if let Some(shelf) = self.shelves.iter_mut().find(|shelf| fits(shelf)) {
            let at = (shelf.next_x, shelf.y);
            shelf.next_x += w;
            return Some(at);
        }
        // Shelf heights in steps of four, so glyphs of one size share.
        let height = h.next_multiple_of(4);
        while self.used + height > self.height {
            if self.height >= self.limit {
                return None;
            }
            self.height = (self.height * 2).min(self.limit);
            self.pixels
                .resize((self.width * self.height * 4) as usize, 0);
        }
        let y = self.used;
        self.used += height;
        self.shelves.push(Shelf {
            y,
            height,
            next_x: w,
        });
        Some((0, y))
    }
}
