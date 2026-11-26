//! Kitty terminal graphics protocol support.
//!
//! # Overview
//!
//! The Kitty graphics protocol transmits image data over APC escape sequences
//! (`\x1b_G…\x1b\\`). The terminal places images at the cursor position in
//! cell coordinates, compositing them over text.
//!
//! # Images and placements
//!
//! An *image* (`i=`) is the pixel data the terminal holds; a *placement*
//! (`p=`) is one spot on screen where that image is shown. Every transmit
//! (`a=T`) and place (`a=p`) command here names both. Placing an
//! `(i, p)` pair that already exists *moves* that placement; placing an
//! image without `p` would add a new placement each time and leave the old
//! one on screen. The same image can be shown in several UI locations at
//! once (a chat thumbnail and the full-size view), so each location is its
//! own [`PlacementSlot`] with its own fixed placement id.
//!
//! # Detection
//!
//! Support is detected once at startup from environment variables. We never
//! query the terminal for a response (`q=2` on every command suppresses
//! acknowledgements). This avoids hanging in a terminal that does not
//! implement the query/response round-trip.
//!
//! # Fallback safety
//!
//! Terminals that do not understand APC sequences are required by the VT
//! standard to silently ignore `\x1b_…\x1b\\` blocks. Well-behaved
//! non-Kitty terminals therefore swallow the escapes harmlessly. The
//! existing half-block Unicode rendering remains in the ratatui buffer
//! underneath; if the detection is wrong the user sees stray ignored bytes
//! at worst and the half-block image at best.
//!
//! # Chunking
//!
//! Kitty limits each APC payload to 4096 base64 bytes. Larger images are
//! split across multiple APC chunks: all but the last carry `m=1` (more
//! chunks follow); the last carries `m=0` (or omits it — we include it
//! explicitly for clarity).
//!
//! # Fire-and-forget
//!
//! All commands include `q=2`, which suppresses the terminal's OK/error
//! response. We never read back from the terminal.

use base64::Engine as _;
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

/// Maximum base64 payload bytes per APC chunk.
const CHUNK_SIZE: usize = 4096;

/// DECSC: save the cursor, so drawing images does not move the caret
/// ratatui left in the input box.
const SAVE_CURSOR: &[u8] = b"\x1b7";

/// DECRC: restore the cursor saved by [`SAVE_CURSOR`].
const RESTORE_CURSOR: &[u8] = b"\x1b8";

// ── Protocol detection ────────────────────────────────────────────────────────

/// Which image protocol the terminal supports, detected once at startup.
///
/// Invariant: the value is determined by [`detect`] and never changes at
/// runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Kitty graphics protocol (APC `\x1b_G…\x1b\\`).
    Kitty,
    /// No graphics protocol; use the half-block Unicode fallback.
    None,
}

impl Protocol {
    /// Whether this is [`Protocol::Kitty`].
    pub fn is_kitty(self) -> bool {
        matches!(self, Self::Kitty)
    }
}

/// Detect which protocol the terminal supports from environment variables.
///
/// `vars` is called with a variable name and returns its value, or `None`
/// when it is unset. Using a closure keeps the function pure and testable
/// without touching the real process environment.
///
/// Detection rules (first match wins):
/// 1. `AURA_IMAGE_PROTOCOL=kitty` → `Kitty` (forced on).
/// 2. `AURA_IMAGE_PROTOCOL=none` → `None` (forced off).
/// 3. Inside a terminal multiplexer → `None`: `TMUX`, `ZELLIJ` or `STY`
///    (GNU screen) set, or `TERM` starting with `screen` or `tmux`. They
///    need passthrough escapes we don't implement, and the variables
///    below may describe the terminal *around* the multiplexer.
/// 4. `KITTY_WINDOW_ID` set **and** `TERM` starting with `xterm-kitty` →
///    `Kitty`. `KITTY_WINDOW_ID` alone is not enough: it is inherited by
///    every child process, including other terminals launched from kitty.
/// 5. `TERM_PROGRAM` is `WezTerm` or `ghostty` → `Kitty`.
/// 6. `TERM` is `xterm-ghostty` → `Kitty`.
/// 7. Everything else → `None`.
///
/// Only kitty, WezTerm and Ghostty are auto-detected: they document full
/// support for placement ids (`a=p` with `p=`, which re-placing relies on)
/// and for scaling to a `c`×`r` cell box. Others that implement parts of
/// the protocol (Konsole, Rio, …) are not verified for both and could
/// leave corrupted or stacked images, so they are left out; foot only
/// supports sixel. `AURA_IMAGE_PROTOCOL=kitty` still turns it on for them.
pub fn detect(vars: impl Fn(&str) -> Option<String>) -> Protocol {
    // Explicit overrides always win.
    match vars("AURA_IMAGE_PROTOCOL").as_deref() {
        Some("kitty") => return Protocol::Kitty,
        Some("none") => return Protocol::None,
        _ => {}
    }

    let term = vars("TERM").unwrap_or_default();
    let in_multiplexer = ["TMUX", "ZELLIJ", "STY"]
        .iter()
        .any(|name| vars(name).is_some())
        || term.starts_with("screen")
        || term.starts_with("tmux");
    if in_multiplexer {
        return Protocol::None;
    }

    if vars("KITTY_WINDOW_ID").is_some() && term.starts_with("xterm-kitty") {
        return Protocol::Kitty;
    }

    if matches!(vars("TERM_PROGRAM").as_deref(), Some("WezTerm" | "ghostty")) {
        return Protocol::Kitty;
    }

    if term == "xterm-ghostty" {
        return Protocol::Kitty;
    }

    Protocol::None
}

