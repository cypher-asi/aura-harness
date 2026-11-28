//! The open channel's HISTORY side tab: every message the user sent in
//! the channel, newest first, each on one row with when it was sent.
//!
//! A message is listed as it was sent, on one line, while it is at most
//! [`MESSAGE_SUMMARY_THRESHOLD`] characters. A longer one is listed by a
//! one-line summary the session has a small model write once and keeps
//! (`UiEvent::SummarizeMessages`, answered with
//! `UiCommand::MessageSummaries`); until it comes, or when it never does,
//! the message is cut at a word with `…`. Summaries are only asked for
//! while HISTORY is on screen, each message once.
//!
//! The list is read from the channel's chat (the coordinator's chat with
//! the channel's posts in it): the user's prompts there, and the posts
//! the user wrote mentioning members. A prompt with the text of a user
//! post written just before it is that post sent on to the coordinator
//! too, so it is listed once. While a member's own chat is on screen the
//! list stays as the channel's chat last left it.
//!
//! Enter or a click on a row scrolls the channel's chat to that message
//! and lights it for a moment (see `jump`); the wheel scrolls the rows
//! without moving the highlight.

mod jump;
mod keys;

#[cfg(test)]
pub(crate) use jump::JUMP_HIGHLIGHT;

use crate::app::App;
use crate::components::{Message, MessageRole};
use crate::events::{ChannelPostAuthor, ChannelPostView, MESSAGE_SUMMARY_THRESHOLD};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

/// How far apart, in milliseconds, a user post and a prompt with its text
/// may be and still be one message sent to members and the coordinator.
const SAME_SEND_MS: u64 = 60_000;

/// A message the user sent in the channel, as HISTORY lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    /// Where it is in the channel's chat
    pub(crate) target: JumpTarget,
    /// The message, as sent
    pub(crate) text: String,
    /// When it was sent, in Unix milliseconds
    pub(crate) at_ms: u64,
}

/// A message of the channel's chat to scroll to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JumpTarget {
    /// A post of the channel, by its `seq`
    Post(u64),
    /// A prompt of the coordinator's chat: the user's message with this
    /// text sent nearest this time. Kept by content rather than by the
    /// message's id, which changes when the chat loads again.
    Message { text: String, at_ms: u64 },
}

/// What HISTORY shows and where it is.
#[derive(Debug, Default)]
pub(in crate::app) struct HistoryState {
    /// The user's messages, newest first
    entries: Vec<HistoryEntry>,
    /// What `entries` was read from, to read it again only on a change
    signature: Option<u64>,
    /// Highlighted row
    selected: usize,
    /// First row shown
    scroll: usize,
    /// Whether the wheel scrolled the rows away from the highlight; they
    /// stay put until the highlight moves
    wheel_scrolled: bool,
    /// Where the rows were drawn this frame
    rows_area: Option<Rect>,
    /// Summaries of long messages, by the message as sent
    summaries: HashMap<String, String>,
    /// Long messages a summary was asked for
    asked: HashSet<String>,
    /// The jump to make once the channel's chat is drawn
    jump: Option<JumpTarget>,
    /// The chat block lit by the last jump, and since when
    flash: Option<(u64, Instant)>,
}

impl HistoryState {
    /// Where the rows were drawn is only good for the frame that drew
    /// them.
    pub(super) fn forget_rows_area(&mut self) {
        self.rows_area = None;
    }

    /// Forget the list and where it was, for another channel. Summaries
    /// are kept: they are keyed by the message.
    pub(super) fn reset(&mut self) {
        self.entries.clear();
        self.signature = None;
        self.selected = 0;
        self.scroll = 0;
        self.wheel_scrolled = false;
        self.rows_area = None;
        self.jump = None;
        self.flash = None;
    }
}

/// The user's messages among `messages` and `posts`, newest first.
pub(crate) fn user_entries(messages: &[&Message], posts: &[ChannelPostView]) -> Vec<HistoryEntry> {
    let user_posts: Vec<&ChannelPostView> = posts
        .iter()
        .filter(|post| post.author == ChannelPostAuthor::User && !post.text.trim().is_empty())
        .collect();
    let is_post_sent_on = |text: &str, at_ms: u64| {
        user_posts.iter().any(|post| {
            post.text == text && post.at_ms <= at_ms && at_ms - post.at_ms <= SAME_SEND_MS
        })
    };
    let mut entries: Vec<HistoryEntry> = user_posts
        .iter()
        .map(|post| HistoryEntry {
            target: JumpTarget::Post(post.seq),
            text: post.text.clone(),
            at_ms: post.at_ms,
        })
        .collect();
    entries.extend(
        messages
            .iter()
            .filter(|message| is_user_prompt(message))
            .filter(|message| !is_post_sent_on(message.content(), message.timestamp_ms()))
            .map(|message| HistoryEntry {
                target: JumpTarget::Message {
                    text: message.content().to_string(),
                    at_ms: message.timestamp_ms(),
                },
                text: message.content().to_string(),
                at_ms: message.timestamp_ms(),
            }),
    );
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.at_ms));
    entries
}

