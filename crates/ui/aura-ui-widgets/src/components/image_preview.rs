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
    symbols::pixel::OCTANTS,
    text::{Line, Span},
};
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, OnceLock,
};

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

/// Squared color distance under which a cell's pixels count as one color.
const FLAT: u32 = 24 * 24;

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

/// A `w`×`h` image, whose pixel at `(x, y)` is `color(x, y)` (`None` when
/// transparent), as `▀` cells: 1×2 pixels each.
fn half_block_lines(
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
fn octant_lines(w: u32, h: u32, color: impl Fn(u32, u32) -> Option<[u8; 3]>) -> Vec<Line<'static>> {
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
fn octant_cell(pixels: &[Option<[u8; 3]>; 8]) -> Span<'static> {
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
    // The two most different pixels seed the groups; one refinement pass
    // then moves each group's center to its average.
    let (mut a, mut b, mut widest) = (first, first, 0);
    for (n, &(_, p)) in opaque.iter().enumerate() {
        for &(_, q) in &opaque[n + 1..] {
            let d = distance(p, q);
            if d > widest {
                (a, b, widest) = (p, q, d);
            }
        }
    }
    if widest < FLAT {
        let bg = mean(opaque.iter().map(|&(_, c)| c));
        return Span::styled(" ", Style::default().bg(rgb(bg)));
    }
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
    Span::styled(
        String::from(OCTANTS[usize::from(bits)]),
        Style::default().fg(rgb(a)).bg(rgb(b)),
    )
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

/// A pasted image open over the chat panel.
#[derive(Debug, Clone)]
pub struct ImageView {
    /// The image
    pub preview: Arc<ImagePreview>,
    /// `IMAGE 2`, for the panel header
    pub title: String,
    /// The image drawn at the size of the last frame
    drawn: Option<((u16, u16), Vec<Line<'static>>)>,
}

impl PartialEq for ImageView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.preview, &other.preview) && self.title == other.title
    }
}

impl Eq for ImageView {}

impl ImageView {
    /// Show `preview` under `title`.
    #[must_use]
    pub const fn new(preview: Arc<ImagePreview>, title: String) -> Self {
        Self {
            preview,
            title,
            drawn: None,
        }
    }

    /// The image fitted to `cols` by `rows`, drawn again only when the
    /// size changes.
    pub fn lines(&mut self, cols: u16, rows: u16) -> &[Line<'static>] {
        if self
            .drawn
            .as_ref()
            .is_none_or(|(size, _)| *size != (cols, rows))
        {
            self.drawn = Some(((cols, rows), self.preview.lines(cols, rows)));
        }
        self.drawn
            .as_ref()
            .map_or(&[], |(_, lines)| lines.as_slice())
    }

    /// `1024×768 · PNG · 1.20 MiB`.
    #[must_use]
    pub fn info(&self) -> String {
        let kind = self
            .preview
            .media_type()
            .strip_prefix("image/")
            .unwrap_or("image")
            .to_uppercase();
        #[expect(clippy::cast_precision_loss, reason = "a file size to two places")]
        let mib = self.preview.bytes().len() as f64 / (1024.0 * 1024.0);
        match self.preview.size() {
            Some((w, h)) => format!("{w}×{h} · {kind} · {mib:.2} MiB"),
            None => format!("{kind} · {mib:.2} MiB"),
        }
    }

