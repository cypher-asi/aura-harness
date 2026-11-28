//! The HISTORY tab of an open channel's side panel: every message the
//! user sent in the channel, oldest first and newest at the bottom, as
//! the chat has them, each with its time.
//!
//! The user's messages are the prompts typed to the coordinator (user
//! messages of its chat that are not shell commands or mail from other
//! agents) and the user's posts, which is what a message to members
//! becomes. A prompt and a post with the same text within
//! [`SAME_MESSAGE_MS`] are one message sent both ways, listed once.
//!
//! The coordinator's messages are read while its chat is on screen and
//! kept per channel, so HISTORY still lists them while a member's chat
//! is open. The list is rebuilt only when the chat or the posts change.
//!
//! The highlight follows the newest message while it is on it, as the
//! chat follows new output. ↑↓, PgUp/PgDn and Home/End move it; Enter or
//! a click scrolls the chat to the message (see `jump`). The wheel
//! scrolls the list and leaves the highlight where it is, until a key or
//! click moves it again. Long messages show a summary (see
//! `summaries`).

mod jump;
mod summaries;

#[cfg(test)]
pub(crate) use jump::CHAT_FLASH;

use crate::app::list_state::ListState;
use crate::app::processes::SIDE_PAGE;
use crate::app::{contains, follow_selection, App};
use crate::components::{Message, MessageRole};
use crate::events::{ChannelPostAuthor, ChannelPostView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Instant;

/// How close in time, in milliseconds, a prompt and a post with the same
/// text are taken for one message.
const SAME_MESSAGE_MS: u64 = 5_000;

/// Where a message of the history is in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HistoryKey {
    /// A prompt to the coordinator: when it was sent and a hash of its
    /// text, which outlast the chat being loaded again
    Message { at_ms: u64, digest: u64 },
    /// A post of the channel
    Post { seq: u64 },
}

/// One message of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    pub(crate) key: HistoryKey,
    /// What the user sent
    pub(crate) text: String,
    /// `text` on one line, its runs of whitespace made single spaces
    pub(crate) line: String,
    /// When it was sent, in Unix milliseconds
    pub(crate) at_ms: u64,
}

impl HistoryEntry {
    fn new(key: HistoryKey, text: &str, at_ms: u64) -> Self {
        Self {
            key,
            text: text.to_string(),
            line: one_line(text),
            at_ms,
        }
    }
}

/// What a chat's messages were when the history last read them: how
/// many, and the ids of the first and the last.
type MessagesSeen = (usize, u64, u64);

/// What HISTORY knows and where it is.
#[derive(Debug, Default)]
pub(crate) struct ChannelHistory {
    /// The coordinator's prompts of each channel, by channel id, with
    /// what its chat was when they were read
    prompts: HashMap<String, (MessagesSeen, Vec<HistoryEntry>)>,
    /// What the list was built from: the channel, its prompts and its
    /// posts' count and last `seq`
    built: Option<(String, MessagesSeen, (usize, u64))>,
    /// The entries, the highlighted one and the first row shown
    list: ListState<HistoryEntry>,
    /// Whether the wheel scrolled the list away from the highlight
    detached: bool,
    /// Rows shown at once, as of the last frame
    visible: usize,
    /// Where the rows were drawn this frame
    rows_area: Option<Rect>,
    /// Summaries of long messages, by summary key
    summaries: HashMap<String, String>,
    /// Summary keys asked for in this run; each is asked for once
    requested: HashSet<String>,
    /// The message the chat is to scroll to, and when it was asked
    jump: Option<(HistoryKey, Instant)>,
    /// The chat block lit up after a jump, and since when
    flash: Option<(u64, Instant)>,
}