// ── Placements ────────────────────────────────────────────────────────────────

/// A UI location an image can be shown in. Each slot is a separate Kitty
/// placement, so one image shown in two slots at once is two placements
/// that never replace each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlacementSlot {
    /// A thumbnail under a chat message.
    ChatThumbnail,
    /// A thumbnail in the strip of images waiting in the input.
    PendingThumbnail,
    /// The full-size image in the detail view.
    Detail,
}

impl PlacementSlot {
    /// The Kitty placement id (`p=`) for this slot; never zero, since
    /// `p=0` means "no placement id" to the terminal.
    pub(crate) const fn placement_id(self) -> u32 {
        match self {
            Self::ChatThumbnail => 1,
            Self::PendingThumbnail => 2,
            Self::Detail => 3,
        }
    }
}

/// One image wanted on screen this frame.
#[derive(Debug, Clone, Copy)]
pub struct Placement<'a> {
    /// Stable per-image identifier (from `ImagePreview::id`).
    pub image_id: u32,
    /// Where in the UI the image is shown.
    pub slot: PlacementSlot,
    /// Absolute terminal cells the image fills (Kitty scales it to fit).
    pub rect: Rect,
    /// PNG bytes, sent only the first time the image is shown.
    pub png: &'a [u8],
}

/// Identity of a placement on the terminal: which image, in which slot.
type PlacementKey = (u32, PlacementSlot);

// ── Wire-format helpers ───────────────────────────────────────────────────────

/// Move the terminal cursor to absolute cell (`col`, `row`), 1-indexed.
///
/// Kitty places images at the current cursor position, so this must be
/// emitted before every transmit/place command.
pub(crate) fn move_cursor(col: u16, row: u16) -> Vec<u8> {
    // CSI row ; col H  (CUP — Cursor Position, 1-indexed)
    format!("\x1b[{row};{col}H").into_bytes()
}

/// Delete placement `placement` of image `id` from the screen, keeping
/// the image data so it can be placed again without resending it.
///
/// Uses `a=d,d=i` (delete by image id) narrowed by `p=`, `q=2` (suppress
/// response).
pub(crate) fn delete(id: u32, placement: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=i,i={id},p={placement},q=2\x1b\\").into_bytes()
}

/// Place a previously transmitted image at the current cursor position,
/// sized to `cols × rows` terminal cells. No PNG bytes are sent. An
/// existing placement with the same `id` and `placement` is moved rather
/// than duplicated.
///
/// Uses `a=p` (place), `q=2` (suppress response).
pub(crate) fn place(id: u32, placement: u32, cols: u16, rows: u16) -> Vec<u8> {
    format!("\x1b_Ga=p,i={id},p={placement},c={cols},r={rows},q=2\x1b\\").into_bytes()
}

