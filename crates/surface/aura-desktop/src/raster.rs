//! A frame's quads painted on the CPU: what the tests compare, and what
//! the window shows when there is no GPU.
//!
//! One blend per quad pixel, the same the GPU does: the quad's color (or
//! the atlas's, for color glyphs) over what is there, by the color's
//! alpha times the atlas's coverage. The sum is rounded once, at the
//! end, as the GPU rounds what it worked out in floating point.
//!
//! A tall frame is cut into bands of rows, each painted by a thread of
//! its own with every quad cut to the band.

use crate::scene::{Fill, Quad, Scene};
use crate::text::Atlas;
use std::num::NonZeroUsize;
use std::ops::Range;

/// Rows a thread is given at least: fewer cost more to hand out than to
/// paint.
const BAND_ROWS: usize = 64;
/// Most threads a frame is painted by.
const MOST_BANDS: usize = 8;

/// Paint `scene` into `out` as RGBA rows, top to bottom, resizing it to
/// the scene.
pub fn paint(scene: &Scene, atlas: &Atlas, out: &mut Vec<u8>) {
    let (width, height) = (scene.width as usize, scene.height as usize);
    let stride = width * 4;
    out.clear();
    out.resize(stride * height, 0);
    if stride == 0 {
        return;
    }
    let rows = height.div_ceil(bands(height));
    if rows >= height {
        paint_band(scene, atlas, out, 0);
        return;
    }
    std::thread::scope(|scope| {
        for (index, band) in out.chunks_mut(rows * stride).enumerate() {
            scope.spawn(move || paint_band(scene, atlas, band, index * rows));
        }
    });
}

/// How many bands, a thread each, a frame `height` rows tall is cut into.
pub fn bands(height: usize) -> usize {
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    threads.min(MOST_BANDS).min(height / BAND_ROWS).max(1)
}

/// Paint the rows of `scene` from row `top` into `band`, as many as it
/// holds.
fn paint_band(scene: &Scene, atlas: &Atlas, band: &mut [u8], top: usize) {
    let [red, green, blue, _] = scene.background;
    for pixel in band.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[red, green, blue, 255]);
    }
    let rows = band.len() / (scene.width as usize * 4);
    for quad in &scene.quads {
        paint_quad(quad, scene, atlas, band, top..top + rows);
    }
}

/// Paint `quad` into `out`, which holds the frame's `rows`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn paint_quad(quad: &Quad, scene: &Scene, atlas: &Atlas, out: &mut [u8], rows: Range<usize>) {
    // The part of the quad inside the frame, and in these rows.
    let left = quad.x.max(0);
    let top = quad.y.max(rows.start as i32);
    let right = quad
        .x
        .saturating_add_unsigned(quad.w)
        .min(scene.width as i32);
    let bottom = quad
        .y
        .saturating_add_unsigned(quad.h)
        .min(scene.height as i32)
        .min(rows.end as i32);
    if right <= left || bottom <= top {
        return;
    }
    let stride = scene.width as usize * 4;
    let span = (right - left) as usize * 4;
    let (atlas_width, atlas_height) = atlas.size();
    let [red, green, blue, alpha] = quad.color;
    for y in top..bottom {
        let start = (y as usize - rows.start) * stride + left as usize * 4;
        let row = &mut out[start..start + span];
        if quad.fill == Fill::Solid {
            if alpha == 255 {
                for pixel in row.chunks_exact_mut(4) {
                    pixel[..3].copy_from_slice(&[red, green, blue]);
                }
            } else {
                for pixel in row.chunks_exact_mut(4) {
                    blend(pixel, [red, green, blue], u32::from(alpha) * 255);
                }
            }
            continue;
        }
        // Skipped columns and rows of the quad are skipped in the atlas.
        let v = quad.v + (y - quad.y) as u32;
        let u = quad.u + (left - quad.x) as u32;
        if v >= atlas_height || u + (right - left) as u32 > atlas_width {
            continue;
        }
        let from = ((v * atlas_width + u) * 4) as usize;
        let texels = &atlas.pixels()[from..from + span];
        for (pixel, texel) in row.chunks_exact_mut(4).zip(texels.chunks_exact(4)) {
            let coverage = u32::from(texel[3]) * u32::from(alpha);
            if coverage == 0 {
                continue;
            }
            if quad.fill == Fill::Mask {
                blend(pixel, [red, green, blue], coverage);
            } else {
                blend(pixel, [texel[0], texel[1], texel[2]], coverage);
            }
        }
    }
}

/// The product of two alphas that are each 255 when opaque.
const OPAQUE: u32 = 255 * 255;

/// `color` over `pixel`, `alpha` of [`OPAQUE`] strong.
#[allow(clippy::cast_possible_truncation)] // a mix of two bytes is a byte
fn blend(pixel: &mut [u8], color: [u8; 3], alpha: u32) {
    for (under, over) in pixel.iter_mut().zip(color) {
        let mix = u32::from(over) * alpha + u32::from(*under) * (OPAQUE - alpha);
        *under = ((mix + OPAQUE / 2) / OPAQUE) as u8;
    }
}

