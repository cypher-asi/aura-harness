//! Keyboard handling for the queued-messages panel.

use crate::app::{App, AppState, PanelFocus};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle `key` for the panel: the editing keys while a queued message
    /// is in the input, the panel's keys while it has the keys, and
    /// Shift+Up, which gives them to it. Returns `None` for keys the panel
    /// leaves to the rest of the screen.
    pub(in crate::app) fn handle_queue_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        if self.queue_panel.editing.is_some() {
            return match key.code {
                KeyCode::Esc => {
                    self.cancel_queue_edit();
                    Some(KeyResult::continue_running())
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.finish_queue_edit();
                    Some(KeyResult::continue_running())
                }
                _ => None,
            };
        }
        if self.queue_panel.parked.is_some() {
            if let Some(result) = self.handle_parked_key(key) {
                return Some(result);
            }
        }
        if self.queue_panel.selected.is_some() {
            return self.handle_focused_queue_key(key);
        }
        let enters = key.code == KeyCode::Up
            && key.modifiers == KeyModifiers::SHIFT
            && self.focus == PanelFocus::Chat
            && matches!(self.state, AppState::Idle | AppState::Processing)
            && !self.queue().is_empty();
        if !enters {
            return None;
        }
        self.queue_panel.collapsed = false;
        self.select_queued(self.queue().len().saturating_sub(1));
        Some(KeyResult::continue_running())
    }

    /// Keys while the input holds the edited text of a message that
    /// already ran: Enter sends it and brings the draft back, Esc brings
    /// the draft back at once.
    pub(super) fn handle_parked_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        match key.code {
            KeyCode::Esc => {
                self.drop_parked_edit();
                Some(KeyResult::continue_running())
            }
            KeyCode::Enter
                if !key.modifiers.contains(KeyModifiers::SHIFT)
                    && !self.input.text.trim().is_empty() =>
            {
                let result = self.submit_input();
                // A send that was refused puts the text back; the draft
                // waits for the next try.
                if self.input.text.is_empty() {
                    if let Some(draft) = self.queue_panel.parked.take() {
                        self.restore_draft(draft);
                    }
                }
                Some(result)
            }
            _ => None,
        }
    }

    pub(super) fn handle_focused_queue_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        let Some(selected) = self.queue_selected() else {
            self.queue_panel.selected = None;
            return None;
        };
        let Some(id) = self.queue().get(selected).map(|item| item.id) else {
            self.queue_panel.selected = None;
            return None;
        };
        let last = self.queue().len().saturating_sub(1);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up if alt => self.move_queued(id, selected.saturating_sub(1)),
            KeyCode::Down if alt => self.move_queued(id, (selected + 1).min(last)),
            KeyCode::Up => self.select_queued(selected.saturating_sub(1)),
            KeyCode::Down => self.select_queued((selected + 1).min(last)),
            KeyCode::Home => self.select_queued(0),
            KeyCode::End => self.select_queued(last),
            KeyCode::Delete => self.remove_queued(id),
            KeyCode::Enter if ctrl => self.send_queued_item_now(id),
            KeyCode::Enter => self.start_queue_edit(id),
            KeyCode::Char('e') if !ctrl && !alt => self.start_queue_edit(id),
            KeyCode::Esc => self.queue_panel.selected = None,
            _ => {
                self.queue_panel.selected = None;
                return None;
            }
        }
        Some(KeyResult::continue_running())
    }
}
