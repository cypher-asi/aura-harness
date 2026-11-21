//! Automaton-builtin knobs (the dev-loop retry-note truncation budget).

// ---------------------------------------------------------------------------
// Compile-time constants
// ---------------------------------------------------------------------------

/// Maximum bytes of `build_retry_note` injected back into the next
/// task attempt by the dev-loop automaton. Larger notes are
/// head/tail-truncated so the bootstrap user message stays within the
/// Cloudflare body cap.
pub const DEV_LOOP_RETRY_NOTE_MAX_BYTES: usize = 12_000;

// ---------------------------------------------------------------------------
// Config struct
// ---------------------------------------------------------------------------

/// Automaton-layer config (all compile-time today).
#[derive(Debug, Clone, Copy)]
pub struct AutomatonConfig {
    /// See [`DEV_LOOP_RETRY_NOTE_MAX_BYTES`].
    pub dev_loop_retry_note_max_bytes: usize,
}

impl AutomatonConfig {
    /// Compile-time defaults.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            dev_loop_retry_note_max_bytes: DEV_LOOP_RETRY_NOTE_MAX_BYTES,
        }
    }
}

impl Default for AutomatonConfig {
    fn default() -> Self {
        Self::defaults()
    }
}
