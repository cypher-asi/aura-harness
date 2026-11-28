//! Scrolling the channel's chat to a message picked on HISTORY, and
//! lighting it up there for a moment.
//!
//! Picking a message notes it as the chat's jump. A member's chat on
//! screen goes back to the channel's first, which loads the
//! coordinator's history again. The jump is resolved as the chat lays
//! its rows out (`renderer::chat_panel`), where every block's row is
//! known however far up it is: the chat scrolls so the message's first
//! row sits near the top and the message is lit for [`CHAT_FLASH`]. A
//! jump to a message the chat no longer holds (cut from the front of a
//! long chat) is given up after [`JUMP_WAIT`], with a notice.

use super::HistoryKey;
use crate::app::{App, NotificationType};
use crate::components::Message;
use std::time::{Duration, Instant};

/// How long a message the chat jumped to stays lit.
pub(crate) const CHAT_FLASH: Duration = Duration::from_millis(1_500);

/// How long a jump waits for its message to be in the chat.
const JUMP_WAIT: Duration = Duration::from_secs(10);

/// Rows of the chat kept above a message jumped to, for context.
const ROWS_ABOVE: usize = 1;

impl App {
    /// Scroll the channel's chat to entry `index` of HISTORY, going back
    /// from a member's chat to the channel's first.
    pub(in crate::app) fn jump_to_channel_history(&mut self, index: usize) {
        let Some(key) = self.channel_history().get(index).map(|entry| entry.key) else {
            return;
        };
        self.close_member_chat();
        self.show_channel_chat();
        self.channels.history.jump = Some((key, Instant::now()));
        self.channels.history.flash = None;
        self.request_redraw();
    }

    /// The message the chat is to scroll to, if a jump waits.
    #[must_use]
    pub(crate) fn chat_jump(&self) -> Option<HistoryKey> {
        self.channels.history.jump.map(|(key, _)| key)
    }

    /// The chat block of the waiting jump's message, once the channel's
    /// chat is on screen and holds it: the id of its message, or of its
    /// post's.
    #[must_use]
    pub(crate) fn chat_jump_block(&self) -> Option<u64> {
        self.coordinated_channel()?;
        match self.chat_jump()? {
            HistoryKey::Post { seq } => self
                .channels
                .coordinator_posts(&self.active_agent_id)
                .iter()
                .any(|post| post.seq == seq)
                .then(|| crate::renderer::post_block_key(seq)),
            HistoryKey::Message { at_ms, digest } => self
                .messages
                .iter()
                .find(|message| is_prompt_at(message, at_ms, digest))
                .map(|message| message.version().0),
        }
    }

    /// Scroll the chat, `total` rows long and `height` rows tall, so
    /// `row`, the first row of block `block`, sits near the top, and
    /// light the block up. The jump is done.
    pub(crate) fn jump_chat_to_row(
        &mut self,
        block: u64,
        row: usize,
        (total, height): (usize, usize),
    ) {
        let bottom_start = total.saturating_sub(height);
        let top = row.saturating_sub(ROWS_ABOVE).min(bottom_start);
        self.scroll_offset = bottom_start - top;
        // Counted as laid out, so the scroll isn't moved for rows that
        // seem new.
        self.chat.line_count = total;
        self.channels.history.jump = None;
        self.channels.history.flash = Some((block, Instant::now()));
        self.request_redraw();
    }

    /// Give up a jump that has waited past [`JUMP_WAIT`] at `now` for its
    /// message, saying so.
    pub(crate) fn expire_chat_jump(&mut self, now: Instant) {
        let Some((_, asked)) = self.channels.history.jump else {
            return;
        };
        if now.saturating_duration_since(asked) >= JUMP_WAIT {
            self.channels.history.jump = None;
            self.notifications.current = Some((
                "That message is no longer in the chat".to_string(),
                NotificationType::Warning,
            ));
        }
    }

    /// The chat block lit up after a jump, while it is at `now`.
    #[must_use]
    pub(crate) fn chat_flash_block(&self, now: Instant) -> Option<u64> {
        let (block, since) = self.channels.history.flash?;
        (now.saturating_duration_since(since) < CHAT_FLASH).then_some(block)
    }

    /// Whether a jump waits or a message is lit, so frames keep coming.
    #[must_use]
    pub(crate) fn chat_jump_animating(&self) -> bool {
        self.channels.history.jump.is_some() || self.chat_flash_block(Instant::now()).is_some()
    }
}

/// Whether `message` is the prompt sent at `at_ms` with text hashing to
/// `digest`.
fn is_prompt_at(message: &Message, at_ms: u64, digest: u64) -> bool {
    message.timestamp_ms() == at_ms
        && super::is_user_prompt(message)
        && super::text_digest(message.content()) == digest
}
