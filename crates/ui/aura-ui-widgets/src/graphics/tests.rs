use super::*;

// ── detect() ─────────────────────────────────────────────────────────────

fn env<'a>(map: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    |name| {
        map.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_string())
    }
}

const KITTY: [(&str, &str); 2] = [("KITTY_WINDOW_ID", "1"), ("TERM", "xterm-kitty")];

#[test]
fn detect_override_kitty_forces_on() {
    assert_eq!(
        detect(env(&[("AURA_IMAGE_PROTOCOL", "kitty")])),
        Protocol::Kitty
    );
}

#[test]
fn detect_override_kitty_wins_inside_multiplexers() {
    for mux in [
        ("TMUX", "/tmp/tmux-1000/default,1234,0"),
        ("ZELLIJ", "0"),
        ("STY", "1234.pts-0.host"),
        ("TERM", "screen-256color"),
    ] {
        assert_eq!(
            detect(env(&[("AURA_IMAGE_PROTOCOL", "kitty"), mux])),
            Protocol::Kitty,
            "{mux:?}"
        );
    }
}

#[test]
fn detect_override_none_forces_off() {
    assert_eq!(
        detect(env(&[("AURA_IMAGE_PROTOCOL", "none"), KITTY[0], KITTY[1]])),
        Protocol::None
    );
}

#[test]
fn detect_multiplexers_disable_even_inside_kitty() {
    for mux in [
        ("TMUX", "/tmp/tmux-1000/default,1234,0"),
        ("ZELLIJ", "0"),
        ("STY", "1234.pts-0.host"),
    ] {
        assert_eq!(
            detect(env(&[mux, KITTY[0], KITTY[1]])),
            Protocol::None,
            "{mux:?}"
        );
        assert_eq!(
            detect(env(&[mux, ("TERM_PROGRAM", "WezTerm")])),
            Protocol::None,
            "{mux:?}"
        );
    }
}

#[test]
fn detect_multiplexer_term_disables() {
    for term in ["screen", "screen-256color", "tmux-256color"] {
        assert_eq!(
            detect(env(&[
                ("TERM", term),
                ("KITTY_WINDOW_ID", "1"),
                ("TERM_PROGRAM", "ghostty")
            ])),
            Protocol::None,
            "{term}"
        );
    }
}

#[test]
fn detect_kitty_needs_window_id_and_term() {
    assert_eq!(detect(env(&KITTY)), Protocol::Kitty);
    assert_eq!(
        detect(env(&[
            ("KITTY_WINDOW_ID", "42"),
            ("TERM", "xterm-kitty-direct")
        ])),
        Protocol::Kitty
    );
}

#[test]
fn detect_inherited_kitty_window_id_alone_is_none() {
    // A terminal launched from inside kitty inherits KITTY_WINDOW_ID.
    assert_eq!(detect(env(&[("KITTY_WINDOW_ID", "42")])), Protocol::None);
    assert_eq!(
        detect(env(&[
            ("KITTY_WINDOW_ID", "42"),
            ("TERM", "xterm-256color")
        ])),
        Protocol::None
    );
}

#[test]
fn detect_wezterm() {
    assert_eq!(detect(env(&[("TERM_PROGRAM", "WezTerm")])), Protocol::Kitty);
}

#[test]
fn detect_ghostty_term_program() {
    assert_eq!(detect(env(&[("TERM_PROGRAM", "ghostty")])), Protocol::Kitty);
}

#[test]
fn detect_ghostty_term() {
    assert_eq!(detect(env(&[("TERM", "xterm-ghostty")])), Protocol::Kitty);
}

#[test]
fn detect_unverified_terminals_are_none() {
    for vars in [
        [("TERM", "foot")],
        [("TERM", "foot-extra")],
        [("TERM_PROGRAM", "rio")],
        [("KONSOLE_VERSION", "230000")],
    ] {
        assert_eq!(detect(env(&vars)), Protocol::None, "{vars:?}");
    }
}

#[test]
fn detect_unknown_terminal_is_none() {
    assert_eq!(
        detect(env(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "iTerm.app")
        ])),
        Protocol::None
    );
}

#[test]
fn detect_no_env_at_all_is_none() {
    assert_eq!(detect(|_| None), Protocol::None);
}

// ── move_cursor() ────────────────────────────────────────────────────────

#[test]
fn move_cursor_produces_cup_sequence() {
    // CSI row ; col H — 1-indexed
    assert_eq!(move_cursor(1, 1), b"\x1b[1;1H");
    assert_eq!(move_cursor(5, 10), b"\x1b[10;5H");
}

// ── delete() ─────────────────────────────────────────────────────────────

#[test]
fn delete_names_the_placement() {
    assert_eq!(delete(7, 2), b"\x1b_Ga=d,d=i,i=7,p=2,q=2\x1b\\");
}

