//! Kitty terminal graphics protocol support.
//!
//! # Overview
//!
//! The Kitty graphics protocol transmits image data over APC escape sequences
//! (`\x1b_G…\x1b\\`). The terminal places images at the cursor position in
//! cell coordinates, compositing them over text.
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
//! underneath; if the detection is wrong (e.g. a proxied environment where
//! `KITTY_WINDOW_ID` is set but the outer terminal is not kitty), the user
//! sees stray ignored bytes at worst and the half-block image at best.
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

// ── Protocol detection ────────────────────────────────────────────────────────

/// Which image protocol the terminal supports, detected once at startup.
///
/// Invariant: the value is determined by [`detect`] and never changes at
/// runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    /// Kitty graphics protocol (APC `\x1b_G…\x1b\\`).
    Kitty,
    /// No graphics protocol; use the half-block Unicode fallback.
    None,
}

impl Protocol {
    /// Whether this is [`Protocol::Kitty`].
    pub(crate) fn is_kitty(self) -> bool {
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
/// 3. Under `TMUX` → `None` (tmux does not propagate `KITTY_WINDOW_ID`
///    and would require passthrough escapes we don't implement).
/// 4. `KITTY_WINDOW_ID` set → `Kitty`.
/// 5. `TERM_PROGRAM` is `WezTerm`, `ghostty`, or `rio` → `Kitty`.
/// 6. `TERM` is `xterm-ghostty` or contains `foot` → `Kitty`.
/// 7. `KONSOLE_VERSION` ≥ `220400` → `Kitty`.
/// 8. Everything else → `None`.
pub(crate) fn detect(vars: impl Fn(&str) -> Option<String>) -> Protocol {
    // Explicit overrides always win.
    match vars("AURA_IMAGE_PROTOCOL").as_deref() {
        Some("kitty") => return Protocol::Kitty,
        Some("none") => return Protocol::None,
        _ => {}
    }

    // tmux does not propagate KITTY_WINDOW_ID and requires passthrough
    // escapes we don't implement; default to None inside tmux.
    if vars("TMUX").is_some() {
        return Protocol::None;
    }

    // kitty sets this in every window it creates.
    if vars("KITTY_WINDOW_ID").is_some() {
        return Protocol::Kitty;
    }

    // TERM_PROGRAM-identified terminals known to implement the protocol.
    match vars("TERM_PROGRAM").as_deref() {
        Some("WezTerm") | Some("ghostty") | Some("rio") => return Protocol::Kitty,
        _ => {}
    }

    // TERM-based identification.
    match vars("TERM").as_deref() {
        Some("xterm-ghostty") => return Protocol::Kitty,
        Some(term) if term.contains("foot") => return Protocol::Kitty,
        _ => {}
    }

    // Konsole >= 22.04 (version string "220400" or higher).
    if let Some(version_str) = vars("KONSOLE_VERSION") {
        if version_str
            .trim()
            .parse::<u64>()
            .is_ok_and(|v| v >= 220_400)
        {
            return Protocol::Kitty;
        }
    }

    Protocol::None
}

// ── Wire-format helpers ───────────────────────────────────────────────────────

/// Move the terminal cursor to absolute cell (`col`, `row`), 1-indexed.
///
/// Kitty places images at the current cursor position, so this must be
/// emitted before every transmit/place command.
pub(crate) fn move_cursor(col: u16, row: u16) -> Vec<u8> {
    // CSI row ; col H  (CUP — Cursor Position, 1-indexed)
    format!("\x1b[{};{}H", row, col).into_bytes()
}

/// Delete the placement for `id` from the screen (by image id).
///
/// Uses `a=d,d=i` (delete by image id), `q=2` (suppress response).
pub(crate) fn delete(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=i,i={id},q=2\x1b\\").into_bytes()
}

/// Re-place a previously transmitted image at the current cursor position,
/// sized to `cols × rows` terminal cells. No PNG bytes are sent.
///
/// Uses `a=p` (place), `q=2` (suppress response).
pub(crate) fn place(id: u32, cols: u16, rows: u16) -> Vec<u8> {
    format!("\x1b_Ga=p,i={id},c={cols},r={rows},q=2\x1b\\").into_bytes()
}

/// Transmit `png` bytes and immediately display the image at the current
/// cursor position, sized to `cols × rows` terminal cells.
///
/// The PNG is base64-encoded and split into chunks of at most [`CHUNK_SIZE`]
/// bytes. All but the last chunk carry `m=1` (more data follows); the last
/// carries `m=0`. The first chunk also carries the display parameters
/// (`a=T`, `f=100` for PNG passthrough, `c`, `r`, `q=2`).
///
/// Returns `Err` only if the base64 encoding or the `Write` calls fail.
pub(crate) fn transmit(id: u32, png: &[u8], cols: u16, rows: u16) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(CHUNK_SIZE)
        .map(|c| {
            // SAFETY: base64 output is ASCII; every chunk is valid UTF-8.
            std::str::from_utf8(c).unwrap_or_default()
        })
        .collect();

    let mut out = Vec::new();
    let n = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < n { 1 } else { 0 };
        if i == 0 {
            // First chunk: include display parameters.
            out.extend_from_slice(
                format!("\x1b_Ga=T,i={id},f=100,c={cols},r={rows},m={more},q=2;{chunk}\x1b\\")
                    .as_bytes(),
            );
        } else {
            // Continuation chunks: id + chunk flag only.
            out.extend_from_slice(format!("\x1b_Gm={more};{chunk}\x1b\\").as_bytes());
        }
    }
    out
}

