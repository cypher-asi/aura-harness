//! Scrolling the chat and the panels: the wheel, the page keys, and
//! keeping each list's scroll on its highlight and each info tab's inside
//! its document.

use super::processes::SIDE_PAGE;
use super::{follow_selection, App, PanelFocus, RecordsTab, SidePanelTab};
use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

/// How far an info tab (POLICIES, HOST, a channel's INFO), a read-only
/// document, is scrolled. The wheel and the navigation keys move it; the
/// renderer keeps it inside the document with [`Self::sync`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InfoScroll {
    /// First line on screen
    first: usize,
}

impl InfoScroll {
    /// Move `delta` lines, as the wheel does.
    pub(crate) fn by(&mut self, delta: isize) {
        self.first = self.first.saturating_add_signed(delta);
    }

    /// Move for a navigation key: ↑↓ a line, PgUp/PgDn a page, Home/End
    /// to either end. Returns whether `code` is one.
    pub(crate) fn key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up => self.by(-1),
            KeyCode::Down => self.by(1),
            KeyCode::PageUp => self.by(-SIDE_PAGE),
            KeyCode::PageDown => self.by(SIDE_PAGE),
            KeyCode::Home => self.first = 0,
            KeyCode::End => self.first = usize::MAX,
            _ => return false,
        }
        true
    }

    /// Keep the scroll inside a document `total` lines long shown
    /// `visible` at a time, returning the first line to show.
    pub(crate) fn sync(&mut self, total: usize, visible: usize) -> usize {
        self.first = self.first.min(total.saturating_sub(visible));
        self.first
    }
}

impl App {
    /// Clamp the scroll offset to a maximum value.
    pub fn clamp_scroll(&mut self, max: usize) {
        self.scroll_offset = self.scroll_offset.min(max);
    }

    /// Fit the chat scroll position to a conversation `total_lines` long shown
    /// `visible_rows` at a time, returning the first line to draw.
    ///
    /// While scrolled up, lines added at the bottom push the offset up by as
    /// much, so the lines on screen stay put instead of sliding away under
    /// streaming output. `reflowed` means every line moved (the width
    /// changed), so there is nothing to keep in place.
    pub fn sync_chat_scroll(
        &mut self,
        total_lines: usize,
        visible_rows: usize,
        reflowed: bool,
    ) -> usize {
        if self.scroll_offset > 0 && !reflowed {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(total_lines)
                .saturating_sub(self.chat_line_count);
        }
        self.chat_line_count = total_lines;
        let bottom_start = total_lines.saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(bottom_start);
        bottom_start - self.scroll_offset
    }

    /// Lines PageUp/PageDown move the chat: a screenful, less two lines
    /// kept for context.
    pub(crate) fn chat_page_rows(&self) -> usize {
        self.areas
            .chat_text
            .map_or(5, |area| usize::from(area.height).saturating_sub(2).max(1))
    }

    /// Scroll the chat to the first message.
    pub(crate) fn scroll_chat_to_top(&mut self) {
        self.scroll_offset = usize::MAX;
    }