// ── place() ──────────────────────────────────────────────────────────────

#[test]
fn place_names_the_placement() {
    assert_eq!(place(3, 1, 20, 6), b"\x1b_Ga=p,i=3,p=1,c=20,r=6,q=2\x1b\\");
}

// ── transmit() ───────────────────────────────────────────────────────────

/// Build a minimal valid PNG for `width × height` solid red pixels.
fn red_png(width: u32, height: u32) -> Vec<u8> {
    use image::{ImageFormat, RgbaImage};
    let img = RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png).unwrap();
    buf.into_inner()
}

#[test]
fn transmit_single_chunk_for_small_image() {
    let png = red_png(2, 2);
    let out = transmit(1, 1, &png, 4, 2);
    let s = String::from_utf8(out).unwrap();
    // Must start with the first-chunk header, placement id included.
    assert!(
        s.starts_with("\x1b_Ga=T,i=1,p=1,f=100,c=4,r=2,m=0,q=2;"),
        "{s:?}"
    );
    // Must end with ST.
    assert!(s.ends_with("\x1b\\"), "{s:?}");
    // No continuation frames.
    assert_eq!(s.matches("\x1b_G").count(), 1, "expected exactly one chunk");
}

#[test]
fn transmit_multi_chunk_for_large_image() {
    // transmit() doesn't validate PNG content — use varied bytes that
    // base64-encode to more than CHUNK_SIZE bytes.
    let n = CHUNK_SIZE * 3 / 4 + 2;
    let png: Vec<u8> = (0..n)
        .map(|i| u8::try_from(i % 251).unwrap_or_default())
        .collect();
    let encoded_len = base64::engine::general_purpose::STANDARD.encode(&png).len();
    assert!(
        encoded_len > CHUNK_SIZE,
        "test requires encoded_len > CHUNK_SIZE but got {encoded_len}"
    );

    let out = transmit(2, 3, &png, 10, 5);
    let s = String::from_utf8(out).unwrap();

    let chunks: Vec<&str> = s.split("\x1b_G").skip(1).collect();
    let n = chunks.len();
    assert!(n >= 2, "expected multiple chunks, got {n}");

    // First chunk carries display parameters and m=1.
    assert!(
        chunks[0].starts_with("a=T,i=2,p=3,f=100,c=10,r=5,m=1"),
        "{}",
        chunks[0]
    );

    // All intermediate chunks carry m=1.
    for chunk in &chunks[1..n - 1] {
        assert!(chunk.starts_with("m=1"), "intermediate chunk: {chunk}");
    }

    // Last chunk carries m=0.
    assert!(
        chunks[n - 1].starts_with("m=0"),
        "last chunk: {}",
        chunks[n - 1]
    );

    // Reconstruct the base64 payload and verify it round-trips.
    let payload: String = chunks
        .iter()
        .map(|c| {
            let semi = c.find(';').unwrap();
            let st = c.rfind("\x1b\\").unwrap();
            &c[semi + 1..st]
        })
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&payload)
        .unwrap();
    assert_eq!(decoded, png);
}

// ── GraphicsState::sync() ─────────────────────────────────────────────────

fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn at<'a>(image_id: u32, slot: PlacementSlot, rect: Rect, png: &'a [u8]) -> Placement<'a> {
    Placement {
        image_id,
        slot,
        rect,
        png,
    }
}

fn chat<'a>(image_id: u32, rect: Rect, png: &'a [u8]) -> Placement<'a> {
    at(image_id, PlacementSlot::ChatThumbnail, rect, png)
}

fn sync_to_string(gs: &mut GraphicsState, wanted: &[Placement<'_>]) -> String {
    let mut out = Vec::new();
    gs.sync(&mut out, wanted).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn slots_have_distinct_nonzero_placement_ids() {
    let ids: HashSet<u32> = [
        PlacementSlot::ChatThumbnail,
        PlacementSlot::PendingThumbnail,
        PlacementSlot::Detail,
    ]
    .iter()
    .map(|slot| slot.placement_id())
    .collect();
    assert_eq!(ids.len(), 3);
    assert!(!ids.contains(&0));
}

#[test]
fn sync_no_op_under_protocol_none() {
    let mut gs = GraphicsState::new(Protocol::None);
    let png = red_png(2, 2);
    let s = sync_to_string(&mut gs, &[chat(1, rect(0, 0, 10, 4), &png)]);
    assert!(s.is_empty(), "Protocol::None must write nothing");
}

#[test]
fn sync_new_image_transmits_with_placement_id() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let s = sync_to_string(&mut gs, &[chat(42, rect(0, 0, 10, 4), &png)]);
    assert!(s.contains("a=T,i=42,p=1,"), "transmit names p=1: {s:?}");
    assert!(gs.transmitted.contains(&42));
}

#[test]
fn sync_restores_the_cursor_around_output() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let s = sync_to_string(&mut gs, &[chat(1, rect(3, 4, 10, 4), &png)]);
    assert!(s.starts_with("\x1b7\x1b[5;4H"), "{s:?}");
    assert!(s.ends_with("\x1b8"), "{s:?}");
}

