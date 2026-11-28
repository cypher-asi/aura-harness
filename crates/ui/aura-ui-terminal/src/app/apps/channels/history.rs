//! The open channel's HISTORY side tab: every message the user posted in
//! the channel, newest first, each one a jump back to it in the chat.
//!
//! The items are the open channel's posts by the user, read from the
//! posts the chat shows. A post longer than [`POST_SUMMARY_MIN_CHARS`]
//! shows the one-line summary the session sends for it
//! ([`App::set_post_summaries`]), else it is cut at a word.
//!
//! Each item takes [`HISTORY_ROWS`] rows. The keys move the highlight and
//! the list follows it. The wheel scrolls the list and leaves the
//! highlight where it is. Enter, or a click on an item, jumps the chat to
//! the message ([`App::request_chat_jump`]), going back from a member's
//! chat to the channel's first; the focus stays on the list.
//!
//! [`POST_SUMMARY_MIN_CHARS`]: crate::events::POST_SUMMARY_MIN_CHARS

use crate::app::processes::SIDE_PAGE;
use crate::app::{contains, follow_selection, App};
use crate::events::{ChannelPostAuthor, ChannelPostView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::collections::HashMap;

/// Rows each item takes: the message, then when it was sent.
pub(crate) const HISTORY_ROWS: usize = 2;

/// Where the HISTORY tab is, and the summaries it shows.
#[derive(Debug, Default)]
pub(crate) struct HistoryState {
    /// Highlighted item, counted from the newest
    selected: usize,
    /// First item on screen
    scroll: usize,
    /// Whether the list keeps the highlight in view: set by the keys,
    /// cleared by the wheel, which scrolls past it
    follow: bool,
    /// Where the items' rows were drawn this frame
    rows_area: Option<Rect>,
    /// Summaries of long posts, by channel id, then post `seq`
    summaries: HashMap<String, HashMap<u64, String>>,
}

/// One item of the HISTORY tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryItem {
    /// The post's place in its channel
    pub seq: u64,
    /// What the user wrote
    pub text: String,
    /// Its summary, once the session sent one
    pub summary: Option<String>,
    /// When it was posted, in Unix milliseconds
    pub at_ms: u64,
}

impl HistoryState {
    /// Start a channel at its newest message.
    pub(super) fn reset(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.follow = true;
        self.rows_area = None;
    }

    pub(super) fn forget_area(&mut self) {
        self.rows_area = None;
    }
}

impl App {
    /// The HISTORY tab's items: the user's posts in the open channel,
    /// newest first.
    #[must_use]
    pub(crate) fn channel_history(&self) -> Vec<HistoryItem> {
        let Some(channel) = self.open_channel() else {
            return Vec::new();
        };
        let summaries = self.channels.history.summaries.get(&channel.id);
        self.channels
            .posts_of(&channel.id)
            .iter()
            .rev()
            .filter(|post| post.author == ChannelPostAuthor::User)
            .map(|post| item(post, summaries))
            .collect()
    }

    /// Highlighted item of the HISTORY tab.
    #[must_use]
    pub(crate) const fn selected_history_item(&self) -> usize {
        self.channels.history.selected
    }

    /// Keep the HISTORY tab's first item inside its `len` items, `visible`
    /// of them fitting, following the highlight after a key; returns it.
    pub(crate) fn sync_channel_history_scroll(&mut self, len: usize, visible: usize) -> usize {
        let state = &mut self.channels.history;
        state.selected = state.selected.min(len.saturating_sub(1));
        state.scroll = if state.follow {
            follow_selection(state.scroll, state.selected, visible, len)
        } else {
            state.scroll.min(len.saturating_sub(visible))
        };
        state.scroll
    }

    /// Record where the HISTORY tab's rows were drawn this frame.
    pub(crate) fn set_channel_history_rows(&mut self, area: Option<Rect>) {
        self.channels.history.rows_area = area;
    }

    /// Keep `summaries` of channel `channel_id`'s posts, by `seq`.
    pub(in crate::app) fn set_post_summaries(
        &mut self,
        channel_id: String,
        summaries: Vec<(u64, String)>,
    ) {
        self.channels
            .history
            .summaries
            .entry(channel_id)
            .or_default()
            .extend(summaries);
    }

    /// Item of the HISTORY tab drawn at a screen cell, on either row.
    pub(in crate::app) fn channel_history_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels.history;
        let rows = state
            .rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.scroll + usize::from(row - rows.y) / HISTORY_ROWS;
        (index < self.channel_history().len()).then_some(index)
    }

    /// Scroll the HISTORY tab `delta` items, as the wheel does, leaving
    /// the highlight where it is.
    pub(in crate::app) fn scroll_channel_history(&mut self, delta: isize) {
        let state = &mut self.channels.history;
        state.follow = false;
        state.scroll = state.scroll.saturating_add_signed(delta);
    }

    /// The HISTORY list follows its highlight again, as any key asks.
    pub(in crate::app) fn follow_channel_history(&mut self) {
        self.channels.history.follow = true;
    }

    /// Move the HISTORY highlight `delta` items, the list following it.
    pub(in crate::app) fn move_channel_history(&mut self, delta: isize) {
        let state = &mut self.channels.history;
        state.follow = true;
        state.selected = state.selected.saturating_add_signed(delta);
    }

    /// Clicking an item highlights it and jumps the chat to its message.
    pub(in crate::app) fn click_channel_history(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_history_row_at(column, row) {
            self.channels.history.selected = index;
            self.jump_to_history_item(index);
        }
    }

    /// Keys of the HISTORY tab: the arrows, page and end keys move its
    /// highlight, Enter jumps the chat to the highlighted message.
    pub(in crate::app) fn handle_channel_history_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_channel_history(-1),
            KeyCode::Down => self.move_channel_history(1),
            KeyCode::PageUp => self.move_channel_history(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_history(SIDE_PAGE),
            KeyCode::Home => self.select_channel_history(0),
            KeyCode::End => self.select_channel_history(usize::MAX),
            KeyCode::Enter => self.jump_to_history_item(self.channels.history.selected),
            _ => {}
        }
    }

    /// Highlight item `index` (kept inside the list when drawn).
    pub(in crate::app) fn select_channel_history(&mut self, index: usize) {
        let state = &mut self.channels.history;
        state.follow = true;
        state.selected = index;
    }

    /// Jump the channel's chat to item `index`'s message, from a member's
    /// chat too.
    fn jump_to_history_item(&mut self, index: usize) {
        let Some(seq) = self.channel_history().get(index).map(|item| item.seq) else {
            return;
        };
        self.close_member_chat();
        self.request_chat_jump(crate::renderer::post_block_key(seq));
    }
}

/// `post` as an item, with its summary from `summaries` when it has one.
fn item(post: &ChannelPostView, summaries: Option<&HashMap<u64, String>>) -> HistoryItem {
    HistoryItem {
        seq: post.seq,
        text: post.text.clone(),
        summary: summaries.and_then(|known| known.get(&post.seq)).cloned(),
        at_ms: post.at_ms,
    }
}