    /// Scroll the chat to the end and keep following new output.
    pub(crate) fn scroll_chat_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scroll up by the given number of lines (panel-aware).
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_panel_up(self.focus, lines);
    }

    /// Scroll down by the given number of lines (panel-aware).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_panel_down(self.focus, lines);
    }

    pub(super) fn scroll_panel_up(&mut self, panel: PanelFocus, lines: usize) {
        match panel {
            PanelFocus::Chat if self.flow.is_open() => {
                self.scroll_flow(MouseEventKind::ScrollUp, KeyModifiers::NONE, lines);
            }
            PanelFocus::Chat if self.bench.is_open() => {
                self.bench
                    .scroll_by(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat if self.log.is_open() && self.detail.is_none() => {
                self.log
                    .scroll_by(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_add(lines);
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Members => {
                self.scroll_members(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Policies => {
                self.scroll_policies(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Inbox => {
                self.scroll_inbox(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records => {
                self.selected_record = self.selected_record.saturating_sub(lines);
            }
            PanelFocus::Swarm => {
                self.scroll_agents_panel(-isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Sandbox => {
                let delta = -isize::try_from(lines).unwrap_or(isize::MAX);
                match self.side_tab {
                    SidePanelTab::Sandbox => self.sandbox.move_by(delta),
                    SidePanelTab::Processes => self.scroll_processes(delta),
                    SidePanelTab::Host => self.scroll_host(delta),
                    SidePanelTab::Trace => self.trace_feed.move_by(delta),
                    SidePanelTab::Tasks => self.task_list.move_by(delta),
                    SidePanelTab::ChannelMembers => self.move_channel_member(delta),
                    SidePanelTab::ChannelInfo => self.scroll_channel_info(delta),
                }
            }
        }
    }

    pub(super) fn scroll_panel_down(&mut self, panel: PanelFocus, lines: usize) {
        match panel {
            PanelFocus::Chat if self.flow.is_open() => {
                self.scroll_flow(MouseEventKind::ScrollDown, KeyModifiers::NONE, lines);
            }
            PanelFocus::Chat if self.bench.is_open() => {
                self.bench
                    .scroll_by(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat if self.log.is_open() && self.detail.is_none() => {
                self.log
                    .scroll_by(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Chat => {
                self.scroll_offset = self.scroll_offset.saturating_sub(lines);
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Members => {
                self.scroll_members(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Policies => {
                self.scroll_policies(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records if self.records_tab == RecordsTab::Inbox => {
                self.scroll_inbox(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Records => {
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + lines).min(self.records.len().saturating_sub(1));
                }
            }
            PanelFocus::Swarm => {
                self.scroll_agents_panel(isize::try_from(lines).unwrap_or(isize::MAX));
            }
            PanelFocus::Sandbox => {
                let delta = isize::try_from(lines).unwrap_or(isize::MAX);
                match self.side_tab {
                    SidePanelTab::Sandbox => self.sandbox.move_by(delta),
                    SidePanelTab::Processes => self.scroll_processes(delta),
                    SidePanelTab::Host => self.scroll_host(delta),
                    SidePanelTab::Trace => self.trace_feed.move_by(delta),
                    SidePanelTab::Tasks => self.task_list.move_by(delta),
                    SidePanelTab::ChannelMembers => self.move_channel_member(delta),
                    SidePanelTab::ChannelInfo => self.scroll_channel_info(delta),
                }
            }
        }
    }

    /// Keep the selected record inside a list `visible_rows` tall, returning
    /// the first record to show.
    pub fn sync_records_scroll(&mut self, visible_rows: usize) -> usize {
        self.records_scroll = follow_selection(
            self.records_scroll,
            self.selected_record,
            visible_rows,
            self.records.len(),
        );
        self.records_scroll
    }

    /// Keep the selected sandbox row inside a tree `visible_rows` tall,
    /// returning the first row to show.
    pub fn sync_sandbox_scroll(&mut self, visible_rows: usize) -> usize {
        self.sandbox_scroll = follow_selection(
            self.sandbox_scroll,
            self.sandbox.selected(),
            visible_rows,
            self.sandbox.rows().len(),
        );
        self.sandbox_scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_info_scroll_moves_by_key_and_stays_inside_its_document() {
        let mut scroll = InfoScroll::default();
        assert!(scroll.key(KeyCode::Down));
        assert!(scroll.key(KeyCode::PageDown));
        assert_eq!(scroll.sync(30, 10), 1 + 10);
        assert!(scroll.key(KeyCode::End));
        assert_eq!(scroll.sync(30, 10), 20, "End stops at the last page");
        assert!(scroll.key(KeyCode::Up));
        assert_eq!(scroll.sync(30, 10), 19);
        assert!(scroll.key(KeyCode::PageUp));
        assert_eq!(scroll.sync(30, 10), 9);
        assert!(scroll.key(KeyCode::Home));
        assert_eq!(scroll.sync(30, 10), 0);
        assert!(!scroll.key(KeyCode::Char('p')), "other keys are the tab's");
        scroll.by(-3);
        assert_eq!(scroll.sync(30, 10), 0, "never above the top");
        scroll.by(50);
        assert_eq!(scroll.sync(5, 10), 0, "a document that fits");
    }
}
