//! Jumping the chat to one of its blocks and flashing it, as the HISTORY
//! tab of a channel does for a message the user sent.
//!
//! A jump is asked for by the block's key ([`App::request_chat_jump`]).
//! The next frame that has the block laid out scrolls the chat so it
//! starts a row below the top, as far back as it is, and lights its rows
//! for [`FLASH`] ([`App::land_chat_jump`]). A jump whose block isn't in
//! the chat (still loading) waits up to [`JUMP_WAIT`], then is dropped.
//!
//! A message the user typed to a channel's coordinator is asked for by
//! its text's key and when it was sent ([`App::request_chat_jump_to_typed`]):
//! its block's key is its message's id, known only once the channel's
//! chat is on screen with it, which going back from a member's chat may
//! take a moment to load.

use super::{App, ChatTab};
use std::time::{Duration, Instant};

/// How long a jump waits for its block to be in the chat.
pub(crate) const JUMP_WAIT: Duration = Duration::from_secs(3);
/// How long a block the chat jumped to stays lit.
pub(crate) const FLASH: Duration = Duration::from_millis(1_200);

/// A jump to a message the user typed, waiting for its block: its text's
/// key, when it was sent, the chat it is in, and when it was asked.
#[derive(Debug, Clone)]
struct TypedJump {
    key: u64,
    at_ms: u64,
    /// The member whose own chat holds it; `None` for the channel's chat
    member: Option<String>,
    asked: Instant,
}

/// Where a jump is.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChatJump {
    /// The block to jump to, and when that was asked
    pending: Option<(u64, Instant)>,
    /// The block jumped to, and when it was reached
    flash: Option<(u64, Instant)>,
    /// A typed message to jump to once its block is known
    typed: Option<TypedJump>,
}

impl App {
    /// Jump the chat to the block keyed `key` on the next frame that has
    /// it, putting the chat tab on screen.
    pub(crate) fn request_chat_jump(&mut self, key: u64) {
        if self.chat_tab() != ChatTab::Chat {
            self.show_chat_tab(ChatTab::Chat);
        }
        self.chat.jump.typed = None;
        self.chat.jump.pending = Some((key, Instant::now()));
        self.request_redraw();
    }

    /// Jump the chat to the message the user typed to the channel's
    /// coordinator keyed `key`, sent at `at_ms`, once the channel's chat
    /// shows it, putting the chat tab on screen.
    pub(crate) fn request_chat_jump_to_typed(&mut self, key: u64, at_ms: u64) {
        self.request_typed_jump(key, at_ms, None);
    }

    /// Jump the chat to the message keyed `key`, sent at `at_ms`, in the
    /// own chat of channel member `member`, once that chat is on screen
    /// and shows it: a `/plan` request in its planner's chat.
    pub(crate) fn request_chat_jump_in_member(&mut self, member: &str, key: u64, at_ms: u64) {
        self.request_typed_jump(key, at_ms, Some(member.to_string()));
    }

    fn request_typed_jump(&mut self, key: u64, at_ms: u64, member: Option<String>) {
        if self.chat_tab() != ChatTab::Chat {
            self.show_chat_tab(ChatTab::Chat);
        }
        self.chat.jump.pending = None;
        self.chat.jump.typed = Some(TypedJump {
            key,
            at_ms,
            member,
            asked: Instant::now(),
        });
        self.request_redraw();
    }

    /// Turn a jump to a typed message into one to its block once the
    /// chat holds it, dropping one that waited too long.
    fn resolve_typed_jump(&mut self) {
        let Some(jump) = self.chat.jump.typed.as_ref() else {
            return;
        };
        let asked = jump.asked;
        let found = self.typed_message_block(jump.key, jump.at_ms, jump.member.as_deref());
        if asked.elapsed() > JUMP_WAIT {
            self.chat.jump.typed = None;
        } else if let Some(block) = found {
            self.chat.jump.typed = None;
            self.chat.jump.pending = Some((block, asked));
        }
    }

    /// The block a jump waits for, dropping one that waited too long.
    pub(crate) fn pending_chat_jump(&mut self) -> Option<u64> {
        self.resolve_typed_jump();
        let (key, asked) = self.chat.jump.pending?;
        if asked.elapsed() > JUMP_WAIT {
            self.chat.jump.pending = None;
            return None;
        }
        Some(key)
    }

    /// The jump to block `key` found it at `row` of a chat `total` rows
    /// long shown `visible` at a time: scroll it to a row below the top
    /// and light it.
    pub(crate) fn land_chat_jump(
        &mut self,
        key: u64,
        row: usize,
        (total, visible): (usize, usize),
    ) {
        let bottom_start = total.saturating_sub(visible);
        self.scroll_offset = bottom_start.saturating_sub(row.saturating_sub(1));
        // The chat's length as of this offset, so the next fit keeps it.
        self.chat.line_count = total;
        self.chat.jump.pending = None;
        self.chat.jump.flash = Some((key, Instant::now()));
    }

    /// The block lit after a jump, while it is.
    #[must_use]
    pub(crate) fn chat_flash(&self) -> Option<u64> {
        self.chat
            .jump
            .flash
            .filter(|(_, at)| at.elapsed() < FLASH)
            .map(|(key, _)| key)
    }

    /// Whether a jump waits or a block is lit: the chat redraws each tick.
    #[must_use]
    pub(crate) fn chat_jumping(&self) -> bool {
        self.chat.jump.pending.is_some()
            || self.chat.jump.typed.is_some()
            || self.chat_flash().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ago(by: Duration) -> Instant {
        Instant::now().checked_sub(by).unwrap()
    }

    #[test]
    fn a_landed_jump_lights_its_block_until_the_flash_ends() {
        let mut app = App::new();
        app.request_chat_jump(7);
        assert_eq!(app.pending_chat_jump(), Some(7));
        app.land_chat_jump(7, 40, (100, 20));
        // 40 rows down a 100-row chat: 59 rows up from the end, one row
        // above the block.
        assert_eq!(app.scroll_offset, 41);
        assert_eq!(app.pending_chat_jump(), None);
        assert_eq!(app.chat_flash(), Some(7));
        assert!(app.chat_jumping(), "the chat redraws while lit");
        app.chat.jump.flash = Some((7, ago(FLASH + Duration::from_millis(1))));
        assert_eq!(app.chat_flash(), None);
        assert!(!app.chat_jumping());
    }

    #[test]
    fn a_jump_whose_block_never_shows_is_dropped() {
        let mut app = App::new();
        app.request_chat_jump(9);
        app.chat.jump.pending = Some((9, ago(JUMP_WAIT + Duration::from_millis(1))));
        assert_eq!(app.pending_chat_jump(), None);
        assert!(!app.chat_jumping());
    }

    #[test]
    fn a_block_at_the_top_lands_at_the_top() {
        let mut app = App::new();
        app.land_chat_jump(1, 0, (100, 20));
        assert_eq!(app.scroll_offset, 80);
    }
}
