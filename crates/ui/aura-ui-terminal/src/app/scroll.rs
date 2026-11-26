//! Scrolling the chat and the panels: the wheel, the page keys, and
//! keeping each list's scroll on its highlight.

use super::{follow_selection, App, PanelFocus, RecordsTab, SidePanelTab};
use crossterm::event::{KeyModifiers, MouseEventKind};

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
                    SidePanelTab::ChannelInfo => {}
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
                    SidePanelTab::ChannelInfo => {}
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
