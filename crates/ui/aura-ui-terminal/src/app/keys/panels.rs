//! Keys for the focused panel once no shortcut took them: the chat, the
//! records panel's tabs, the agents panel and the side panel's tabs.

use crate::app::processes::SIDE_PAGE;
use crate::app::{App, DetailView, PanelFocus, RecordsTab, SidePanelTab};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// Hand the key to the focused panel (and its current tab).
    pub(super) fn focused_panel_key(&mut self, key: KeyEvent) -> KeyResult {
        match self.focus {
            PanelFocus::Chat => self.handle_chat_key(key),
            PanelFocus::Records => self.records_tab_key(key),
            PanelFocus::Swarm => self.handle_agents_panel_key(key),
            PanelFocus::Sandbox => self.side_tab_key(key),
        }
    }

    fn records_tab_key(&mut self, key: KeyEvent) -> KeyResult {
        match self.records.tab {
            RecordsTab::Kernel => return self.handle_records_key(key),
            RecordsTab::Members => self.handle_members_key(key),
            RecordsTab::Policies => self.handle_policies_key(key),
            RecordsTab::Inbox => self.handle_inbox_key(key),
            RecordsTab::ChannelTasks => self.handle_tasks_key(key),
        }
        KeyResult::continue_running()
    }

    fn side_tab_key(&mut self, key: KeyEvent) -> KeyResult {
        match self.side.tab {
            SidePanelTab::Sandbox => return self.handle_sandbox_key(key),
            SidePanelTab::Repo => self.handle_repo_key(key),
            SidePanelTab::Processes => self.handle_processes_key(key),
            SidePanelTab::Host => {
                self.host_key(key);
            }
            SidePanelTab::Trace => self.handle_trace_key(key),
            SidePanelTab::Tasks => self.handle_tasks_key(key),
            SidePanelTab::ChannelMembers => self.handle_channel_members_key(key),
            SidePanelTab::ChannelInfo => self.handle_channel_info_key(key),
        }
        KeyResult::continue_running()
    }

    /// Handle key when records panel is focused.
    fn handle_records_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => {
                if self.records.selected > 0 {
                    self.records.selected -= 1;
                }
            }
            KeyCode::Down => {
                if !self.records.items.is_empty()
                    && self.records.selected < self.records.items.len() - 1
                {
                    self.records.selected += 1;
                }
            }
            KeyCode::Enter => {
                if !self.records.items.is_empty() {
                    self.open_detail(DetailView::Record);
                }
            }
            KeyCode::PageUp => {
                self.records.selected = self.records.selected.saturating_sub(5);
            }
            KeyCode::PageDown => {
                if !self.records.items.is_empty() {
                    self.records.selected =
                        (self.records.selected + 5).min(self.records.items.len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.records.selected = 0;
            }
            KeyCode::End => {
                if !self.records.items.is_empty() {
                    self.records.selected = self.records.items.len() - 1;
                }
            }
            KeyCode::Char(c) => {
                self.input.text.insert(self.input.cursor_pos, c);
                self.input.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.input.cursor_pos > 0 {
                    self.input.cursor_pos -= 1;
                    self.input.text.remove(self.input.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when sandbox panel is focused: arrows move through the
    /// tree, Enter opens a file over the chat (or folds a directory),
    /// Right unfolds a directory and Left folds it or goes up a level.
    fn handle_sandbox_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => self.sandbox.move_by(-1),
            KeyCode::Down => self.sandbox.move_by(1),
            KeyCode::PageUp => self.sandbox.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.sandbox.move_by(SIDE_PAGE),
            KeyCode::Home => self.sandbox.select_first(),
            KeyCode::End => self.sandbox.select_last(),
            KeyCode::Enter => self.activate_selected_sandbox_row(),
            KeyCode::Right => self.expand_selected_sandbox_dir(),
            KeyCode::Left => self.sandbox.collapse_or_parent(),
            _ => {}
        }
        KeyResult::continue_running()
    }
}
