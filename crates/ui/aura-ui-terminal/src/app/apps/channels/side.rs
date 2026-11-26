//! The open channel's side panel: the MEMBERS and INFO tabs.

use crate::app::processes::SIDE_PAGE;
use crate::app::{contains, follow_selection, App};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

impl App {
    /// Highlighted row of the MEMBERS side tab.
    #[must_use]
    pub(crate) const fn selected_channel_member(&self) -> usize {
        self.channels.member_selected
    }

    /// Keep the highlighted member inside rows `visible_rows` tall,
    /// returning the first row to draw.
    pub(crate) fn sync_channel_members_scroll(&mut self, visible_rows: usize) -> usize {
        let members = self.channels.open_members();
        let state = &mut self.channels;
        state.member_scroll = follow_selection(
            state.member_scroll,
            state.member_selected,
            visible_rows,
            members,
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

    /// Member row of the MEMBERS side tab drawn at a screen cell.
    pub(in crate::app) fn channel_member_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        let rows = state
            .member_rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.member_scroll + usize::from(row - rows.y);
        (index < state.open_members()).then_some(index)
    }

    /// Highlight member row `index` (clamped). Members only show; going
    /// to a member's own chat would leave the channel app.
    pub(in crate::app) fn select_channel_member(&mut self, index: usize) {
        self.channels.set_member_selected(index);
    }

    /// Move the MEMBERS highlight `delta` rows, as the wheel does.
    pub(in crate::app) fn move_channel_member(&mut self, delta: isize) {
        let row = self.channels.member_selected.saturating_add_signed(delta);
        self.channels.set_member_selected(row);
    }

    /// Clicking a member row highlights it.
    pub(in crate::app) fn click_channel_member(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_member_row_at(column, row) {
            self.select_channel_member(index);
        }
    }

    /// Keys of the MEMBERS side tab, which move its highlight.
    pub(in crate::app) fn handle_channel_members_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_channel_member(-1),
            KeyCode::Down => self.move_channel_member(1),
            KeyCode::PageUp => self.move_channel_member(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_member(SIDE_PAGE),
            KeyCode::Home => self.select_channel_member(0),
            KeyCode::End => self.select_channel_member(usize::MAX),
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
}