// ── GraphicsState ─────────────────────────────────────────────────────────────

/// Tracks which images are currently displayed via the Kitty graphics
/// protocol and reconciles frame-to-frame differences.
///
/// Invariants:
/// - `transmitted` contains every image id whose data the terminal has
///   already received. Data is never re-sent for an id in this set.
/// - `placed` contains the `Rect` of every image visible in the **previous**
///   frame. On the next call to [`GraphicsState::sync`] it is diffed against
///   `wanted` to decide what to delete, re-place, or transmit+display.
/// - Both sets are empty when [`Protocol::None`]; `sync` is a no-op.
pub(crate) struct GraphicsState {
    protocol: Protocol,
    /// Image ids whose PNG data the terminal has already received.
    transmitted: HashSet<u32>,
    /// Previous frame's placements: id → absolute cell `Rect`.
    placed: HashMap<u32, Rect>,
}

impl GraphicsState {
    /// Create a new state using the given protocol.
    pub(crate) fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            transmitted: HashSet::new(),
            placed: HashMap::new(),
        }
    }

    /// Protocol this state was created for.
    pub(crate) fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Reconcile the desired image placements for this frame against the
    /// previous frame, writing only the minimum set of Kitty escapes to
    /// `out`.
    ///
    /// `wanted` is a list of `(id, rect, png)` triples:
    /// - `id` — stable per-image identifier (from [`ImagePreview::id`]).
    /// - `rect` — absolute terminal cell rectangle where the image should
    ///   appear (Kitty scales the image to fill it).
    /// - `png` — PNG bytes to transmit on first display; ignored for ids
    ///   already in `transmitted`.
    ///
    /// Actions taken:
    /// - **Delete**: an id in `placed` but not in `wanted` → `a=d`.
    /// - **Re-place**: an id in both but with a different `Rect` → `a=p`
    ///   (no data re-sent).
    /// - **Transmit + display** (first time): an id not in `transmitted` →
    ///   `a=T` with the PNG payload.
    /// - **No-op**: an id already placed at the same `Rect`.
    ///
    /// When `Protocol::None` this method returns immediately without writing
    /// anything.
    pub(crate) fn sync(
        &mut self,
        out: &mut impl Write,
        wanted: &[(u32, Rect, &[u8])],
    ) -> io::Result<()> {
        if !self.protocol.is_kitty() {
            return Ok(());
        }

        // Collect the ids we want visible this frame.
        let wanted_ids: HashSet<u32> = wanted.iter().map(|&(id, _, _)| id).collect();

        // Delete anything placed last frame that is no longer wanted.
        let to_delete: Vec<u32> = self
            .placed
            .keys()
            .copied()
            .filter(|id| !wanted_ids.contains(id))
            .collect();
        for id in to_delete {
            out.write_all(&delete(id))?;
            self.placed.remove(&id);
        }

        // Transmit new images and (re-)place moved/resized ones.
        for &(id, rect, png) in wanted {
            if rect.is_empty() {
                continue;
            }
            let prev = self.placed.get(&id).copied();
            let already_transmitted = self.transmitted.contains(&id);

            if already_transmitted {
                match prev {
                    Some(r) if r == rect => {
                        // Unchanged — nothing to do.
                    }
                    _ => {
                        // Position or size changed: re-place without resending
                        // data.
                        out.write_all(&move_cursor(rect.x + 1, rect.y + 1))?;
                        out.write_all(&place(id, rect.width, rect.height))?;
                        self.placed.insert(id, rect);
                    }
                }
            } else {
                // First time this image appears: transmit + display.
                out.write_all(&move_cursor(rect.x + 1, rect.y + 1))?;
                out.write_all(&transmit(id, png, rect.width, rect.height))?;
                self.transmitted.insert(id);
                self.placed.insert(id, rect);
            }
        }

        out.flush()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── detect() ─────────────────────────────────────────────────────────────

    fn env<'a>(map: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            map.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn detect_override_kitty_forces_on() {
        assert_eq!(
            detect(env(&[("AURA_IMAGE_PROTOCOL", "kitty")])),
            Protocol::Kitty
        );
    }

    #[test]
    fn detect_override_none_forces_off() {
        assert_eq!(
            detect(env(&[
                ("AURA_IMAGE_PROTOCOL", "none"),
                ("KITTY_WINDOW_ID", "1")
            ])),
            Protocol::None
        );
    }

    #[test]
    fn detect_tmux_disables_even_with_kitty_window_id() {
        assert_eq!(
            detect(env(&[
                ("TMUX", "/tmp/tmux-1000/default,1234,0"),
                ("KITTY_WINDOW_ID", "1")
            ])),
            Protocol::None
        );
    }

    #[test]
    fn detect_kitty_window_id() {
        assert_eq!(detect(env(&[("KITTY_WINDOW_ID", "42")])), Protocol::Kitty);
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
    fn detect_foot() {
        assert_eq!(detect(env(&[("TERM", "foot")])), Protocol::Kitty);
        assert_eq!(detect(env(&[("TERM", "foot-extra")])), Protocol::Kitty);
    }

    #[test]
    fn detect_rio() {
        assert_eq!(detect(env(&[("TERM_PROGRAM", "rio")])), Protocol::Kitty);
    }

    #[test]
    fn detect_konsole_new_enough() {
        assert_eq!(
            detect(env(&[("KONSOLE_VERSION", "220400")])),
            Protocol::Kitty
        );
        assert_eq!(
            detect(env(&[("KONSOLE_VERSION", "230000")])),
            Protocol::Kitty
        );
    }

    #[test]
    fn detect_konsole_too_old() {
        assert_eq!(
            detect(env(&[("KONSOLE_VERSION", "210000")])),
            Protocol::None
        );
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
    fn delete_produces_correct_escape() {
        let d = delete(7);
        assert_eq!(d, b"\x1b_Ga=d,d=i,i=7,q=2\x1b\\");
    }

    // ── place() ──────────────────────────────────────────────────────────────

    #[test]
    fn place_produces_correct_escape() {
        let p = place(3, 20, 6);
        assert_eq!(p, b"\x1b_Ga=p,i=3,c=20,r=6,q=2\x1b\\");
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
        let out = transmit(1, &png, 4, 2);
        let s = String::from_utf8(out).unwrap();
        // Must start with the first-chunk header.
        assert!(
            s.starts_with("\x1b_Ga=T,i=1,f=100,c=4,r=2,m=0,q=2;"),
            "{s:?}"
        );
        // Must end with ST.
        assert!(s.ends_with("\x1b\\"), "{s:?}");
        // m=0 on the only chunk.
        assert!(s.contains("m=0"), "{s:?}");
        // No continuation frames.
        assert_eq!(s.matches("\x1b_G").count(), 1, "expected exactly one chunk");
    }

    #[test]
    fn transmit_multi_chunk_for_large_image() {
        // transmit() doesn't validate PNG content — use varied bytes that
        // base64-encode to more than CHUNK_SIZE bytes.
        // ceil(CHUNK_SIZE * 3 / 4) + 1 raw bytes → encoded length > CHUNK_SIZE.
        let n = CHUNK_SIZE * 3 / 4 + 2;
        // `i % 251` is always in 0..251, which fits in u8; the cast is safe.
        #[allow(clippy::cast_possible_truncation)]
        let png: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        let encoded_len = base64::engine::general_purpose::STANDARD.encode(&png).len();
        assert!(
            encoded_len > CHUNK_SIZE,
            "test requires encoded_len > CHUNK_SIZE but got {encoded_len}"
        );

        let out = transmit(2, &png, 10, 5);
        let s = String::from_utf8(out).unwrap();

        let chunks: Vec<&str> = s.split("\x1b_G").skip(1).collect();
        let n = chunks.len();
        assert!(n >= 2, "expected multiple chunks, got {n}");

        // First chunk carries display parameters and m=1.
        assert!(
            chunks[0].starts_with("a=T,i=2,f=100,c=10,r=5,m=1"),
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

    fn tiny_png() -> Vec<u8> {
        red_png(2, 2)
    }

    #[test]
    fn sync_no_op_under_protocol_none() {
        let mut gs = GraphicsState::new(Protocol::None);
        let png = tiny_png();
        let mut out = Vec::new();
        gs.sync(&mut out, &[(1, rect(0, 0, 10, 4), &png)]).unwrap();
        assert!(out.is_empty(), "Protocol::None must write nothing");
    }

    #[test]
    fn sync_new_image_transmits() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        let mut out = Vec::new();
        gs.sync(&mut out, &[(42, rect(0, 0, 10, 4), &png)]).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("a=T"), "new image must use a=T: {s:?}");
        assert!(s.contains("i=42"), "id must appear: {s:?}");
        assert!(gs.transmitted.contains(&42));
    }

    #[test]
    fn sync_unchanged_image_is_noop() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        let r = rect(0, 0, 10, 4);
        // First frame: transmit.
        let mut out = Vec::new();
        gs.sync(&mut out, &[(1, r, &png)]).unwrap();
        // Second frame: same position/size — must produce no additional output.
        let mut out2 = Vec::new();
        gs.sync(&mut out2, &[(1, r, &png)]).unwrap();
        // The only write should be the flush; an empty flush writes nothing.
        assert!(
            out2.is_empty(),
            "no-op frame must write nothing, got {out2:?}"
        );
    }

    #[test]
    fn sync_moved_image_replaces_without_retransmitting() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        // First frame: transmit at (0,0).
        let mut out = Vec::new();
        gs.sync(&mut out, &[(5, rect(0, 0, 10, 4), &png)]).unwrap();
        // Second frame: same id at a different position.
        let mut out2 = Vec::new();
        gs.sync(&mut out2, &[(5, rect(20, 5, 10, 4), &png)])
            .unwrap();
        let s = String::from_utf8(out2).unwrap();
        // Must use a=p (place) not a=T (transmit).
        assert!(s.contains("a=p"), "moved image must use a=p: {s:?}");
        assert!(!s.contains("a=T"), "moved image must NOT retransmit: {s:?}");
    }

    #[test]
    fn sync_removed_image_is_deleted() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        // First frame.
        let mut out = Vec::new();
        gs.sync(&mut out, &[(9, rect(0, 0, 10, 4), &png)]).unwrap();
        // Second frame: image is gone.
        let mut out2 = Vec::new();
        gs.sync(&mut out2, &[]).unwrap();
        let s = String::from_utf8(out2).unwrap();
        assert!(s.contains("a=d"), "removed image must be deleted: {s:?}");
        assert!(s.contains("i=9"), "delete must name the id: {s:?}");
        assert!(!gs.placed.contains_key(&9));
    }

    #[test]
    fn sync_empty_wanted_deletes_everything() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        // Place two images.
        let mut out = Vec::new();
        gs.sync(
            &mut out,
            &[(10, rect(0, 0, 5, 3), &png), (11, rect(10, 0, 5, 3), &png)],
        )
        .unwrap();
        // Clear everything.
        let mut out2 = Vec::new();
        gs.sync(&mut out2, &[]).unwrap();
        let s = String::from_utf8(out2).unwrap();
        assert!(s.contains("i=10"), "id 10 must be deleted: {s:?}");
        assert!(s.contains("i=11"), "id 11 must be deleted: {s:?}");
        assert!(gs.placed.is_empty());
    }

    #[test]
    fn sync_skips_empty_rect() {
        let mut gs = GraphicsState::new(Protocol::Kitty);
        let png = tiny_png();
        let mut out = Vec::new();
        // A zero-width rect is empty and should be skipped.
        gs.sync(&mut out, &[(1, rect(0, 0, 0, 4), &png)]).unwrap();
        assert!(out.is_empty(), "empty rect must produce no output");
    }

    // Silence the unused import warning that the HashMap bring-in can cause
    // in some configurations; it's used by the env() helper above.
    #[allow(dead_code)]
    fn _use_hashmap(_: HashMap<(), ()>) {}
}
