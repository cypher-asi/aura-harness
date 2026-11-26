//! Pasted images drawn in the terminal with block characters.
//!
//! By default each cell shows two pixels stacked: `▀` in the top pixel's
//! color over a background of the bottom pixel's. Terminal cells are
//! about twice as tall as wide, so those pixels come out roughly square.
//! This works in any truecolor terminal, and in the desktop app, which
//! draws the same cells, without a graphics protocol.
//!
//! Where the terminal draws octant characters itself (VTE, so GNOME
//! Console, Ptyxis, Terminator and others), each cell shows 2×4 pixels
//! instead, in the two colors that best split them; see [`CellStyle`].
//!
//! When the terminal supports the Kitty graphics protocol, `graphics_png`
//! provides the PNG bytes that the protocol layer sends; the block
//! rendering remains in the ratatui buffer as a fallback underneath.

use image::{imageops::FilterType, ImageFormat, RgbaImage};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, OnceLock,
};

mod cells;
mod view;

use cells::{half_block_lines, octant_lines};
pub use view::{open_with_system, ImageView};

/// Process-wide counter for assigning unique ids to `ImagePreview` instances.
///
/// Invariant: ids are never zero (the counter starts at 1 and wraps are
/// astronomically unlikely in a single process run). Ids are stable for
/// the lifetime of the owning `ImagePreview`.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Largest side kept of a decoded image: enough for a full-screen view,
/// which has at most a few hundred half-block pixels a side.
const KEEP_SIDE: u32 = 480;

/// Columns of a thumbnail at most.
pub const THUMB_COLS: u16 = 24;

/// Rows of a thumbnail at most: as tall as a square image is wide, so a
/// portrait screenshot is not squeezed into a few columns.
pub const THUMB_ROWS: u16 = 10;

/// Alpha below which a pixel is left as the terminal's background.
const TRANSPARENT: u8 = 96;

/// First `VTE_VERSION` whose terminals draw the octant characters
/// themselves rather than hoping a font has them.
const VTE_OCTANTS: u32 = 8000;

/// How an image's pixels are laid onto cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStyle {
    /// `▀` over a background: 1×2 pixels a cell, each in its own color.
    HalfBlock,
    /// Octant characters: 2×4 pixels a cell, in two colors.
    Octant,
}

impl CellStyle {
    /// The style for the terminal described by `vars`, which returns an
    /// environment variable's value, or `None` when it is unset.
    ///
    /// `AURA_IMAGE_CELLS=octant` or `=half` decides it. Otherwise VTE from
    /// [`VTE_OCTANTS`] on gets octants; every other terminal keeps
    /// half-blocks, which any font can draw.
    fn detect(vars: impl Fn(&str) -> Option<String>) -> Self {
        match vars("AURA_IMAGE_CELLS").as_deref() {
            Some("octant") => return Self::Octant,
            Some("half") => return Self::HalfBlock,
            _ => {}
        }
        let vte = vars("VTE_VERSION").and_then(|v| v.trim().parse::<u32>().ok());
        if vte.is_some_and(|v| v >= VTE_OCTANTS) {
            Self::Octant
        } else {
            Self::HalfBlock
        }
    }

    /// The style for this process's terminal, detected once.
    fn current() -> Self {
        static STYLE: OnceLock<CellStyle> = OnceLock::new();
        *STYLE.get_or_init(|| Self::detect(|name| std::env::var(name).ok()))
    }

    /// Pixels one cell holds, across and down.
    const fn pixels(self) -> (u32, u32) {
        match self {
            Self::HalfBlock => (1, 2),
            Self::Octant => (2, 4),
        }
    }
}