/// Whether `message` is one the user typed into the chat: not a shell
/// command, a post, or another agent's message delivered as the user's.
pub(crate) fn is_user_prompt(message: &Message) -> bool {
    message.role() == MessageRole::User
        && message.shell_run().is_none()
        && message.author().is_none()
        && message.inbox_fold().is_none()
        && !message.content().trim().is_empty()
}

/// `text` on one line: its runs of whitespace made one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `text` is long enough to be listed by a summary.
#[must_use]
pub(crate) fn is_long(text: &str) -> bool {
    one_line(text).chars().count() > MESSAGE_SUMMARY_THRESHOLD
}

/// What HISTORY lists `text` as: itself on one line when short, else its
/// `summary`, else itself cut at a word under the threshold with `…`.
#[must_use]
pub(crate) fn history_label(text: &str, summary: Option<&str>) -> String {
    let line = one_line(text);
    if line.chars().count() <= MESSAGE_SUMMARY_THRESHOLD {
        return line;
    }
    if let Some(summary) = summary.map(one_line).filter(|s| !s.is_empty()) {
        return summary;
    }
    let cut: String = line.chars().take(MESSAGE_SUMMARY_THRESHOLD).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > 0 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end_matches([' ', ',', ';', ':', '-', '.']))
}

impl App {
    /// The messages HISTORY lists, newest first, read again from the
    /// channel's chat when it is on screen and changed.
    pub(crate) fn channel_history(&mut self) -> &[HistoryEntry] {
        self.refresh_channel_history();
        &self.channels.history.entries
    }

    /// HISTORY's label of `entry` ([`history_label`]).
    #[must_use]
    pub(crate) fn channel_history_label(&self, entry: &HistoryEntry) -> String {
        let summary = self.channels.history.summaries.get(&entry.text);
        history_label(&entry.text, summary.map(String::as_str))
    }

    /// Read the list again from the channel's chat when it is the chat on
    /// screen, loaded, and changed since it was last read.
    fn refresh_channel_history(&mut self) {
        let Some(channel_id) = self.coordinated_channel().map(|c| c.id.clone()) else {
            return;
        };
        if self.chat_loading() {
            return;
        }
        let posts = self.channels.coordinator_posts(&self.active_agent_id);
        let mut hasher = std::hash::DefaultHasher::new();
        channel_id.hash(&mut hasher);
        self.messages.len().hash(&mut hasher);
        self.messages
            .front()
            .map(Message::version)
            .hash(&mut hasher);
        self.messages.back().map(Message::version).hash(&mut hasher);
        posts.len().hash(&mut hasher);
        posts
            .last()
            .map(|post| (post.seq, post.at_ms))
            .hash(&mut hasher);
        let signature = Some(hasher.finish());
        if self.channels.history.signature == signature {
            return;
        }
        let messages: Vec<&Message> = self.messages.iter().collect();
        let entries = user_entries(&messages, posts);
        let history = &mut self.channels.history;
        history.signature = signature;
        history.entries = entries;
        history.selected = history
            .selected
            .min(history.entries.len().saturating_sub(1));
    }

    /// Take the summaries the session wrote of long messages.
    pub(in crate::app) fn add_message_summaries(
        &mut self,
        summaries: Vec<crate::events::MessageSummary>,
    ) {
        let history = &mut self.channels.history;
        for summary in summaries {
            history.summaries.insert(summary.text, summary.summary);
        }
    }

    /// While HISTORY is on screen, ask the session once for a summary of
    /// each long message listed that has none. Called every tick.
    pub(in crate::app) fn ask_message_summaries(&mut self) {
        if !self.channel_open() || self.side_tab() != crate::app::SidePanelTab::ChannelHistory {
            return;
        }
        let Some(channel_id) = self.open_channel().map(|c| c.id.clone()) else {
            return;
        };
        self.refresh_channel_history();
        let history = &self.channels.history;
        let mut texts: Vec<String> = Vec::new();
        for entry in &history.entries {
            let wanted = is_long(&entry.text)
                && !history.summaries.contains_key(&entry.text)
                && !history.asked.contains(&entry.text)
                && !texts.contains(&entry.text);
            if wanted {
                texts.push(entry.text.clone());
            }
        }
        if texts.is_empty() {
            return;
        }
        let event = crate::events::UiEvent::SummarizeMessages {
            channel_id,
            texts: texts.clone(),
        };
        if self.try_send_event(event) {
            self.channels.history.asked.extend(texts);
        }
    }
}

#[cfg(test)]
mod tests;
