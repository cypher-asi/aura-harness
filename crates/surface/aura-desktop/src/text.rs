//! Text for the bare renderer: the fonts, shaped runs, and the atlas the
//! glyphs are drawn into.
//!
//! Shaping and glyph rasterization are cosmic-text's, the library iced
//! draws text with, so a run comes out with the same glyphs at the same
//! places. Shaped runs and measured widths are kept in bounded
//! least-recently-used stores: a run is shaped once however many frames
//! show it. Each glyph is rasterized once per size and sub-pixel position
//! into the [`Atlas`], which drawn shapes share.

use crate::geom::Point;
use crate::scene::{Fill, Quad};
use crate::theme;
use cosmic_text::{
    fontdb, Attrs, Buffer, CacheKey, Family as FontFamily, FontSystem, LayoutGlyph, Metrics, Style,
    SwashCache, SwashContent, Weight, Wrap,
};
use ratatui::style::Modifier;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

pub use cosmic_text::Shaping;

/// Shaped runs kept.
const MAX_RUNS: usize = 4096;
/// Measured widths kept.
const MAX_WIDTHS: usize = 8192;
/// Line height prose is measured at, in ems. A width doesn't depend on
/// it; it is the one iced measures with.
const LINE_EM: f32 = 1.3;
/// Line height of a label on a title bar or the tab bar, in ems: what
/// text drawn on an iced canvas gets unless it names another.
pub const LABEL_LINE_EM: f32 = 1.2;

/// Which of the two families a run is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// The grid's monospace face
    Mono,
    /// The proportional face of the chat's prose
    Sans,
}

/// A font of one of the two families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Face {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

impl Face {
    pub const fn mono(bold: bool) -> Self {
        Self {
            family: Family::Mono,
            bold,
            italic: false,
        }
    }

    /// The family's name and regular weight, as [`crate::theme`] names
    /// them for iced.
    fn attrs(self) -> Attrs<'static> {
        let font = match (self.family, self.bold) {
            (Family::Mono, false) => theme::mono(),
            (Family::Mono, true) => theme::mono_bold(),
            (Family::Sans, false) => theme::sans(),
            (Family::Sans, true) => theme::sans_bold(),
        };
        let family = match font.family {
            iced::font::Family::Name(name) => FontFamily::Name(name),
            _ => FontFamily::Monospace,
        };
        let weight = match font.weight {
            iced::font::Weight::Medium => Weight::MEDIUM,
            iced::font::Weight::Bold => Weight::BOLD,
            _ => Weight::NORMAL,
        };
        let style = if self.italic {
            Style::Italic
        } else {
            Style::Normal
        };
        Attrs::new().family(family).weight(weight).style(style)
    }
}

/// Where the point a label is written at is on its line, sideways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A line of text to write.
#[derive(Debug, Clone, Copy)]
pub struct Label<'a> {
    pub text: &'a str,
    pub face: Face,
    pub size: f32,
    pub line_height: f32,
    pub shaping: Shaping,
    pub align: Align,
    /// Whether the point is the middle of the line's height, not its top
    pub middle: bool,
    pub color: [u8; 4],
    /// Whether the line is on a grid row painted with its top at 0, to
    /// be lowered to where the row is
    pub on_row: bool,
}

/// One line of shaped text.
pub struct Run {
    pub glyphs: Vec<LayoutGlyph>,
    /// From the top of the line to its baseline
    pub baseline: f32,
    pub width: f32,
    pub height: f32,
}

/// A least-recently-used store. Entries are found by a hash of their key
/// and checked against it, so a lookup allocates nothing.
struct Lru<K, V> {
    entries: HashMap<u64, (K, V, u64)>,
    clock: u64,
    limit: usize,
}

impl<K, V> Lru<K, V> {
    fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            limit: limit.max(2),
        }
    }

    fn get(&mut self, hash: u64, is: impl FnOnce(&K) -> bool) -> Option<&V> {
        self.clock += 1;
        let (key, value, used) = self.entries.get_mut(&hash)?;
        if !is(key) {
            return None;
        }
        *used = self.clock;
        Some(value)
    }

    /// Store `value`, dropping the half used longest ago when full.
    fn put(&mut self, hash: u64, key: K, value: V) {
        if self.entries.len() >= self.limit {
            let mut used: Vec<u64> = self.entries.values().map(|entry| entry.2).collect();
            used.sort_unstable();
            let cutoff = used[used.len() / 2];
            self.entries.retain(|_, entry| entry.2 > cutoff);
        }
        self.clock += 1;
        self.entries.insert(hash, (key, value, self.clock));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(PartialEq)]