/// A pasted image: the bytes as pasted, and a decoded copy small enough
/// to draw quickly.
pub struct ImagePreview {
    /// Stable, process-unique id assigned at construction; never zero.
    id: u32,
    /// The image as pasted
    bytes: Vec<u8>,
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`
    media_type: String,
    /// Decoded pixels, shrunk to [`KEEP_SIDE`]; `None` when the image
    /// would not decode
    pixels: Option<RgbaImage>,
    /// Size of the image as pasted, when it decoded
    size: Option<(u32, u32)>,
    /// The thumbnail's rows, drawn once
    thumbnail: Vec<Line<'static>>,
    /// PNG-encoded bytes of `pixels` for the Kitty graphics protocol;
    /// `None` when the image did not decode (the half-block placeholder
    /// is used instead).
    graphics_png: Option<Vec<u8>>,
}

impl std::fmt::Debug for ImagePreview {
    // The bytes are megabytes; logging them would bury everything else.
    // graphics_png is also omitted for the same reason.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImagePreview")
            .field("id", &self.id)
            .field("media_type", &self.media_type)
            .field("bytes", &self.bytes.len())
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ImagePreview {
    /// Decode `bytes` for drawing. An image that does not decode is still
    /// kept, and draws as a placeholder.
    ///
    /// PNG bytes are encoded from the downscaled pixels and cached in
    /// `graphics_png` for use by the Kitty graphics protocol layer. If PNG
    /// encoding fails the field is `None` and the half-block renderer is
    /// used instead (non-fatal).
    #[must_use]
    pub fn new(bytes: Vec<u8>, media_type: &str) -> Arc<Self> {
        let decoded = image::load_from_memory(&bytes);
        if let Err(e) = &decoded {
            tracing::debug!(error = %e, media_type, "Pasted image does not decode");
        }
        let decoded = decoded.ok();
        let size = decoded.as_ref().map(|img| (img.width(), img.height()));
        let pixels = decoded.map(|img| {
            if img.width() > KEEP_SIDE || img.height() > KEEP_SIDE {
                img.resize(KEEP_SIDE, KEEP_SIDE, FilterType::Lanczos3)
                    .to_rgba8()
            } else {
                img.to_rgba8()
            }
        });
        // Encode the downscaled pixels as PNG once, for the Kitty graphics
        // protocol.  Failure is non-fatal: the half-block renderer is used
        // instead, and the protocol layer skips images whose `graphics_png`
        // is `None`.
        let graphics_png = pixels.as_ref().and_then(|px| {
            let mut buf = Cursor::new(Vec::new());
            px.write_to(&mut buf, ImageFormat::Png)
                .map(|()| buf.into_inner())
                .map_err(|e| {
                    tracing::debug!(error = %e, "Could not encode image for graphics protocol");
                })
                .ok()
        });
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut preview = Self {
            id,
            bytes,
            media_type: media_type.to_string(),
            pixels,
            size,
            thumbnail: Vec::new(),
            graphics_png,
        };
        preview.thumbnail = preview.lines(THUMB_COLS, THUMB_ROWS);
        Arc::new(preview)
    }

    /// The image as pasted.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Stable, process-unique id assigned at construction; never zero.
    ///
    /// Two `ImagePreview` values have the same id only when they point to
    /// the same allocation (i.e. `Arc::ptr_eq`).
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// PNG-encoded bytes of the downscaled pixels, for the Kitty graphics
    /// protocol. `None` when the image did not decode or could not be
    /// re-encoded.
    #[must_use]
    pub fn graphics_png(&self) -> Option<&[u8]> {
        self.graphics_png.as_deref()
    }

    /// `image/png` and the like.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Width and height as pasted, when the image decoded.
    #[must_use]
    pub const fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// The thumbnail's rows, at most [`THUMB_COLS`] by [`THUMB_ROWS`].
    #[must_use]
    pub fn thumbnail(&self) -> &[Line<'static>] {
        &self.thumbnail
    }

    /// Columns of the thumbnail.
    #[must_use]
    pub fn thumbnail_width(&self) -> u16 {
        lines_width(&self.thumbnail)
    }

    /// The image as large as fits in `cols` by `rows` cells, keeping its
    /// shape. One that did not decode is a box saying so.
    #[must_use]
    pub fn lines(&self, cols: u16, rows: u16) -> Vec<Line<'static>> {
        self.lines_in(CellStyle::current(), cols, rows)
    }

    /// [`Self::lines`] in cell style `style`.
    fn lines_in(&self, style: CellStyle, cols: u16, rows: u16) -> Vec<Line<'static>> {
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let Some(pixels) = &self.pixels else {
            return placeholder(cols.min(THUMB_COLS), rows.min(3));
        };
        let (across, down) = style.pixels();
        let (w, h) = fit(
            (pixels.width(), pixels.height()),
            u32::from(cols) * across,
            u32::from(rows) * down,
        );
        let scaled = image::imageops::resize(pixels, w, h, FilterType::CatmullRom);
        let color = |x: u32, y: u32| {
            (x < w && y < h)
                .then(|| *scaled.get_pixel(x, y))
                .and_then(|p| (p[3] >= TRANSPARENT).then_some([p[0], p[1], p[2]]))
        };
        match style {
            CellStyle::HalfBlock => half_block_lines(w, h, color),
            CellStyle::Octant => octant_lines(w, h, color),
        }
    }
}

/// Pixel size of a `(w, h)` image shrunk or grown to fit `max_w` by
/// `max_h`, keeping its shape and at least one pixel a side.
fn fit((w, h): (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = (u64::from(w.max(1)), u64::from(h.max(1)));
    let (max_w, max_h) = (u64::from(max_w.max(1)), u64::from(max_h.max(1)));
    // Width-limited unless that makes it too tall.
    let (fw, fh) = if w * max_h >= h * max_w {
        (max_w, (h * max_w / w).max(1))
    } else {
        ((w * max_h / h).max(1), max_h)
    };
    (
        u32::try_from(fw).unwrap_or(1),
        u32::try_from(fh).unwrap_or(1),
    )
}

/// A box `cols` wide marking an image that cannot be drawn.
fn placeholder(cols: u16, rows: u16) -> Vec<Line<'static>> {
    let cols = usize::from(cols.max(3));
    let style = Style::default().fg(Color::DarkGray);
    let inner = cols - 2;
    let label: String = " no preview".chars().take(inner).collect();
    let mut lines = vec![Line::from(Span::styled(
        format!("┌{}┐", "─".repeat(inner)),
        style,
    ))];
    if rows > 2 {
        lines.push(Line::from(Span::styled(
            format!("│{label:<inner$}│"),
            style,
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("└{}┘", "─".repeat(inner)),
        style,
    )));
    lines
}

/// Widest of `lines`, in columns.
fn lines_width(lines: &[Line<'_>]) -> u16 {
    lines
        .iter()
        .map(Line::width)
        .max()
        .map_or(0, |w| u16::try_from(w).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests;
