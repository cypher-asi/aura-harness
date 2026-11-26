use super::cells::octant_cell;
use super::*;
use ratatui::symbols::pixel::OCTANTS;

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