/// Transmit `png` bytes and immediately display the image at the current
/// cursor position as placement `placement`, sized to `cols × rows`
/// terminal cells.
///
/// The PNG is base64-encoded and split into chunks of at most [`CHUNK_SIZE`]
/// bytes. All but the last chunk carry `m=1` (more data follows); the last
/// carries `m=0`. The first chunk also carries the display parameters
/// (`a=T`, `f=100` for PNG passthrough, `p`, `c`, `r`, `q=2`).
pub(crate) fn transmit(id: u32, placement: u32, png: &[u8], cols: u16, rows: u16) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(CHUNK_SIZE).collect();
    let last = chunks.len().saturating_sub(1);

    let mut out = Vec::with_capacity(encoded.len() + chunks.len() * 64);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i < last);
        if i == 0 {
            // First chunk: include display parameters.
            out.extend_from_slice(
                format!("\x1b_Ga=T,i={id},p={placement},f=100,c={cols},r={rows},m={more},q=2;")
                    .as_bytes(),
            );
        } else {
            // Continuation chunks: chunk flag only.
            out.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

// ── GraphicsState ─────────────────────────────────────────────────────────────

/// Tracks which images are currently displayed via the Kitty graphics
/// protocol and reconciles frame-to-frame differences.
///
/// Invariants:
/// - `transmitted` contains every image id whose data the terminal has
///   already received. Data is per image, not per placement, so it is
///   never re-sent for an id in this set, whichever slot shows it.
/// - `placed` holds the `Rect` of every placement — `(image id, slot)` —
///   visible in the **previous** frame. On the next call to
///   [`GraphicsState::sync`] it is diffed against `wanted` to decide what
///   to delete, re-place, or transmit+display.
/// - Both are empty when [`Protocol::None`]; `sync` is a no-op.
pub struct GraphicsState {
    protocol: Protocol,
    /// Image ids whose PNG data the terminal has already received.
    transmitted: HashSet<u32>,
    /// Previous frame's placements: (image id, slot) → absolute cell `Rect`.
    placed: HashMap<PlacementKey, Rect>,
}

impl GraphicsState {
    /// Create a new state using the given protocol.
    pub fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            transmitted: HashSet::new(),
            placed: HashMap::new(),
        }
    }

    /// Protocol this state was created for.
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Reconcile the desired image placements for this frame against the
    /// previous frame, writing only the minimum set of Kitty escapes to
    /// `out`.
    ///
    /// Actions taken, per `(image id, slot)` placement:
    /// - **Delete**: placed last frame but not wanted now (or wanted with
    ///   an empty rect) → `a=d` for that placement only.
    /// - **Re-place**: image already transmitted but the placement is new
    ///   or its `Rect` changed → `a=p` with the slot's placement id, which
    ///   moves an existing placement instead of adding another. No data is
    ///   re-sent.
    /// - **Transmit + display**: image not transmitted yet → `a=T` with the
    ///   PNG payload.
    /// - **No-op**: placement already at the same `Rect`.
    ///
    /// A placement listed twice in `wanted` is shown at its first rect; one
    /// placement cannot be in two places.
    ///
    /// The cursor is saved before and restored after any output, so the
    /// caret stays where ratatui put it. When `Protocol::None` this method
    /// returns immediately without writing anything.
    pub fn sync(&mut self, out: &mut impl Write, wanted: &[Placement<'_>]) -> io::Result<()> {
        if !self.protocol.is_kitty() {
            return Ok(());
        }

        let mut seen: HashSet<PlacementKey> = HashSet::with_capacity(wanted.len());
        let wanted: Vec<&Placement<'_>> = wanted
            .iter()
            .filter(|p| !p.rect.is_empty() && seen.insert((p.image_id, p.slot)))
            .collect();

        let mut bytes = Vec::new();

        // Delete placements from last frame that are no longer wanted.
        // Sorted so the output is deterministic.
        let mut stale: Vec<PlacementKey> = self
            .placed
            .keys()
            .copied()
            .filter(|key| !seen.contains(key))
            .collect();
        stale.sort_unstable();
        for key in stale {
            bytes.extend_from_slice(&delete(key.0, key.1.placement_id()));
            self.placed.remove(&key);
        }

        for p in wanted {
            let key = (p.image_id, p.slot);
            let rect = p.rect;
            let placement = p.slot.placement_id();
            if !self.transmitted.contains(&p.image_id) {
                bytes.extend_from_slice(&move_cursor(rect.x + 1, rect.y + 1));
                bytes.extend_from_slice(&transmit(
                    p.image_id,
                    placement,
                    p.png,
                    rect.width,
                    rect.height,
                ));
                self.transmitted.insert(p.image_id);
            } else if self.placed.get(&key) != Some(&rect) {
                bytes.extend_from_slice(&move_cursor(rect.x + 1, rect.y + 1));
                bytes.extend_from_slice(&place(p.image_id, placement, rect.width, rect.height));
            } else {
                continue;
            }
            self.placed.insert(key, rect);
        }

        if bytes.is_empty() {
            return Ok(());
        }
        out.write_all(SAVE_CURSOR)?;
        out.write_all(&bytes)?;
        out.write_all(RESTORE_CURSOR)?;
        out.flush()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