#[test]
fn sync_unchanged_image_is_noop() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let r = rect(0, 0, 10, 4);
    sync_to_string(&mut gs, &[chat(1, r, &png)]);
    let s = sync_to_string(&mut gs, &[chat(1, r, &png)]);
    assert!(s.is_empty(), "no-op frame must write nothing, got {s:?}");
}

#[test]
fn sync_moved_image_replaces_its_placement_without_delete() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    sync_to_string(&mut gs, &[chat(5, rect(0, 0, 10, 4), &png)]);
    let s = sync_to_string(&mut gs, &[chat(5, rect(20, 5, 10, 4), &png)]);
    // Same image, same placement id: the terminal moves the placement.
    assert!(
        s.contains("\x1b[6;21H\x1b_Ga=p,i=5,p=1,c=10,r=4,q=2"),
        "moved image must re-place p=1: {s:?}"
    );
    assert!(!s.contains("a=T"), "moved image must NOT retransmit: {s:?}");
    assert!(
        !s.contains("a=d"),
        "re-placing must not need a delete: {s:?}"
    );
    assert_eq!(gs.placed.len(), 1);
}

#[test]
fn sync_same_image_in_two_slots_is_two_stable_placements() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let wanted = [
        chat(7, rect(0, 0, 10, 4), &png),
        at(7, PlacementSlot::Detail, rect(5, 2, 40, 20), &png),
    ];
    let s = sync_to_string(&mut gs, &wanted);
    assert_eq!(s.matches("a=T").count(), 1, "data is sent once: {s:?}");
    assert!(s.contains("a=T,i=7,p=1,"), "{s:?}");
    assert!(s.contains("a=p,i=7,p=3,c=40,r=20"), "{s:?}");
    assert_eq!(gs.placed.len(), 2);

    // Both are where they were: the next frame writes nothing, instead
    // of the two fighting over one slot every frame.
    let s = sync_to_string(&mut gs, &wanted);
    assert!(s.is_empty(), "{s:?}");

    // Closing the detail view deletes only its placement.
    let s = sync_to_string(&mut gs, &wanted[..1]);
    assert!(s.contains("a=d,d=i,i=7,p=3,"), "{s:?}");
    assert!(!s.contains("p=1"), "{s:?}");
}

#[test]
fn sync_duplicate_placement_keeps_the_first_rect() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let wanted = [
        chat(8, rect(0, 0, 10, 4), &png),
        chat(8, rect(0, 10, 10, 4), &png),
    ];
    sync_to_string(&mut gs, &wanted);
    let s = sync_to_string(&mut gs, &wanted);
    assert!(s.is_empty(), "{s:?}");
    assert_eq!(
        gs.placed.get(&(8, PlacementSlot::ChatThumbnail)),
        Some(&rect(0, 0, 10, 4))
    );
}

#[test]
fn sync_removed_image_is_deleted() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    sync_to_string(&mut gs, &[chat(9, rect(0, 0, 10, 4), &png)]);
    let s = sync_to_string(&mut gs, &[]);
    assert!(s.contains("a=d,d=i,i=9,p=1,"), "{s:?}");
    assert!(gs.placed.is_empty());
}

#[test]
fn sync_empty_wanted_deletes_everything() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    sync_to_string(
        &mut gs,
        &[
            chat(10, rect(0, 0, 5, 3), &png),
            chat(11, rect(10, 0, 5, 3), &png),
        ],
    );
    let s = sync_to_string(&mut gs, &[]);
    assert!(s.contains("i=10,"), "id 10 must be deleted: {s:?}");
    assert!(s.contains("i=11,"), "id 11 must be deleted: {s:?}");
    assert!(gs.placed.is_empty());
}

#[test]
fn sync_skips_empty_rect() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    let s = sync_to_string(&mut gs, &[chat(1, rect(0, 0, 0, 4), &png)]);
    assert!(s.is_empty(), "empty rect must produce no output");
}

#[test]
fn sync_placement_shrunk_to_empty_is_deleted() {
    let mut gs = GraphicsState::new(Protocol::Kitty);
    let png = red_png(2, 2);
    sync_to_string(&mut gs, &[chat(1, rect(0, 0, 4, 4), &png)]);
    let s = sync_to_string(&mut gs, &[chat(1, rect(0, 0, 0, 4), &png)]);
    assert!(s.contains("a=d,d=i,i=1,p=1,"), "{s:?}");
}
