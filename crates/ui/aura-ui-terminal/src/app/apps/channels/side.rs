//! The open channel's side panel: the MEMBERS, INFO and QUEUE tabs.

use crate::app::processes::SIDE_PAGE;
use crate::app::wheel_views::WheelList;
use crate::app::{contains, App};
use crate::events::MergeQueueView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Rows each member takes on the MEMBERS side tab: its name, then what it
/// works on.
pub(crate) const CHANNEL_MEMBER_ROWS: usize = 2;

impl App {
    /// Highlighted member of the MEMBERS side tab.
    #[must_use]
    pub(crate) const fn selected_channel_member(&self) -> usize {
        self.channels.member_selected
    }

    /// Keep the highlighted member among the `visible_members` that fit,
    /// returning the first member to draw.
    pub(crate) fn sync_channel_members_scroll(&mut self, visible_members: usize) -> usize {
        let members = self.channels.open_members();
        let state = &mut self.channels;
        state.member_scroll = self.panels.wheel.place(
            WheelList::ChannelMembers,
            (state.member_scroll, state.member_selected),
            (visible_members, members),
        );
        state.member_scroll
    }

    /// Record where the MEMBERS side tab's rows were drawn this frame.
    pub(crate) fn set_channel_member_rows_area(&mut self, area: Rect) {
        self.channels.member_rows_area = Some(area);
    }

    /// Drop where the MEMBERS side tab's rows were drawn, for a frame
    /// that may not draw them.
    pub(in crate::app) fn forget_channel_member_rows(&mut self) {
        self.channels.member_rows_area = None;
    }

    /// Member of the MEMBERS side tab drawn at a screen cell, on either of
    /// its rows.
    pub(in crate::app) fn channel_member_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        let rows = state
            .member_rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.member_scroll + usize::from(row - rows.y) / CHANNEL_MEMBER_ROWS;
        (index < state.open_members()).then_some(index)
    }

    /// Highlight member row `index` (clamped). Opening a member's chat
    /// is [`Self::open_channel_member`]'s.
    pub(in crate::app) fn select_channel_member(&mut self, index: usize) {
        self.channels.set_member_selected(index);
    }

    /// Move the MEMBERS highlight `delta` rows, as the wheel does.
    pub(in crate::app) fn move_channel_member(&mut self, delta: isize) {
        let row = self.channels.member_selected.saturating_add_signed(delta);
        self.channels.set_member_selected(row);
    }

    /// Clicking a member row highlights it and opens the member's chat,
    /// or the channel's on the coordinator's row.
    pub(in crate::app) fn click_channel_member(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_member_row_at(column, row) {
            self.open_channel_member(index);
        }
    }

    /// Keys of the MEMBERS side tab, which move its highlight; Enter
    /// opens the highlighted member's chat, as a click does.
    pub(in crate::app) fn handle_channel_members_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_channel_member(-1),
            KeyCode::Down => self.move_channel_member(1),
            KeyCode::PageUp => self.move_channel_member(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_member(SIDE_PAGE),
            KeyCode::Home => self.select_channel_member(0),
            KeyCode::End => self.select_channel_member(usize::MAX),
            KeyCode::Enter => self.open_channel_member(self.channels.member_selected),
            _ => {}
        }
    }

    // ---- The INFO side tab ---------------------------------------------

    /// Keep the INFO side tab's scroll inside a document `total` lines
    /// long shown `visible` lines at a time, returning the first line to
    /// show.
    pub(crate) fn sync_channel_info_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.channels.info_scroll.sync(total, visible)
    }

    /// Scroll the INFO side tab by `delta` lines, as the wheel does.
    pub(in crate::app) fn scroll_channel_info(&mut self, delta: isize) {
        self.channels.info_scroll.by(delta);
    }

    /// Keys of the INFO side tab, which scroll it.
    pub(in crate::app) fn handle_channel_info_key(&mut self, key: KeyEvent) {
        self.channels.info_scroll.key(key.code);
    }

    // ---- The QUEUE side tab --------------------------------------------

    /// The merge queue of the open channel's repo, once the session sent
    /// it. `None` with no channel open.
    #[must_use]
    pub(crate) fn open_channel_queue(&self) -> Option<&MergeQueueView> {
        let id = self.channels.open.as_deref()?;
        self.channels.merge_queues.get(id)
    }

    /// Keep the merge queue of channel `channel_id`'s repo, for its QUEUE
    /// tab, in place of the one before.
    pub(in crate::app) fn set_merge_queue(&mut self, channel_id: String, queue: MergeQueueView) {
        self.channels.merge_queues.insert(channel_id, queue);
    }

    /// Keep the QUEUE side tab's scroll inside a document `total` lines
    /// long shown `visible` lines at a time, returning the first line to
    /// show.
    pub(crate) fn sync_channel_queue_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.channels.queue_scroll.sync(total, visible)
    }

    /// Scroll the QUEUE side tab by `delta` lines, as the wheel does.
    pub(in crate::app) fn scroll_channel_queue(&mut self, delta: isize) {
        self.channels.queue_scroll.by(delta);
    }

    /// Keys of the QUEUE side tab, which scroll it.
    pub(in crate::app) fn handle_channel_queue_key(&mut self, key: KeyEvent) {
        self.channels.queue_scroll.key(key.code);
    }
}
