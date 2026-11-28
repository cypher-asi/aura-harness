use super::frame::encode;
use super::{Offscreen, INSTANCE};
use crate::chrome::Chrome;
use crate::geom::{Point, Rectangle, Size};
use crate::grid::{Metrics, Palette};
use crate::paint::{Painter, View};
use crate::raster;
use crate::scene::{Fill, Quad, Scene};
use crate::shapes;
use crate::tab_bar::{self, Bar};
use crate::text::Atlas;
use crate::text::{Align, Bitmap, Face, Family, Label, Pixels, ShapeKey, Shaping, TextSystem};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as TermColor, Modifier, Style};

/// Channel levels the GPU may be off the CPU by: it blends in
/// floating point and rounds once, where the CPU rounds each product.
const CLOSE: u8 = 1;

/// Check the GPU paints `scene` as the CPU does.
fn same(gpu: &mut Offscreen, scene: &Scene, atlas: &mut Atlas, what: &str) {
    let mut cpu = Vec::new();
    raster::paint(scene, atlas, &mut cpu);
    let drawn = gpu.read(scene, atlas);
    assert_eq!(drawn.len(), cpu.len(), "{what}: another size");
    let gap = |(a, b): (&[u8], &[u8])| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max();
    let pixels = || drawn.chunks_exact(4).zip(cpu.chunks_exact(4));
    let worst = pixels().filter_map(gap).max().unwrap_or(0);
    // A software rasterizer blends in eight bits, and stacked
    // translucent quads can add up to one level more.
    let close = CLOSE + u8::from(gpu.is_software());
    let apart = pixels().filter(|pair| gap(*pair) > Some(close)).count();
    let off = pixels().filter(|pair| gap(*pair) > Some(0)).count();
    println!(
        "{what} on {}: {} quads, {off} pixels off by a level, worst {worst}",
        gpu.name(),
        scene.quads.len()
    );
    if apart > 0 {
        if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
            let dir = std::path::PathBuf::from(dir);
            let name = what.replace(' ', "-");
            for (kind, pixels) in [("gpu", &drawn), ("cpu", &cpu)] {
                let image = raster::png(scene.width, scene.height, pixels);
                std::fs::write(dir.join(format!("{name}.{kind}.png")), image).expect("write");
            }
        }
    }
    assert_eq!(apart, 0, "{what}: {apart} pixels apart, worst {worst}");
}

fn label(text: &str, size: f32, color: [u8; 4]) -> Label<'_> {
    Label {
        text,
        face: Face::mono(false),
        size,
        line_height: (size * 1.3).round(),
        shaping: Shaping::Advanced,
        align: Align::Left,
        middle: false,
        color,
        on_row: false,
    }
}

/// A picture with its own colors, clear to opaque left to right.
fn picture(width: u32, height: u32, seed: u32) -> Bitmap {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let level = |n: u32| u8::try_from(n % 256).unwrap_or(0);
            pixels.extend_from_slice(&[
                level(x * 7 + seed),
                level(y * 11 + seed * 3),
                level((x + y) * 5),
                level(x * 255 / width.max(1)),
            ]);
        }
    }
    Bitmap {
        dx: 0,
        dy: 0,
        width,
        height,
        pixels: Pixels::Rgba(pixels),
    }
}

fn key(shape: u32, width: u32, height: u32) -> ShapeKey {
    ShapeKey {
        shape,
        width,
        height,
        phase: [0, 0],
    }
}

/// Solid colors: cut by the frame, over each other, see-through.
fn solid_quads(quads: &mut Vec<Quad>) {
    quads.extend(Quad::solid(-4.0, -4.0, 60.0, 30.0, [200, 0, 0, 255]));
    quads.extend(Quad::solid(40.0, 10.0, 400.0, 400.0, [0, 0, 255, 128]));
    quads.extend(Quad::solid(0.0, 100.0, 300.0, 101.0, [255, 255, 255, 37]));
    for step in 0..32u8 {
        let x = f32::from(step) * 9.0;
        let color = [step * 8, 255 - step * 8, 90, step * 8 + 7];
        quads.extend(Quad::solid(x, 120.0, x + 9.0, 150.0, color));
    }
}