struct RunKey {
    text: Box<str>,
    face: Face,
    size: u32,
    line_height: u32,
    advanced: bool,
}

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
    glyphs: HashMap<CacheKey, Option<Slot>>,
    shapes: HashMap<ShapeKey, Option<Slot>>,
    generation: u64,
    dirty: Option<Range<u32>>,
}

impl Atlas {
    pub const WIDTH: u32 = 1024;
    const START_HEIGHT: u32 = 512;
    const DEFAULT_LIMIT: u32 = 4096;

    fn new() -> Self {
        Self {
            width: Self::WIDTH,
            height: Self::START_HEIGHT,
            limit: Self::DEFAULT_LIMIT,
            pixels: vec![0; (Self::WIDTH * Self::START_HEIGHT * 4) as usize],
            shelves: Vec::new(),
            used: 0,
            glyphs: HashMap::new(),
            shapes: HashMap::new(),
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
    fn store(&mut self, bitmap: &Bitmap) -> Option<Slot> {
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
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
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

/// The fonts, what was shaped and measured with them, and the atlas.
pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    runs: Lru<RunKey, Arc<Run>>,
    widths: Lru<(Box<str>, Face, u32), f32>,
    atlas: Atlas,
}

impl TextSystem {
    /// The system's fonts and the bundled ones.
    pub fn new() -> Self {
        let bundled = theme::font_files()
            .into_iter()
            .map(|font| fontdb::Source::Binary(Arc::new(font.into_owned())));
        Self {
            fonts: FontSystem::new_with_fonts(bundled),
            swash: SwashCache::new(),
            runs: Lru::new(MAX_RUNS),
            widths: Lru::new(MAX_WIDTHS),
            atlas: Atlas::new(),
        }
    }

    pub const fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub const fn atlas_mut(&mut self) -> &mut Atlas {
        &mut self.atlas
    }

    /// `text` as one line in `face` at `size`, in a line `line_height`
    /// tall.
    pub fn shape(
        &mut self,
        text: &str,
        face: Face,
        size: f32,
        line_height: f32,
        shaping: Shaping,
    ) -> Arc<Run> {
        let advanced = matches!(shaping, Shaping::Advanced);
        let (size_bits, line_bits) = (size.to_bits(), line_height.to_bits());
        let hash = hash_of(&(text, face, size_bits, line_bits, advanced));
        let is = |key: &RunKey| {
            *key.text == *text
                && (key.face, key.size, key.line_height, key.advanced)
                    == (face, size_bits, line_bits, advanced)
        };
        if let Some(run) = self.runs.get(hash, is) {
            return Arc::clone(run);
        }
        let buffer = self.layout(text, face, size, line_height, shaping, None);
        let mut run = Run {
            glyphs: Vec::new(),
            baseline: 0.0,
            width: 0.0,
            height: 0.0,
        };
        for line in buffer.layout_runs() {
            if run.glyphs.is_empty() {
                run.baseline = line.line_y;
            }
            run.glyphs.extend_from_slice(line.glyphs);
            run.width = run.width.max(line.line_w);
            run.height += line.line_height;
        }
        let run = Arc::new(run);
        let key = RunKey {
            text: text.into(),
            face,
            size: size_bits,
            line_height: line_bits,
            advanced,
        };
        self.runs.put(hash, key, Arc::clone(&run));
        run
    }

    /// How wide a run of `text` is in `face` at `size`, trailing spaces
    /// included.
    pub fn run_width(&mut self, text: &str, face: Face, size: f32) -> f32 {
        // A closing bar keeps trailing spaces in the measured line.
        self.line_width(&format!("{text}|"), face, size) - self.line_width("|", face, size)
    }

    /// How wide `text` is laid out on one line.
    fn line_width(&mut self, text: &str, face: Face, size: f32) -> f32 {
        let hash = hash_of(&(text, face, size.to_bits()));
        let is = |key: &(Box<str>, Face, u32)| {
            *key.0 == *text && (key.1, key.2) == (face, size.to_bits())
        };
        if let Some(width) = self.widths.get(hash, is) {
            return *width;
        }
        let buffer = self.layout(
            text,
            face,
            size,
            size * LINE_EM,
            Shaping::Advanced,
            Some(Wrap::None),
        );
        let width = buffer
            .layout_runs()
            .fold(0.0_f32, |width, line| width.max(line.line_w));
        self.widths
            .put(hash, (text.into(), face, size.to_bits()), width);
        width
    }

    fn layout(
        &mut self,
        text: &str,
        face: Face,
        size: f32,
        line_height: f32,
        shaping: Shaping,
        wrap: Option<Wrap>,
    ) -> Buffer {
        let metrics = Metrics::new(size, line_height.max(f32::MIN_POSITIVE));
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_size(&mut self.fonts, Some(f32::INFINITY), Some(f32::INFINITY));
        if let Some(wrap) = wrap {
            buffer.set_wrap(&mut self.fonts, wrap);
        }
        buffer.set_text(&mut self.fonts, text, &face.attrs(), shaping, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }

    /// The glyphs of `label` as quads, written at `at`.
    ///
    /// Glyphs land on the pixels iced puts them on. Each is placed by
    /// cutting its height down to a whole pixel, which for a height
    /// below 0 rounds the other way; a row's text is at 0 here and far
    /// below it on screen, so it is placed that far down and moved back.
    #[allow(clippy::cast_possible_truncation)]
    pub fn write(&mut self, out: &mut Vec<Quad>, label: &Label<'_>, at: Point) {
        /// How far down a row's text is placed, in whole pixels
        const LIFT: u16 = 4096;
        if label.color[3] == 0 || label.text.is_empty() {
            return;
        }
        let run = self.shape(
            label.text,
            label.face,
            label.size,
            label.line_height,
            label.shaping,
        );
        let x = match label.align {
            Align::Left => at.x,
            Align::Center => at.x - run.width / 2.0,
            Align::Right => at.x - run.width,
        };
        let y = if label.middle {
            at.y - run.height / 2.0
        } else {
            at.y
        };
        let lift = if label.on_row { LIFT } else { 0 };
        let line = run.baseline.round() as i32 - i32::from(lift);
        for glyph in &run.glyphs {
            let placed = glyph.physical((x, y + f32::from(lift)), 1.0);
            let Some(slot) = self.glyph(placed.cache_key) else {
                continue;
            };
            out.push(Quad {
                x: placed.x + slot.dx,
                y: placed.y + slot.dy + line,
                w: slot.w,
                h: slot.h,
                color: label.color,
                fill: if slot.image { Fill::Image } else { Fill::Mask },
                u: slot.u,
                v: slot.v,
            });
        }
    }

    /// The slot of a glyph, rasterized the first time. `None` for glyphs
    /// without pixels, like a space.
    pub fn glyph(&mut self, key: CacheKey) -> Option<Slot> {
        if let Some(slot) = self.atlas.glyphs.get(&key) {
            return *slot;
        }
        let slot = self
            .swash
            .get_image_uncached(&mut self.fonts, key)
            .and_then(|image| {
                let pixels = match image.content {
                    SwashContent::Mask => Pixels::Mask(image.data),
                    SwashContent::Color => Pixels::Rgba(image.data),
                    SwashContent::SubpixelMask => return None,
                };
                self.atlas.store(&Bitmap {
                    dx: image.placement.left,
                    dy: -image.placement.top,
                    width: image.placement.width,
                    height: image.placement.height,
                    pixels,
                })
            });
        self.atlas.glyphs.insert(key, slot);
        slot
    }

    #[cfg(test)]
    fn runs_kept(&self) -> usize {
        self.runs.len()
    }
}

fn hash_of(key: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Run `f` with this thread's text system, made on first use. The
/// window is drawn from one thread, so there is one; `f` must not ask
/// for it again.
pub fn with<R>(f: impl FnOnce(&mut TextSystem) -> R) -> R {
    thread_local! {
        static TEXT: RefCell<TextSystem> = RefCell::new(TextSystem::new());
    }
    TEXT.with_borrow_mut(f)
}

/// How many cells `text` takes in the proportional face: what the
/// terminal UI wraps chat prose to.
pub fn prose_cells(text: &str, modifier: Modifier) -> f32 {
    /// Size widths are measured at, large enough that rounding is lost
    const SIZE: f32 = 100.0;
    let face = Face {
        family: Family::Sans,
        bold: modifier.contains(Modifier::BOLD),
        italic: modifier.contains(Modifier::ITALIC),
    };
    with(|text_system| text_system.run_width(text, face, theme::prose_size(SIZE)))
        / theme::cell_size(SIZE).width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(width: u32, height: u32, coverage: u8) -> Bitmap {
        Bitmap {
            dx: 1,
            dy: -2,
            width,
            height,
            pixels: Pixels::Mask(vec![coverage; (width * height) as usize]),
        }
    }

    fn key(shape: u32) -> ShapeKey {
        ShapeKey {
            shape,
            width: 8,
            height: 18,
            phase: [0, 0],
        }
    }

    #[test]
    fn the_lru_drops_what_was_used_longest_ago() {
        let mut lru: Lru<u64, u64> = Lru::new(8);
        for n in 0..8 {
            lru.put(n, n, n * 10);
        }
        assert_eq!(lru.get(0, |key| *key == 0), Some(&0), "now the newest");
        lru.put(8, 8, 80);
        assert!(lru.len() <= 8);
        assert_eq!(lru.get(0, |key| *key == 0), Some(&0));
        assert_eq!(lru.get(8, |key| *key == 8), Some(&80));
        assert_eq!(lru.get(1, |key| *key == 1), None, "the oldest went");
        assert_eq!(lru.get(8, |key| *key == 9), None, "another key's hash");
    }

    #[test]
    fn the_atlas_packs_shelves_and_reports_the_rows_it_wrote() {
        let mut atlas = Atlas::new();
        let a = atlas
            .shape(key(1), || Some(mask(10, 14, 200)))
            .expect("room");
        let b = atlas
            .shape(key(2), || Some(mask(10, 15, 100)))
            .expect("room");
        let tall = atlas
            .shape(key(3), || Some(mask(30, 40, 50)))
            .expect("room");
        assert_eq!((a.u, a.v, a.dx, a.dy), (0, 0, 1, -2));
        assert_eq!((b.u, b.v), (10, 0), "the same shelf");
        assert_eq!((tall.u, tall.v), (0, 16), "a shelf of its own");
        assert_eq!(atlas.take_dirty(), Some(0..56));
        assert_eq!(atlas.take_dirty(), None);

        let pixel = |x: u32, y: u32| {
            let at = ((y * Atlas::WIDTH + x) * 4) as usize;
            atlas.pixels()[at..at + 4].to_vec()
        };
        assert_eq!(pixel(0, 0), [255, 255, 255, 200]);
        assert_eq!(pixel(10, 14), [255, 255, 255, 100]);
        assert_eq!(pixel(0, 14), [0, 0, 0, 0], "below the first mask");

        let again = atlas.shape(key(1), || panic!("drawn once"));
        assert_eq!(again, Some(a));
        assert_eq!(atlas.shape(key(4), || None), None);
        assert_eq!(atlas.shape(key(4), || panic!("asked once")), None);
    }

    #[test]
    fn the_atlas_grows_then_starts_over_when_full() {
        let mut atlas = Atlas::new();
        atlas.set_limit(1024);
        let generation = atlas.generation();
        let mut next = 0;
        let mut fill = |atlas: &mut Atlas| {
            next += 1;
            atlas.shape(key(next), || Some(mask(Atlas::WIDTH, 100, 255)))
        };
        for _ in 0..10 {
            assert!(fill(&mut atlas).is_some());
        }
        assert_eq!(atlas.size(), (Atlas::WIDTH, 1024), "grown to the limit");
        assert_eq!(atlas.generation(), generation);

        let slot = fill(&mut atlas).expect("room after starting over");
        assert_eq!(slot.v, 0);
        assert_eq!(atlas.generation(), generation + 1);
        assert_eq!(atlas.take_dirty(), Some(0..1024));
        assert!(atlas.shape(key(999), || Some(mask(2000, 4, 1))).is_none());
    }

    #[test]
    fn runs_are_shaped_once_and_glyphs_rasterized_once() {
        let mut text = TextSystem::new();
        let face = Face::mono(false);
        let run = text.shape("fn main()", face, 14.0, 18.0, Shaping::Basic);
        assert_eq!(run.glyphs.len(), 9);
        assert!((run.width - 9.0 * 8.4).abs() < 0.01, "{}", run.width);
        assert!((run.height - 18.0).abs() < f32::EPSILON);
        assert!(run.baseline > 9.0 && run.baseline < 18.0);

        let again = text.shape("fn main()", face, 14.0, 18.0, Shaping::Basic);
        assert!(Arc::ptr_eq(&run, &again), "served from the store");
        let bold = text.shape("fn main()", Face::mono(true), 14.0, 18.0, Shaping::Basic);
        assert!(!Arc::ptr_eq(&run, &bold));
        assert_eq!(text.runs_kept(), 2);

        let glyph = run.glyphs[0].physical((0.0, 0.0), 1.0);
        let slot = text.glyph(glyph.cache_key).expect("`f` has pixels");
        assert!(slot.w > 2 && slot.h > 6, "{slot:?}");
        assert_eq!(text.glyph(glyph.cache_key), Some(slot));
        let space = run.glyphs[2].physical((0.0, 0.0), 1.0);
        assert_eq!(text.glyph(space.cache_key), None);
    }

    #[test]
    fn a_label_is_written_the_same_on_a_row_and_on_the_screen() {
        let mut text = TextSystem::new();
        let label = Label {
            text: "Agé",
            face: Face::mono(false),
            size: 14.0,
            line_height: 18.0,
            shaping: Shaping::Advanced,
            align: Align::Left,
            middle: false,
            color: [1, 2, 3, 255],
            on_row: false,
        };
        let (mut screen, mut row) = (Vec::new(), Vec::new());
        text.write(&mut screen, &label, Point::new(16.8, 90.0));
        let on_row = Label {
            on_row: true,
            ..label
        };
        text.write(&mut row, &on_row, Point::new(16.8, 0.0));
        assert_eq!(screen.len(), 3);
        let lowered: Vec<Quad> = row.iter().map(|quad| quad.lowered(90)).collect();
        assert_eq!(screen, lowered);
        assert!(screen.iter().all(|q| q.fill == Fill::Mask));
        assert!(screen.iter().all(|q| q.y >= 90 && q.y < 108), "{screen:?}");
        assert!(screen[0].x >= 16 && screen[2].x >= 33, "{screen:?}");

        // Centered both ways: around the point.
        let mut centered = Vec::new();
        let middle = Label {
            align: Align::Center,
            middle: true,
            ..label
        };
        text.write(&mut centered, &middle, Point::new(100.0, 100.0));
        let left = centered.iter().map(|q| q.x).min().expect("glyphs");
        let right = centered.iter().map(Quad::right).max().expect("glyphs");
        assert!((left + right - 200).abs() <= 3, "{left}..{right}");
        assert!(centered.iter().all(|q| q.y > 90 && q.y < 110));

        let mut none = Vec::new();
        let clear = Label {
            color: [0; 4],
            ..label
        };
        text.write(&mut none, &clear, Point::new(0.0, 0.0));
        assert!(none.is_empty());
    }

    #[test]
    fn prose_is_measured_in_cells_with_its_trailing_spaces() {
        let narrow = prose_cells("illicit", Modifier::empty());
        let wide = prose_cells("mammoth", Modifier::empty());
        assert!(narrow > 1.0 && narrow < 7.0, "{narrow}");
        assert!(wide > narrow, "proportional: {wide} vs {narrow}");
        let spaced = prose_cells("illicit ", Modifier::empty());
        assert!(spaced > narrow + 0.2, "the space counts: {spaced}");
        assert!(prose_cells("mammoth", Modifier::BOLD) > wide);
    }
}