/// `text` on one line: every run of whitespace one space.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A hash of `text`, stable for the run.
pub(crate) fn text_digest(text: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Whether `message` is one the user typed to the agent: a user message
/// that is not a shell command, a post or mail from another agent, and
/// says something.
pub(crate) fn is_user_prompt(message: &Message) -> bool {
    message.role() == MessageRole::User
        && message.shell_run().is_none()
        && message.author().is_none()
        && message.inbox_fold().is_none()
        && !message.content().trim().is_empty()
}

/// The prompts of a chat's `messages`, oldest first.
pub(crate) fn prompt_entries(messages: &VecDeque<Message>) -> Vec<HistoryEntry> {
    messages
        .iter()
        .filter(|message| is_user_prompt(message))
        .map(|message| {
            let key = HistoryKey::Message {
                at_ms: message.timestamp_ms(),
                digest: text_digest(message.content()),
            };
            HistoryEntry::new(key, message.content(), message.timestamp_ms())
        })
        .collect()
}

/// The user's posts among `posts`, oldest first.
pub(crate) fn post_entries(posts: &[ChannelPostView]) -> Vec<HistoryEntry> {
    posts
        .iter()
        .filter(|post| post.author == ChannelPostAuthor::User && !post.text.trim().is_empty())
        .map(|post| HistoryEntry::new(HistoryKey::Post { seq: post.seq }, &post.text, post.at_ms))
        .collect()
}

/// `prompts` and `posts` in one list by time, prompts first on equal
/// times, each prompt sent as a post too listed once.
pub(crate) fn merge_entries(
    prompts: Vec<HistoryEntry>,
    posts: Vec<HistoryEntry>,
) -> Vec<HistoryEntry> {
    let posts: Vec<HistoryEntry> = posts
        .into_iter()
        .filter(|post| {
            !prompts.iter().any(|prompt| {
                prompt.line == post.line && prompt.at_ms.abs_diff(post.at_ms) <= SAME_MESSAGE_MS
            })
        })
        .collect();
    let mut all = prompts;
    all.extend(posts);
    // Stable, so prompts stay before posts of the same time and each
    // keeps its own order.
    all.sort_by_key(|entry| entry.at_ms);
    all
}

/// What `messages` are now, to tell whether they changed.
fn messages_seen(messages: &VecDeque<Message>) -> MessagesSeen {
    let id = |message: Option<&Message>| message.map_or(0, |m| m.version().0);
    (messages.len(), id(messages.front()), id(messages.back()))
}

impl ChannelHistory {
    /// Show `entries`, keeping the highlight on the newest when it was
    /// there, else on the same message.
    fn set_entries(&mut self, entries: Vec<HistoryEntry>) {
        let list = &mut self.list;
        let on_newest = list.selected + 1 >= list.len();
        let selected = list.selected_item().map(|entry| entry.key);
        list.items = entries;
        let last = list.len().saturating_sub(1);
        list.selected = if on_newest {
            last
        } else {
            selected
                .and_then(|key| list.items.iter().position(|entry| entry.key == key))
                .unwrap_or(list.selected)
                .min(last)
        };
    }

    /// Highlight entry `index` (clamped) and let the list follow it.
    fn select(&mut self, index: usize) {
        self.list.selected = index.min(self.list.len().saturating_sub(1));
        self.detached = false;
    }
}

impl App {
    /// Bring HISTORY up to date with the open channel's chat and posts.
    /// Cheap when nothing changed.
    pub(crate) fn refresh_channel_history(&mut self) {
        let Some(channel_id) = self.channels.open.clone() else {
            return;
        };
        if self.coordinated_channel().is_some() && !self.chat_loading() {
            let seen = messages_seen(&self.messages);
            let history = &mut self.channels.history;
            if history.prompts.get(&channel_id).map(|(s, _)| *s) != Some(seen) {
                let prompts = prompt_entries(&self.messages);
                history.prompts.insert(channel_id.clone(), (seen, prompts));
            }
        }
        let posts = self
            .channels
            .posts
            .get(&channel_id)
            .map_or(&[][..], Vec::as_slice);
        let posts_seen = (posts.len(), posts.last().map_or(0, |post| post.seq));
        let history = &mut self.channels.history;
        let (seen, prompts) = history
            .prompts
            .get(&channel_id)
            .cloned()
            .unwrap_or_default();
        let built = Some((channel_id, seen, posts_seen));
        if history.built == built {
            return;
        }
        let other_channel = history.built.as_ref().map(|b| &b.0) != built.as_ref().map(|b| &b.0);
        if other_channel {
            // Another channel's list opens on its newest message.
            history.list = ListState::default();
            history.detached = false;
            history.jump = None;
            history.flash = None;
        }
        history.built = built;
        history.set_entries(merge_entries(prompts, post_entries(posts)));
    }

    /// The messages HISTORY lists, oldest first.
    #[must_use]
    pub(crate) fn channel_history(&self) -> &[HistoryEntry] {
        &self.channels.history.list.items
    }

    /// The highlighted entry of HISTORY.
    #[must_use]
    pub(crate) const fn selected_channel_history(&self) -> usize {
        self.channels.history.list.selected
    }

    /// Keep HISTORY's scroll inside a list `visible` rows tall, on the
    /// highlight unless the wheel moved it off, returning the first row.
    pub(crate) fn sync_channel_history_scroll(&mut self, visible: usize) -> usize {
        let history = &mut self.channels.history;
        let visible = visible.max(1);
        history.visible = visible;
        let list = &mut history.list;
        list.scroll = if history.detached {
            list.scroll.min(list.len().saturating_sub(visible))
        } else {
            follow_selection(list.scroll, list.selected, visible, list.len())
        };
        list.scroll
    }

    /// Record where HISTORY's rows were drawn this frame.
    pub(crate) fn set_channel_history_rows_area(&mut self, area: Option<Rect>) {
        self.channels.history.rows_area = area;
    }

    /// Where HISTORY's rows were drawn last frame.
    #[cfg(test)]
    pub(crate) const fn channel_history_rows_area(&self) -> Option<Rect> {
        self.channels.history.rows_area
    }

    /// First row of HISTORY shown.
    #[cfg(test)]
    pub(crate) const fn channel_history_scroll(&self) -> usize {
        self.channels.history.list.scroll
    }

    /// Entry of HISTORY drawn at a screen cell.
    pub(in crate::app) fn channel_history_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let history = &self.channels.history;
        let rows = history
            .rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = history.list.scroll + usize::from(row - rows.y);
        (index < history.list.len()).then_some(index)
    }

    /// Highlight entry `index` of HISTORY, clamped.
    pub(in crate::app) fn select_channel_history(&mut self, index: usize) {
        self.refresh_channel_history();
        self.channels.history.select(index);
    }

    /// Move HISTORY's highlight `delta` rows.
    fn move_channel_history(&mut self, delta: isize) {
        let index = self
            .channels
            .history
            .list
            .selected
            .saturating_add_signed(delta);
        self.select_channel_history(index);
    }

    /// The wheel over HISTORY: scroll the list `delta` rows, leaving the
    /// highlight where it is.
    pub(in crate::app) fn wheel_channel_history(&mut self, delta: isize) {
        let history = &mut self.channels.history;
        let last = history.list.len().saturating_sub(history.visible.max(1));
        history.list.scroll = history.list.scroll.saturating_add_signed(delta).min(last);
        history.detached = true;
    }

    /// Keys of HISTORY: ↑↓, PgUp/PgDn and Home/End move the highlight,
    /// Enter scrolls the chat to its message.
    pub(in crate::app) fn handle_channel_history_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_channel_history(-1),
            KeyCode::Down => self.move_channel_history(1),
            KeyCode::PageUp => self.move_channel_history(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_history(SIDE_PAGE),
            KeyCode::Home => self.select_channel_history(0),
            KeyCode::End => self.select_channel_history(usize::MAX),
            KeyCode::Enter => self.jump_to_channel_history(self.selected_channel_history()),
            _ => {}
        }
    }

    /// A click on HISTORY: highlight the entry under it and scroll the
    /// chat to its message.
    pub(in crate::app) fn click_channel_history(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_history_row_at(column, row) {
            self.select_channel_history(index);
            self.jump_to_channel_history(index);
        }
    }
}

#[cfg(test)]
mod tests;