    /// Open the image in the system's viewer, from a copy in the temp
    /// folder.
    ///
    /// # Errors
    /// Returns why the copy could not be written or the viewer started.
    pub fn open_externally(&self) -> std::io::Result<std::path::PathBuf> {
        let extension = match self.preview.media_type() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let bytes = self.preview.bytes();
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(bytes, &mut hasher);
        let name = format!(
            "aura-image-{:016x}.{extension}",
            std::hash::Hasher::finish(&hasher)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes)?;
        let mut command = if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "start", ""]);
            command
        } else {
            std::process::Command::new("xdg-open")
        };
        let mut child = command
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        // Reap the opener when it exits; it hands off to the viewer at once.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(path)
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
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, image::Rgba([200, 10, 10, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn fit_keeps_the_shape() {
        assert_eq!(fit((100, 50), 20, 12), (20, 10));
        assert_eq!(fit((50, 100), 20, 12), (6, 12));
        assert_eq!(fit((1000, 1), 20, 12), (20, 1));
    }

    fn image(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = RgbaImage::from_fn(w, h, |x, y| image::Rgba(pixel(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_decoded_image_draws_half_block_rows() {
        let preview = ImagePreview::new(png(40, 40), "image/png");
        assert_eq!(preview.size(), Some((40, 40)));
        let thumb = preview.lines_in(CellStyle::HalfBlock, THUMB_COLS, THUMB_ROWS);
        assert_eq!(thumb.len(), usize::from(THUMB_ROWS));
        assert_eq!(lines_width(&thumb), THUMB_ROWS * 2);
        let cell = &thumb[0].spans[0];
        assert_eq!(cell.content, "▀");
        assert_eq!(cell.style.fg, Some(Color::Rgb(200, 10, 10)));
        assert_eq!(cell.style.bg, Some(Color::Rgb(200, 10, 10)));
    }

    #[test]
    fn octants_fill_the_same_box_at_four_times_the_pixels() {
        let preview = ImagePreview::new(png(40, 40), "image/png");
        let lines = preview.lines_in(CellStyle::Octant, THUMB_COLS, THUMB_ROWS);
        assert_eq!(lines.len(), usize::from(THUMB_ROWS));
        assert_eq!(lines_width(&lines), THUMB_ROWS * 2);
        // A flat cell is a blank in its color: nothing for a font to miss.
        let cell = &lines[0].spans[0];
        assert_eq!(cell.content, " ");
        assert_eq!(cell.style.bg, Some(Color::Rgb(200, 10, 10)));
    }

    #[test]
    fn an_octant_cell_splits_its_pixels_into_two_colors() {
        // Left column black, right column white: the left half block.
        let bytes = image(2, 4, |x, _| {
            if x == 0 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let preview = ImagePreview::new(bytes, "image/png");
        let lines = preview.lines_in(CellStyle::Octant, 1, 1);
        let cell = &lines[0].spans[0];
        assert_eq!(cell.content, "▌");
        assert_eq!(cell.style.fg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(cell.style.bg, Some(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn transparent_octant_pixels_keep_the_terminal_background() {
        // Top half red, bottom half clear: the upper half block, no bg.
        let bytes = image(2, 4, |_, y| {
            if y < 2 {
                [200, 10, 10, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let preview = ImagePreview::new(bytes, "image/png");
        let lines = preview.lines_in(CellStyle::Octant, 1, 1);
        let cell = &lines[0].spans[0];
        assert_eq!(cell.content, "▀");
        assert_eq!(cell.style.fg, Some(Color::Rgb(200, 10, 10)));
        assert_eq!(cell.style.bg, None);
    }

    #[test]
    fn octant_cells_pick_any_pattern() {
        // One dark pixel at the bottom right of a light cell.
        let mut cell = [Some([240, 240, 240]); 8];
        cell[7] = Some([10, 10, 10]);
        let span = octant_cell(&cell);
        let bits = OCTANTS
            .iter()
            .position(|&c| span.content == String::from(c))
            .expect("an octant character");
        // The foreground is whichever group holds the first pixel.
        assert_eq!(bits, 0b0111_1111);
        assert_eq!(span.style.fg, Some(Color::Rgb(240, 240, 240)));
        assert_eq!(span.style.bg, Some(Color::Rgb(10, 10, 10)));
    }

    #[test]
    fn cell_style_detection() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        assert_eq!(CellStyle::detect(env(&[])), CellStyle::HalfBlock);
        assert_eq!(
            CellStyle::detect(env(&[("VTE_VERSION", "8401")])),
            CellStyle::Octant
        );
        assert_eq!(
            CellStyle::detect(env(&[("VTE_VERSION", "7600")])),
            CellStyle::HalfBlock
        );
        assert_eq!(
            CellStyle::detect(env(&[
                ("VTE_VERSION", "8401"),
                ("AURA_IMAGE_CELLS", "half")
            ])),
            CellStyle::HalfBlock
        );
        assert_eq!(
            CellStyle::detect(env(&[("AURA_IMAGE_CELLS", "octant")])),
            CellStyle::Octant
        );
    }

    #[test]
    fn an_image_that_does_not_decode_draws_a_placeholder() {
        let preview = ImagePreview::new(b"\x89PNG\r\n\x1a\nbroken".to_vec(), "image/png");
        assert_eq!(preview.size(), None);
        let text: String = preview.thumbnail()[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("no preview"), "{text}");
    }

    #[test]
    fn ids_are_unique_across_previews() {
        let a = ImagePreview::new(png(4, 4), "image/png");
        let b = ImagePreview::new(png(4, 4), "image/png");
        assert_ne!(a.id(), b.id(), "each ImagePreview must get a unique id");
        assert_ne!(a.id(), 0, "id must never be zero");
        assert_ne!(b.id(), 0, "id must never be zero");
    }

    #[test]
    fn graphics_png_present_for_decodable_image() {
        let preview = ImagePreview::new(png(8, 8), "image/png");
        let png_bytes = preview.graphics_png().expect("expected PNG bytes");
        // Must be a valid PNG: check the magic signature.
        assert!(
            png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "graphics_png must be a PNG"
        );
    }

    #[test]
    fn graphics_png_absent_for_broken_image() {
        let preview = ImagePreview::new(b"\x89PNG\r\n\x1a\nbroken".to_vec(), "image/png");
        assert!(
            preview.graphics_png().is_none(),
            "graphics_png must be None for images that do not decode"
        );
    }
}
