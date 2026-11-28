//! HISTORY's keys, clicks and wheel, and where its rows are scrolled.

use crate::app::processes::SIDE_PAGE;
use crate::app::{contains, follow_selection, App};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

impl App {
    /// Highlighted row of HISTORY.
    #[must_use]
    pub(crate) const fn selected_channel_history(&self) -> usize {
        self.channels.history.selected
    }

    /// Where HISTORY's rows are drawn this frame, for the pointer.
    pub(crate) fn set_channel_history_rows_area(&mut self, area: Rect) {
        self.channels.history.rows_area = Some(area);
    }

    /// Keep the highlighted row among the `visible` rows, unless the
    /// wheel scrolled them away from it; returns the first row to draw.
    pub(crate) fn sync_channel_history_scroll(&mut self, visible: usize) -> usize {
        let history = &mut self.channels.history;
        let total = history.entries.len();
        history.scroll = if history.wheel_scrolled {
            history.scroll.min(total.saturating_sub(visible))
        } else {
            follow_selection(history.scroll, history.selected, visible, total)
        };
        history.scroll
    }

    /// Row of HISTORY drawn at a screen cell.
    pub(in crate::app) fn channel_history_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let history = &self.channels.history;
        let rows = history
            .rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let at = history.scroll + usize::from(row - rows.y);
        (at < history.entries.len()).then_some(at)
    }

    /// Highlight row `index` of HISTORY (clamped); the rows follow it.
    pub(in crate::app) fn select_channel_history(&mut self, index: usize) {
        let history = &mut self.channels.history;
        history.selected = index.min(history.entries.len().saturating_sub(1));
        history.wheel_scrolled = false;
    }

    /// Move HISTORY's highlight `delta` rows.
    fn move_channel_history(&mut self, delta: isize) {
        let at = self.channels.history.selected.saturating_add_signed(delta);
        self.select_channel_history(at);
    }

    /// The wheel over HISTORY: scroll its rows `delta`, leaving the
    /// highlight where it is.
    pub(in crate::app) fn scroll_channel_history(&mut self, delta: isize) {
        let history = &mut self.channels.history;
        let last = history.entries.len().saturating_sub(1);
        history.scroll = history.scroll.saturating_add_signed(delta).min(last);
        history.wheel_scrolled = true;
    }

    /// A click on a HISTORY row highlights it and jumps to its message.
    pub(in crate::app) fn click_channel_history(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_history_row_at(column, row) {
            self.select_channel_history(index);
            self.jump_to_selected_history();
        }
    }

    /// Keys of HISTORY: ↑↓, PgUp/PgDn and Home/End move the highlight,
    /// Enter jumps to the highlighted message in the chat.
    pub(in crate::app) fn handle_channel_history_key(&mut self, key: KeyEvent) {
        self.refresh_channel_history();
        match key.code {
            KeyCode::Up => self.move_channel_history(-1),
            KeyCode::Down => self.move_channel_history(1),
            KeyCode::PageUp => self.move_channel_history(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_history(SIDE_PAGE),
            KeyCode::Home => self.select_channel_history(0),
            KeyCode::End => self.select_channel_history(usize::MAX),
            KeyCode::Enter => self.jump_to_selected_history(),
            _ => {}
        }
    }
}
