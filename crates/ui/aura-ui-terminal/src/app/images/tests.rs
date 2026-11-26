
use super::*;

#[test]
fn media_type_comes_from_the_signature() {
    assert_eq!(media_type_of(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
    assert_eq!(media_type_of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
    assert_eq!(media_type_of(b"GIF89a.."), Some("image/gif"));
    assert_eq!(media_type_of(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
    assert_eq!(media_type_of(b"plain text"), None);
}

#[test]
fn pasted_paths_are_unquoted_and_unescaped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("my shot.PNG");
    std::fs::write(&path, b"x").unwrap();
    let shown = path.display().to_string();

    assert_eq!(
        pasted_image_path(&format!("'{shown}'\n")),
        Some(path.clone())
    );
    assert_eq!(
        pasted_image_path(&shown.replace(' ', "\\ ")),
        Some(path.clone())
    );
    assert_eq!(
        pasted_image_path(&format!("file://{}", shown.replace(' ', "%20"))),
        Some(path.clone())
    );
    assert_eq!(pasted_image_path(&format!("{shown}\nmore")), None);
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"x").unwrap();
    assert_eq!(pasted_image_path(&text.display().to_string()), None);
}

#[test]
fn pending_placements_cover_the_picture_not_its_caption() {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let mut app = App::new();
    app.attach_image(png.get_ref()).unwrap();
    let preview = Arc::clone(&app.input.images[0].preview);
    let cols = preview.thumbnail_width();
    let rows = u16::try_from(preview.thumbnail().len()).unwrap();
    // The tile is wider than the picture for its caption, and a row
    // taller for the caption itself.
    let tile = ratatui::layout::Rect {
        x: 3,
        y: 5,
        width: cols + 6,
        height: rows + 1,
    };
    app.set_pending_image_areas(vec![(tile, 1)]);
    let placements = app.visible_image_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(
        placements[0].1,
        ratatui::layout::Rect {
            width: cols,
            height: rows,
            ..tile
        }
    );
}
