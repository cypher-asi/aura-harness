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
fn rows_are_copied_out_as_a_texture_of_the_atlas_lacks_them() {
    let mut atlas = Atlas::new();
    let stride = Atlas::WIDTH as usize * 4;
    let all = atlas.take_rows(None);
    assert_eq!((all.size, all.rows()), ((Atlas::WIDTH, 512), 0..512));
    assert!(all.whole(), "a new texture gets every row");
    let size = Some(atlas.size());
    assert!(atlas.take_rows(size).rows().is_empty(), "nothing new");

    atlas.shape(key(1), || Some(mask(10, 14, 200)));
    let first = atlas.take_rows(size);
    assert_eq!(first.rows(), 0..14);
    assert_eq!(&first.pixels[..4], &[255, 255, 255, 200]);
    assert_eq!(first.pixels.len(), 14 * stride);

    // The frame with `first` was replaced before the texture took
    // its rows, so the next one carries both.
    atlas.shape(key(2), || Some(mask(10, 30, 9)));
    let second = atlas.take_rows(size);
    assert_eq!(second.rows(), 16..46);
    let both = atlas.merge_rows(second, &first);
    assert_eq!(both.rows(), 0..46);
    assert_eq!(
        &both.pixels[16 * stride..16 * stride + 4],
        &[255, 255, 255, 9]
    );

    // Grown: every row again, whatever was missed before.
    atlas.shape(key(3), || Some(mask(10, 600, 1)));
    let grown = atlas.take_rows(size);
    assert_eq!(grown.size, (Atlas::WIDTH, 1024));
    assert!(grown.whole());
    assert!(atlas.merge_rows(grown, &both).whole());
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

#[test]
fn prose_is_as_wide_on_any_thread_and_as_drawn() {
    const WORDS: [&str; 5] = ["illicit", "mammoth ", "naïve", "日本語", "Wi-Fi"];
    let measure = || {
        WORDS
            .iter()
            .map(|word| prose_cells(word, Modifier::ITALIC))
            .collect::<Vec<f32>>()
    };
    let here = measure();
    let there = std::thread::spawn(measure).join().expect("measured");
    assert_eq!(here, there);

    let face = Face {
        family: Family::Sans,
        bold: false,
        italic: true,
    };
    let mut text = TextSystem::new();
    let size = theme::prose_size(100.0);
    let drawn = text.run_width(WORDS[1], face, size) / theme::cell_size(100.0).width;
    assert!(
        (drawn - here[1]).abs() < f32::EPSILON,
        "{drawn} vs {}",
        here[1]
    );
}