/// An RGBA image as a PNG file, stored without compressing.
#[cfg(test)]
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<_> = raw.chunks(0xffff).collect();
    for (i, block) in blocks.iter().enumerate() {
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        zlib.push(u8::from(i + 1 == blocks.len()));
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(*b"IHDR", header), (*b"IDAT", zlib), (*b"IEND", Vec::new())] {
        let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        let start = out.len();
        out.extend_from_slice(&kind);
        out.extend_from_slice(&data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

#[cfg(test)]
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rectangle;
    use crate::shapes;
    use crate::text::{Face, Shaping, TextSystem};

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const WHITE: [u8; 4] = [255; 4];

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    #[test]
    fn quads_are_painted_in_order_and_cut_at_the_edges() {
        let text = TextSystem::new();
        let mut scene = Scene::default();
        scene.begin(8, 6, [10, 20, 30, 255]);
        scene
            .quads
            .extend(Quad::solid(-4.0, -4.0, 3.0, 2.0, [200, 0, 0, 255]));
        scene
            .quads
            .extend(Quad::solid(2.0, 1.0, 20.0, 20.0, [0, 0, 255, 128]));
        let mut pixels = Vec::new();
        paint(&scene, text.atlas(), &mut pixels);
        assert_eq!(pixels.len(), 8 * 6 * 4);
        assert_eq!(pixel(&pixels, 8, 0, 0), [200, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 8, 3, 0), [10, 20, 30, 255], "untouched");
        assert_eq!(pixel(&pixels, 8, 2, 1), [100, 0, 128, 255], "half over");
        assert_eq!(pixel(&pixels, 8, 7, 5), [5, 10, 143, 255]);

        // The same list paints the same picture into a used buffer.
        let again = pixels.clone();
        paint(&scene, text.atlas(), &mut pixels);
        assert_eq!(pixels, again);
    }

    #[test]
    fn masks_tint_by_coverage_and_text_lands_on_its_line() {
        let mut text = TextSystem::new();
        let mut scene = Scene::default();
        scene.begin(40, 20, BLACK);
        let cell = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 8.4,
            height: 18.0,
        };
        assert!(shapes::draw(
            &mut scene.quads,
            text.atlas_mut(),
            '◆',
            cell,
            WHITE
        ));
        let run = text.shape("l", Face::mono(false), 14.0, 18.0, Shaping::Basic);
        let glyph = run.glyphs[0].physical((20.0, 0.0), 1.0);
        let slot = text.glyph(glyph.cache_key).expect("`l` has pixels");
        #[allow(clippy::cast_possible_truncation)]
        let baseline = run.baseline.round() as i32;
        scene.quads.push(Quad {
            x: glyph.x + slot.dx,
            y: glyph.y + slot.dy + baseline,
            w: slot.w,
            h: slot.h,
            color: [0, 255, 0, 255],
            fill: Fill::Mask,
            u: slot.u,
            v: slot.v,
        });
        let mut pixels = Vec::new();
        paint(&scene, text.atlas(), &mut pixels);

        assert_eq!(pixel(&pixels, 40, 4, 9), WHITE, "inside the diamond");
        assert_eq!(pixel(&pixels, 40, 0, 0), BLACK, "outside it");
        let edge = pixel(&pixels, 40, 1, 8);
        assert!(edge[0] > 0 && edge[0] < 255, "its slanted edge: {edge:?}");
        assert_eq!(edge[0], edge[1], "a gray, between the two colors");

        let green: Vec<(u32, u32)> = (0..20)
            .flat_map(|y| (16..40).map(move |x| (x, y)))
            .filter(|(x, y)| pixel(&pixels, 40, *x, *y)[1] > 128)
            .collect();
        assert!(green.len() > 8, "an `l` is a stem: {green:?}");
        let rows = green.iter().map(|(_, y)| *y);
        let (top, bottom) = (rows.clone().min(), rows.max());
        assert!(top >= Some(1) && bottom <= Some(15), "{top:?}..{bottom:?}");
        assert!(green.iter().all(|(x, _)| (20..29).contains(x)), "{green:?}");
        assert!(green
            .iter()
            .all(|(x, y)| pixel(&pixels, 40, *x, *y)[0] == 0));
    }

    #[test]
    fn a_frame_painted_in_bands_is_the_frame_painted_whole() {
        let mut text = TextSystem::new();
        let mut scene = Scene::default();
        scene.begin(50, 300, [10, 20, 30, 255]);
        for (step, top) in (0u8..).zip([0.0, 60.0, 70.0, 140.0, 148.0, 220.0, 290.0]) {
            let color = [200, step * 30, 90, 100 + step * 20];
            scene
                .quads
                .extend(Quad::solid(-5.0, top, 40.0, top + 20.0, color));
        }
        for y in [60.0, 145.0, 223.0] {
            let cell = Rectangle {
                x: 10.0,
                y,
                width: 8.4,
                height: 18.0,
            };
            shapes::draw(&mut scene.quads, text.atlas_mut(), '◆', cell, WHITE);
        }
        let mut banded = Vec::new();
        paint(&scene, text.atlas(), &mut banded);
        let mut whole = vec![0; banded.len()];
        paint_band(&scene, text.atlas(), &mut whole, 0);
        assert_eq!(banded.len(), 50 * 300 * 4);
        assert!(banded == whole, "the bands meet without a seam");
    }

    #[test]
    fn the_png_holds_the_pixels() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255];
        let file = png(2, 1, &rgba);
        assert_eq!(&file[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&file[12..16], b"IHDR");
        assert_eq!(&file[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        let data = file.windows(8).position(|w| w == rgba).expect("stored");
        assert_eq!(file[data - 1], 0, "the row's filter byte");
        assert_eq!(&file[file.len() - 8..file.len() - 4], b"IEND");
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }
}
