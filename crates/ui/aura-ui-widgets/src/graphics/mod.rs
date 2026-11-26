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
mod tests;
