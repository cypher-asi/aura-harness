//! How the terminal reports keys and pastes: bracketed paste, the kitty
//! keyboard protocol, and the legacy control codes it changes.

use crossterm::{
    event::{
        EnableBracketedPaste, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::supports_keyboard_enhancement,
};
use std::io;

/// Have the terminal wrap pastes so they arrive as one [`Event::Paste`](crossterm::event::Event::Paste):
/// a pasted image path can be attached, and pasted newlines no longer send
/// the message line by line. Terminals without it paste as typed keys.
pub(super) fn enable_bracketed_paste(out: &mut impl io::Write) {
    if let Err(e) = execute!(out, EnableBracketedPaste) {
        tracing::debug!(error = %e, "Bracketed paste is not available");
    }
}

/// Whether the terminal answers the kitty keyboard protocol query. Legacy
/// and Windows terminals say no or fail; either way the UI starts with
/// Alt+Enter and Ctrl+J as its line-break keys, so an error is no reason
/// to stop.
pub(super) fn keyboard_enhancement_supported() -> bool {
    supports_keyboard_enhancement().unwrap_or_else(|e| {
        tracing::debug!(error = %e, "Keyboard enhancement query failed");
        false
    })
}

/// Ask the terminal to report modified keys unambiguously, so Shift+Enter
/// arrives apart from Enter. Only the disambiguation flag is pushed: event
/// types or every key as an escape code would change how plain typing
/// arrives. Returns whether the flags were pushed.
pub(super) fn enable_keyboard_enhancement(out: &mut impl io::Write) -> bool {
    let flags = PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES);
    match execute!(out, flags) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "Keyboard enhancement is not available");
            false
        }
    }
}

/// Report Ctrl+[, Ctrl+M and Ctrl+I as the Esc, Enter and Tab they are
/// without the kitty keyboard protocol. With [`enable_keyboard_enhancement`]
/// on, the terminal sends them as the letter with Control, and the UI's
/// keys are written for the legacy codes. Only plain Control is mapped, so
/// Ctrl+Shift+M and the like stay what they are.
pub(super) fn legacy_control_key(key: KeyEvent) -> KeyEvent {
    if key.modifiers != KeyModifiers::CONTROL {
        return key;
    }
    let code = match key.code {
        KeyCode::Char('[') => KeyCode::Esc,
        KeyCode::Char('m') => KeyCode::Enter,
        KeyCode::Char('i') => KeyCode::Tab,
        _ => return key,
    };
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        ..key
    }
}
