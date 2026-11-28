//! The messages the user typed to a channel's coordinator, on HISTORY.
//!
//! A message that mentions no member stays in the coordinator's
//! conversation and never becomes a post, so HISTORY reads those from the
//! coordinator's chat (on screen, or held while a member's chat is): its
//! user messages, less shell commands, agent steps and mail other agents
//! sent through the inbox. Each is keyed by [`prompt_key`], under which
//! the session sends its summary, and jumps the chat to its block.
//!
//! A message that mentions members and the coordinator is a post and a
//! message of the conversation both; it is listed once, as the post.

use super::{HistoryItem, HistoryKind};
use crate::app::App;
use crate::components::{Message, MessageRole};
use crate::events::{is_typed_prompt, prompt_key};
use std::collections::{HashMap, VecDeque};

/// How far apart, in milliseconds, a post and a typed message with the
/// same text may be and still be one message sent both ways.
const SAME_MESSAGE_MS: u64 = 10_000;

/// Whether `message` is one the user typed to the coordinator.
pub(crate) fn is_typed(message: &Message) -> bool {
    message.role() == MessageRole::User
        && message.shell_run().is_none()
        && message.activity_step().is_none()
        && message.author().is_none()
        && message.inbox_fold().is_none()
        && is_typed_prompt(message.content())
}

/// The typed messages among `messages`, oldest first, with their
/// summaries from `summaries`.
pub(super) fn typed_items(
    messages: &VecDeque<Message>,
    summaries: Option<&HashMap<u64, String>>,
) -> Vec<HistoryItem> {
    messages
        .iter()
        .filter(|message| is_typed(message))
        .map(|message| {
            let key = prompt_key(message.content());
            HistoryItem {
                seq: key,
                kind: HistoryKind::Typed,
                text: message.content().to_string(),
                summary: summaries.and_then(|known| known.get(&key)).cloned(),
                at_ms: message.timestamp_ms(),
            }
        })
        .collect()
}

/// `posts` (newest first) and `typed` (oldest first) in one list, newest
/// first, leaving out a typed message that is also one of the posts.
pub(super) fn merge_newest_first(
    posts: Vec<HistoryItem>,
    typed: Vec<HistoryItem>,
) -> Vec<HistoryItem> {
    let typed: Vec<HistoryItem> = typed
        .into_iter()
        .filter(|item| !posts.iter().any(|post| is_same_message(post, item)))
        .collect();
    let mut all = posts;
    all.extend(typed);
    // Stable, so items sent at the same moment keep their order.
    all.sort_by_key(|item| std::cmp::Reverse(item.at_ms));
    all
}

/// Whether `post` and `typed` are one message sent both ways.
fn is_same_message(post: &HistoryItem, typed: &HistoryItem) -> bool {
    post.text.trim() == typed.text.trim() && post.at_ms.abs_diff(typed.at_ms) <= SAME_MESSAGE_MS
}

impl App {
    /// The chat block of the typed message keyed `key` sent at `at_ms`,
    /// when the channel's chat is on screen and holds it: the id of its
    /// message, the one nearest `at_ms` when it was sent more than once.
    pub(crate) fn typed_message_block(&self, key: u64, at_ms: u64) -> Option<u64> {
        self.coordinated_channel()?;
        self.messages
            .iter()
            .filter(|message| is_typed(message) && prompt_key(message.content()) == key)
            .min_by_key(|message| message.timestamp_ms().abs_diff(at_ms))
            .map(|message| message.version().0)
    }
}

#[cfg(test)]
mod tests;
