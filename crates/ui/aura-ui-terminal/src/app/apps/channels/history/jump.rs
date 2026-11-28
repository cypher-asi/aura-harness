//! Jumping from a HISTORY row to its message in the channel's chat.
//!
//! The jump is kept until the channel's chat is next laid out, when the
//! renderer knows where every block falls however far back it is
//! (`renderer::chat_panel::jump`): the chat is scrolled to put the
//! message near the top, and the message's block is lit for
//! [`JUMP_HIGHLIGHT`]. A message no longer in the chat says so.

use super::{is_user_prompt, JumpTarget};
use crate::app::{App, NotificationType};
use crate::renderer::post_block_key;
use std::ops::Range;
use std::time::{Duration, Instant};

/// How long the message jumped to stays lit.
pub(crate) const JUMP_HIGHLIGHT: Duration = Duration::from_millis(1500);

/// Rows of context kept above the message jumped to.
const CONTEXT_ROWS: usize = 1;

impl App {
    /// Scroll the channel's chat to the highlighted HISTORY message: back
    /// from a member's chat to the channel's, on CHAT with nothing over
    /// it, then on the next frame that lays the chat out.
    pub(in crate::app) fn jump_to_selected_history(&mut self) {
        let history = &self.channels.history;
        let Some(target) = history
            .entries
            .get(history.selected)
            .map(|e| e.target.clone())
        else {
            return;
        };
        self.close_member_chat();
        self.show_channel_chat();
        self.channels.history.jump = Some(target);
        self.channels.history.flash = None;
        self.ticks.redraw = true;
    }

    /// The jump waiting for the channel's chat to be laid out, taken:
    /// `Some(Some(key))` with the key of the block to scroll to,
    /// `Some(None)` when its message is gone. `None` with no jump, or
    /// while the channel's chat is not the chat on screen.
    pub(crate) fn take_history_jump(&mut self) -> Option<Option<u64>> {
        self.coordinated_channel()?;
        let target = self.channels.history.jump.take()?;
        Some(self.block_key_of(&target))
    }

    /// The key of the chat block `target` is drawn in, if it is there.
    fn block_key_of(&self, target: &JumpTarget) -> Option<u64> {
        match target {
            JumpTarget::Post(seq) => self
                .channels
                .coordinator_posts(&self.active_agent_id)
                .iter()
                .any(|post| post.seq == *seq)
                .then(|| post_block_key(*seq)),
            JumpTarget::Message { text, at_ms } => self
                .messages
                .iter()
                .filter(|message| is_user_prompt(message) && message.content() == text)
                .min_by_key(|message| message.timestamp_ms().abs_diff(*at_ms))
                .map(|message| message.version().0),
        }
    }

    /// Scroll the chat, `total` rows shown `height` at a time, to the
    /// block `key` drawn on `rows`, and light it; returns the first row to
    /// show. A message not found (`rows` is `None`) is said to be gone.
    pub(crate) fn finish_history_jump(
        &mut self,
        key: Option<u64>,
        rows: Option<Range<usize>>,
        (total, height): (usize, usize),
    ) -> Option<usize> {
        let (Some(key), Some(rows)) = (key, rows) else {
            self.notifications.current = Some((
                "That message is no longer in the chat".to_string(),
                NotificationType::Warning,
            ));
            return None;
        };
        let bottom_start = total.saturating_sub(height);
        let first = rows.start.saturating_sub(CONTEXT_ROWS).min(bottom_start);
        self.scroll_offset = bottom_start - first;
        self.channels.history.flash = Some((key, Instant::now()));
        Some(first)
    }

    /// The chat block lit by the last jump, while it still is.
    #[must_use]
    pub(crate) fn history_flash(&self) -> Option<u64> {
        self.channels
            .history
            .flash
            .filter(|(_, since)| since.elapsed() < JUMP_HIGHLIGHT)
            .map(|(key, _)| key)
    }

    /// Whether a block is lit by a jump, so every tick draws until it
    /// goes out.
    pub(in crate::app) fn history_flashing(&self) -> bool {
        self.history_flash().is_some()
    }

    /// Move the start of the jump's light `by` back, as if that long had
    /// passed.
    #[cfg(test)]
    pub(crate) fn age_history_flash(&mut self, by: Duration) {
        if let Some((_, since)) = &mut self.channels.history.flash {
            *since = since.checked_sub(by).unwrap_or(*since);
        }
    }
}
