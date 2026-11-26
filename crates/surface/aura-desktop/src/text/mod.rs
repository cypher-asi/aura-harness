//! Text: the fonts, shaped runs, and the atlas the glyphs are drawn
//! into.
//!
//! Shaping and glyph rasterization are cosmic-text's. Shaped runs and
//! measured widths are kept in bounded
//! least-recently-used stores: a run is shaped once however many frames
//! show it. Each glyph is rasterized once per size and sub-pixel position
//! into the [`Atlas`], which drawn shapes share. Threads that only lay
//! text out measure it with a [`Measurer`], which has no atlas.

use crate::geom::Point;
use crate::scene::{Fill, Quad};
use crate::theme;
use cosmic_text::{
    fontdb, Attrs, Buffer, CacheKey, Family as FontFamily, FontSystem, LayoutGlyph, Metrics, Style,
    SwashCache, SwashContent, Weight, Wrap,
};
use ratatui::style::Modifier;
use rustc_hash::FxHasher;
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

mod atlas;
mod lru;

pub use atlas::{Atlas, AtlasRows, Bitmap, Pixels, ShapeKey, Slot};
pub use cosmic_text::Shaping;
use lru::Lru;

/// Shaped runs kept.
const MAX_RUNS: usize = 4096;
/// Measured widths kept.
const MAX_WIDTHS: usize = 8192;
/// Line height prose is measured at, in ems. A width doesn't depend on
/// it.
const LINE_EM: f32 = 1.3;
/// Line height of a label on a title bar or the tab bar, in ems.
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

    /// The family by the name [`crate::theme`] gives it, at its regular
    /// or bold weight.
    fn attrs(self) -> Attrs<'static> {
        let (family, weight) = match (self.family, self.bold) {
            (Family::Mono, false) => (theme::mono_family(), Weight::NORMAL),
            (Family::Sans, false) => (theme::sans_family(), Weight::NORMAL),
            (Family::Mono, true) => (theme::mono_family(), Weight::BOLD),
            (Family::Sans, true) => (theme::sans_family(), Weight::BOLD),
        };
        let family = FontFamily::Name(family);
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

#[derive(PartialEq)]
struct RunKey {
    text: Box<str>,
    face: Face,
    size: u32,
    line_height: u32,
    advanced: bool,
}

/// The fonts and how wide text measured in them is: all laying text out
/// needs, without drawing any.
pub struct Measurer {
    fonts: FontSystem,
    widths: Lru<(Box<str>, Face, u32), f32>,
}

impl Measurer {
    fn new() -> Self {
        Self {
            fonts: font_system(),
            widths: Lru::new(MAX_WIDTHS),
        }
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
        let buffer = layout(
            &mut self.fonts,
            text,
            (face, size, size * LINE_EM),
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
}

/// The system's fonts and the bundled ones. They are found once, and
/// every thread's text system or measurer starts from a copy.
fn font_system() -> FontSystem {
    static FONTS: OnceLock<(String, fontdb::Database)> = OnceLock::new();
    let (locale, fonts) = FONTS.get_or_init(|| {
        let bundled = theme::font_files()
            .into_iter()
            .map(|font| fontdb::Source::Binary(Arc::new(font.into_owned())));
        FontSystem::new_with_fonts(bundled).into_locale_and_db()
    });
    FontSystem::new_with_locale_and_db(locale.clone(), fonts.clone())
}

/// `text` in `face` at `size`, in lines `line_height` tall.
fn layout(
    fonts: &mut FontSystem,
    text: &str,
    (face, size, line_height): (Face, f32, f32),
    shaping: Shaping,
    wrap: Option<Wrap>,
) -> Buffer {
    let metrics = Metrics::new(size, line_height.max(f32::MIN_POSITIVE));
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_size(fonts, Some(f32::INFINITY), Some(f32::INFINITY));
    if let Some(wrap) = wrap {
        buffer.set_wrap(fonts, wrap);
    }
    buffer.set_text(fonts, text, &face.attrs(), shaping, None);
    buffer.shape_until_scroll(fonts, false);
    buffer
}

/// The fonts, what was shaped and measured with them, and the atlas.
pub struct TextSystem {
    measurer: Measurer,
    swash: SwashCache,
    runs: Lru<RunKey, Arc<Run>>,
    atlas: Atlas,
}

impl TextSystem {
    /// The system's fonts and the bundled ones.
    pub fn new() -> Self {
        Self {
            measurer: Measurer::new(),
            swash: SwashCache::new(),
            runs: Lru::new(MAX_RUNS),
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
        let buffer = layout(
            &mut self.measurer.fonts,
            text,
            (face, size, line_height),
            shaping,
            None,
        );
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
        self.measurer.run_width(text, face, size)
    }

    /// The glyphs of `label` as quads, written at `at`.
    ///
    /// A glyph is placed by cutting its height down to a whole pixel,
    /// which for a height below 0 rounds the other way. So that a row's
    /// text lands on the same pixels wherever the row is, it is placed
    /// far below 0 and moved back.
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
            .get_image_uncached(&mut self.measurer.fonts, key)
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
    let mut hasher = FxHasher::default();
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
/// terminal UI wraps chat prose to. The chat is laid out on several
/// threads, so each measures with a measurer of its own, which has no
/// atlas to keep.
pub fn prose_cells(text: &str, modifier: Modifier) -> f32 {
    /// Size widths are measured at, large enough that rounding is lost
    const SIZE: f32 = 100.0;
    thread_local! {
        static MEASURER: RefCell<Measurer> = RefCell::new(Measurer::new());
    }
    let face = Face {
        family: Family::Sans,
        bold: modifier.contains(Modifier::BOLD),
        italic: modifier.contains(Modifier::ITALIC),
    };
    MEASURER.with_borrow_mut(|measurer| measurer.run_width(text, face, theme::prose_size(SIZE)))
        / theme::cell_size(SIZE).width
}

#[cfg(test)]
mod tests;