/// Text, in three faces, opaque and see-through, and hanging off
/// the left edge.
fn text_quads(text: &mut TextSystem, quads: &mut Vec<Quad>, white: [u8; 4]) {
    text.write(
        quads,
        &label("The quick brown fox", 14.0, white),
        Point::new(8.0, 4.0),
    );
    text.write(
        quads,
        &label("jumps over 0123456789", 22.0, [255, 200, 0, 140]),
        Point::new(-9.0, 30.0),
    );
    let prose = Label {
        face: Face {
            family: Family::Sans,
            bold: true,
            italic: true,
        },
        ..label(
            "the lazy dog; fi fl \u{2192} \u{e9}",
            17.0,
            [120, 255, 160, 255],
        )
    };
    text.write(quads, &prose, Point::new(70.5, 64.25));
}

/// Drawn characters, straight and curved.
fn drawn_characters(text: &mut TextSystem, quads: &mut Vec<Quad>) {
    for (index, c) in "\u{256d}\u{2500}\u{256e}\u{2502}\u{25c6}\u{28ff}\u{2588}\u{2591}\u{1f5c1}"
        .chars()
        .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let cell = Rectangle {
            x: 12.0 + index as f32 * 8.4,
            y: 104.0,
            width: 8.4,
            height: 18.0,
        };
        shapes::draw(quads, text.atlas_mut(), c, cell, [0, 255, 255, 255]);
    }
}

/// A picture with its own colors, whole, faded and cut.
fn picture_quads(text: &mut TextSystem, quads: &mut Vec<Quad>) {
    let slot = text
        .atlas_mut()
        .shape(key(9000, 40, 24), || Some(picture(40, 24, 1)))
        .expect("room");
    for (x, y, alpha) in [(200, 20, 255), (230, 50, 128), (280, 148, 255)] {
        quads.push(Quad {
            x,
            y,
            w: slot.w,
            h: slot.h,
            color: [255, 255, 255, alpha],
            fill: Fill::Image,
            u: slot.u,
            v: slot.v,
        });
    }
}

#[test]
#[ignore = "needs a graphics adapter: cargo test -p aura-desktop -- --ignored gpu"]
fn the_gpu_paints_what_the_cpu_paints() {
    let mut gpu = Offscreen::new().expect("a graphics adapter");
    let mut text = TextSystem::new();
    let mut scene = Scene::default();
    scene.begin(300, 160, [10, 20, 30, 255]);

    let quads = &mut scene.quads;
    let white = [255, 255, 255, 255];
    solid_quads(quads);
    text_quads(&mut text, quads, white);
    drawn_characters(&mut text, quads);
    picture_quads(&mut text, quads);
    same(&mut gpu, &scene, text.atlas_mut(), "a mixed frame");

    // The next frame has glyphs the texture does not: only their
    // rows are sent.
    text.write(
        &mut scene.quads,
        &label("NEW GLYPHS @ 31", 31.0, white),
        Point::new(4.0, 60.0),
    );
    same(
        &mut gpu,
        &scene,
        text.atlas_mut(),
        "a frame with new glyphs",
    );

    // A frame of nothing is its background.
    scene.begin(64, 48, [1, 2, 3, 255]);
    same(&mut gpu, &scene, text.atlas_mut(), "an empty frame");
}

#[test]
#[ignore = "needs a graphics adapter: cargo test -p aura-desktop -- --ignored gpu"]
fn the_gpu_follows_the_atlas_as_it_grows_and_starts_over() {
    let mut gpu = Offscreen::new().expect("a graphics adapter");
    assert!(gpu.atlas_limit() >= 2048, "{}", gpu.atlas_limit());
    let mut text = TextSystem::new();
    text.atlas_mut().set_limit(1024);
    let first = (text.atlas().size(), text.atlas().generation());
    let (mut grew, mut restarted) = (false, false);
    let mut slots = Vec::new();
    let mut scene = Scene::default();
    for step in 0..14u32 {
        let generation = text.atlas().generation();
        let slot = text
            .atlas_mut()
            .shape(key(step, 900, 90), || Some(picture(900, 90, step)))
            .expect("room");
        if text.atlas().generation() != generation {
            // Everything before this one is gone from the atlas.
            slots.clear();
            restarted = true;
        }
        grew |= text.atlas().size() != first.0;
        slots.push(slot);

        // Every picture still in the atlas, a strip of each.
        scene.begin(900, 14 * 12, [0, 0, 0, 255]);
        for (row, slot) in slots.iter().enumerate() {
            scene.quads.push(Quad {
                x: 0,
                y: i32::try_from(row * 12).unwrap_or(0),
                w: slot.w,
                h: 12,
                color: [255; 4],
                fill: Fill::Image,
                u: slot.u,
                v: slot.v + 40,
            });
        }
        same(
            &mut gpu,
            &scene,
            text.atlas_mut(),
            &format!("atlas step {step}"),
        );
    }
    assert!(grew, "the atlas grew past {:?}", first.0);
    assert!(restarted, "the atlas started over");
}

