//! The bare renderer against the canvas renderer: every screen the
//! snapshot tests draw through iced is painted again from the same app
//! state through [`crate::paint`] and [`crate::raster`], and the two
//! pictures are compared pixel by pixel.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write, beside each
//! `<name>.png` from the canvas renderer, `<name>.bare.png` from this
//! one, `<name>.pair.png` with the two side by side, and
//! `<name>.diff.png` marking the pixels that differ.
//!
//! Set `AURA_DESKTOP_GPU=1` on a machine with a graphics adapter to draw
//! each screen with [`crate::gpu`] as well, and check it against the CPU
//! rasterizer's picture of the same quads.

use crate::app::App;
use crate::geom::Size;
use crate::gpu::Offscreen;
use crate::paint::{Painter, View};
use crate::raster;
use crate::text;

/// Channel levels two renderers may disagree by and still count as the
/// same pixel: they round colors and blends differently.
const CLOSE: u8 = 4;

/// Pixels of one screen that may be further apart than [`CLOSE`], and
/// how far: a sixteenth of full coverage, the smallest step the edge of
/// a drawn symbol is smoothed in.
///
/// The canvas draws a symbol's curves where the symbol is, and this
/// renderer draws them once and stamps the picture wherever the symbol
/// is. The same arithmetic at another place rounds its last digit
/// another way, which now and then moves a curve's edge across one of
/// those steps in a pixel or two. The canvas's own picture of a symbol
/// differs from row to row by as much.
const FEW: usize = 12;
const STEP: u8 = 16;

/// How the two pictures of one screen differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Difference {
    /// Pixels more than [`CLOSE`] apart in some channel
    pub pixels: usize,
    /// The largest gap in any channel
    pub worst: u8,
}

/// Channel levels the GPU may be off the CPU rasterizer by: both blend
/// the same quads, and round differently.
const GPU_CLOSE: u8 = 1;

/// Paint what `app` shows with the bare renderer, as RGBA rows: on the
/// CPU, and on the GPU when that was asked for.
fn paint(app: &App, width: u32, height: u32) -> (Vec<u8>, Option<Vec<u8>>) {
    let grid = app.grid();
    let (cells, prose, cursor) = grid.screen.contents();
    let (cols, rows) = grid.screen.size();
    #[allow(clippy::cast_precision_loss)]
    let view = View {
        size: Size::new(width as f32, height as f32),
        cells,
        cols,
        rows,
        prose,
        cursor: cursor.filter(|_| grid.cursor_on),
        metrics: grid.metrics,
        palette: grid.palette,
        bar: app.bar(),
        chrome: grid.chrome,
    };
    let mut pixels = Vec::new();
    let on_gpu = std::env::var_os("AURA_DESKTOP_GPU").is_some();
    let drawn = text::with(|text| {
        let mut painter = Painter::default();
        let scene = painter.paint_with(&view, text);
        raster::paint(scene, text.atlas(), &mut pixels);
        on_gpu.then(|| {
            let mut gpu = Offscreen::new().expect("AURA_DESKTOP_GPU needs a graphics adapter");
            gpu.read(scene, text.atlas_mut())
        })
    });
    (pixels, drawn)
}

fn compare(canvas: &[u8], bare: &[u8]) -> Difference {
    let mut difference = Difference {
        pixels: 0,
        worst: 0,
    };
    for (a, b) in canvas.chunks_exact(4).zip(bare.chunks_exact(4)) {
        let gap = a[..3]
            .iter()
            .zip(&b[..3])
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        difference.worst = difference.worst.max(gap);
        if gap > CLOSE {
            difference.pixels += 1;
        }
    }
    difference
}

/// Check the bare renderer draws `app` as `canvas`, the canvas
/// renderer's pixels of it in a window of this size, does.
pub fn check(name: &str, app: &App, (width, height): (u32, u32), canvas: &[u8]) {
    let (bare, drawn) = paint(app, width, height);
    assert_eq!(bare.len(), canvas.len(), "{name}: another size");
    let difference = compare(canvas, &bare);
    if let Some(drawn) = drawn {
        assert_eq!(drawn.len(), bare.len(), "{name}: the GPU drew another size");
        let gaps = drawn.iter().zip(&bare).map(|(a, b)| a.abs_diff(*b));
        let worst = gaps.max().unwrap_or(0);
        println!("parity {name}: the GPU is at most {worst} levels off the CPU");
        assert!(worst <= GPU_CLOSE, "{name}: the GPU is {worst} levels off");
    }

    if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("snapshot dir");
        let write = |kind: &str, width: u32, pixels: &[u8]| {
            let image = raster::png(width, height, pixels);
            std::fs::write(dir.join(format!("{name}.{kind}.png")), image).expect("write image");
        };
        write("bare", width, &bare);
        let row = width as usize * 4;
        let mut pair = Vec::with_capacity(canvas.len() * 2);
        for (a, b) in canvas.chunks_exact(row).zip(bare.chunks_exact(row)) {
            pair.extend_from_slice(a);
            pair.extend_from_slice(b);
        }
        write("pair", width * 2, &pair);
        // The canvas picture dimmed, with what differs in red.
        let mut diff = Vec::with_capacity(canvas.len());
        for (a, b) in canvas.chunks_exact(4).zip(bare.chunks_exact(4)) {
            let apart = a[..3]
                .iter()
                .zip(&b[..3])
                .any(|(a, b)| a.abs_diff(*b) > CLOSE);
            if apart {
                diff.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                diff.extend_from_slice(&[a[0] / 3, a[1] / 3, a[2] / 3, 255]);
            }
        }
        write("diff", width, &diff);
        let report = format!("{name}: {difference:?}\n");
        std::fs::write(dir.join(format!("{name}.parity.txt")), report).expect("write report");
    }

    println!("parity {name}: {difference:?}");
    assert!(
        difference.pixels <= FEW && difference.worst <= STEP,
        "{name}: the bare renderer differs from the canvas in {} pixels (worst {})",
        difference.pixels,
        difference.worst
    );
}