const COLS: u16 = 48;
const ROWS: u16 = 6;

/// The grid the zoom test paints: box drawing, styles and symbols.
fn grid_buffer() -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
    let plain = Style::default();
    let loud = plain
        .fg(TermColor::Rgb(255, 180, 0))
        .bg(TermColor::Rgb(40, 0, 90))
        .add_modifier(Modifier::BOLD);
    buffer.set_string(0, 0, "\u{256d}\u{2500} agents \u{2500}\u{2500}\u{2500}\u{256e} \u{28f7} working \u{2588}\u{2593}\u{2592}\u{2591}", plain);
    buffer.set_string(
        0,
        1,
        "\u{2502} fn main() { println!(\"hi\"); } \u{2502}",
        plain,
    );
    buffer.set_string(2, 2, "BOLD on a background, 0123456789", loud);
    buffer.set_string(
        0,
        3,
        "italic and dim ~!@#$%^&*()_+",
        plain.add_modifier(Modifier::ITALIC | Modifier::DIM),
    );
    buffer.set_string(
        0,
        4,
        "underlined, crossed out",
        plain.add_modifier(Modifier::UNDERLINED | Modifier::CROSSED_OUT),
    );
    buffer.set_string(
        0,
        5,
        "\u{2570}\u{2500}\u{2500}\u{2500}\u{256f} \u{25c6} \u{25cf} \u{2713} \u{1f5c1}",
        plain,
    );
    buffer
}

#[test]
#[ignore = "needs a graphics adapter: cargo test -p aura-desktop -- --ignored gpu"]
fn the_gpu_paints_the_grid_at_every_zoom() {
    let mut gpu = Offscreen::new().expect("a graphics adapter");
    let mut text = TextSystem::new();
    let palette = Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber"));
    let (bar, chrome) = (Bar::default(), Chrome::default());
    let buffer = grid_buffer();

    let mut painter = Painter::default();
    let mut generations = Vec::new();
    for size in [14.0, 15.0, 8.0, 21.5, 40.0, 14.0] {
        let metrics = Metrics::new(size);
        let view = View {
            size: Size::new(
                (f32::from(COLS) * metrics.cell.width).ceil() + 13.0,
                tab_bar::height(&metrics) + f32::from(ROWS) * metrics.cell.height + 9.0,
            ),
            cells: &buffer.content,
            cols: COLS,
            rows: ROWS,
            prose: &[],
            panes: &[],
            bands: &[],
            cursor: Some(ratatui::layout::Position::new(3, 2)),
            metrics,
            palette,
            bar: &bar,
            chrome: &chrome,
            slide: None,
            canvas: None,
        };
        painter.paint_with(&view, &mut text);
        generations.push(text.atlas().generation());
        same(
            &mut gpu,
            painter.scene(),
            text.atlas_mut(),
            &format!("the grid at {size}"),
        );
    }
    generations.dedup();
    assert_eq!(
        generations.len(),
        6,
        "each size starts the atlas over: {generations:?}"
    );
    assert_eq!(
        text.atlas().size().1,
        512,
        "one size's glyphs fit the first atlas"
    );
}

#[test]
fn a_quad_is_encoded_as_the_shader_reads_it() {
    let quad = Quad {
        x: -3,
        y: 7,
        w: 20,
        h: 18,
        color: [1, 2, 3, 4],
        fill: Fill::Mask,
        u: 512,
        v: 96,
    };
    let mut bytes = Vec::new();
    encode(&mut bytes, &quad);
    encode(&mut bytes, &quad.lowered(1));
    assert_eq!(bytes.len(), 2 * INSTANCE);
    let word =
        |at: usize| u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    assert_eq!(word(0), (-3i32).cast_unsigned());
    assert_eq!([word(4), word(8), word(12)], [7, 20, 18]);
    assert_eq!([word(16), word(20)], [512, 96]);
    assert_eq!(&bytes[24..28], &[1, 2, 3, 4]);
    assert_eq!(word(28), 1);
    assert_eq!(word(INSTANCE + 4), 8, "the next quad, one row lower");
}
