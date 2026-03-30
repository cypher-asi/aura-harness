//! Key event handling for the App.

use super::{App, AppState, NotificationType, PanelFocus};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        self.notification = None;

        if self.showing_record_detail {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.showing_record_detail = false;
            }
            return KeyResult::continue_running();
        }

        match self.state {
            AppState::AwaitingApproval => self.handle_approval_key(key),
            AppState::ShowingHelp => {
                self.state = AppState::Idle;
                KeyResult::continue_running()
            }
            AppState::LoginEmail | AppState::LoginPassword => self.handle_login_key(key),
            AppState::Idle | AppState::Processing => self.handle_normal_key(key),
        }
    }

    /// Handle key in approval mode.
    fn handle_approval_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                if let Some(approval) = self.pending_approval.take() {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::Approve(approval.id));
                    }
                    self.state = AppState::Processing;
                    self.status = "Approved, continuing...".to_string();
                }
            }
            KeyCode::Char('n' | 'N') => {
                if let Some(approval) = self.pending_approval.take() {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::Deny(approval.id));
                    }
                    self.state = AppState::Idle;
                    self.status = "Denied".to_string();
                }
            }
            KeyCode::Esc => {
                self.pending_approval = None;
                self.state = AppState::Idle;
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key during login flow (email or password entry).
    fn handle_login_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Esc => {
                self.state = AppState::Idle;
                self.status = "Ready".to_string();
                self.input.clear();
                self.cursor_pos = 0;
                self.login_email.clear();
                self.notification =
                    Some(("Login cancelled".to_string(), NotificationType::Warning));
            }
            KeyCode::Enter => {
                let value = std::mem::take(&mut self.input);
                self.cursor_pos = 0;

                if value.trim().is_empty() {
                    self.notification =
                        Some(("Cannot be empty".to_string(), NotificationType::Warning));
                    return KeyResult::continue_running();
                }

                match self.state {
                    AppState::LoginEmail => {
                        self.login_email = value.trim().to_string();
                        self.state = AppState::LoginPassword;
                        self.status = "Login — enter password".to_string();
                    }
                    AppState::LoginPassword => {
                        let email = std::mem::take(&mut self.login_email);
                        self.state = AppState::Processing;
                        self.status = "Authenticating...".to_string();
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.try_send(UiEvent::LoginCredentials {
                                email,
                                password: value,
                            });
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key in normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        if key.code == KeyCode::Tab {
            self.focus = self.next_panel_focus();
            return KeyResult::continue_running();
        }

        match key.code {
            KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Home
            | KeyCode::End => {
                return self.handle_chat_key(key);
            }
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    return self.handle_chat_key(key);
                }
            }
            _ => {}
        }

        match self.focus {
            PanelFocus::Chat => self.handle_chat_key(key),
            PanelFocus::Records => self.handle_records_key(key),
            PanelFocus::Swarm => self.handle_swarm_key(key),
        }
    }

    /// Get the next panel focus when Tab is pressed.
    const fn next_panel_focus(&self) -> PanelFocus {
        match self.focus {
            PanelFocus::Chat => {
                if self.record_panel_visible {
                    PanelFocus::Records
                } else if self.swarm_panel_visible {
                    PanelFocus::Swarm
                } else {
                    PanelFocus::Chat
                }
            }
            PanelFocus::Records => {
                if self.swarm_panel_visible {
                    PanelFocus::Swarm
                } else {
                    PanelFocus::Chat
                }
            }
            PanelFocus::Swarm => PanelFocus::Chat,
        }
    }

    /// Handle key when chat panel is focused.
    fn handle_chat_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() && self.state != AppState::Processing {
                    return self.submit_input();
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Up => {
                if let Some(prev) = self.input_history.previous() {
                    self.input = prev.to_string();
                    self.cursor_pos = self.input.len();
                }
            }
            KeyCode::Down => {
                if let Some(newer) = self.input_history.next_newer() {
                    self.input = newer.to_string();
                    self.cursor_pos = self.input.len();
                } else {
                    self.input.clear();
                    self.cursor_pos = 0;
                }
            }
            KeyCode::PageUp => {
                self.scroll_up(5);
            }
            KeyCode::PageDown => {
                self.scroll_down(5);
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when records panel is focused.
    fn handle_records_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => {
                if self.selected_record > 0 {
                    self.selected_record -= 1;
                }
            }
            KeyCode::Down => {
                if !self.records.is_empty() && self.selected_record < self.records.len() - 1 {
                    self.selected_record += 1;
                }
            }
            KeyCode::Enter => {
                if !self.records.is_empty() {
                    self.showing_record_detail = true;
                }
            }
            KeyCode::PageUp => {
                self.selected_record = self.selected_record.saturating_sub(5);
            }
            KeyCode::PageDown => {
                if !self.records.is_empty() {
                    self.selected_record =
                        (self.selected_record + 5).min(self.records.len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.selected_record = 0;
            }
            KeyCode::End => {
                if !self.records.is_empty() {
                    self.selected_record = self.records.len() - 1;
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when swarm panel is focused.
    fn handle_swarm_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => {
                if self.selected_agent > 0 {
                    self.selected_agent -= 1;
                }
            }
            KeyCode::Down => {
                if !self.agents.is_empty() && self.selected_agent < self.agents.len() - 1 {
                    self.selected_agent += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(agent) = self.agents.get(self.selected_agent) {
                    let agent_id = agent.id.clone();
                    if agent_id != self.active_agent_id {
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.try_send(UiEvent::SelectAgent(agent_id));
                        }
                    }
                }
            }
            KeyCode::PageUp => {
                self.selected_agent = self.selected_agent.saturating_sub(5);
            }
            KeyCode::PageDown => {
                if !self.agents.is_empty() {
                    self.selected_agent =
                        (self.selected_agent + 5).min(self.agents.len().saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.selected_agent = 0;
            }
            KeyCode::End => {
                if !self.agents.is_empty() {
                    self.selected_agent = self.agents.len() - 1;
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Submit the current input.
    pub(super) fn submit_input(&mut self) -> KeyResult {
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.input_history.add(&text);

        if text.starts_with('/') {
            return self.handle_command(&text);
        }

        self.add_message(crate::components::Message::new(
            crate::components::MessageRole::User,
            &text,
        ));
        self.state = AppState::Processing;
        self.status = "Thinking...".to_string();

        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::UserMessage(text));
        }
        KeyResult::continue_running()
    }
}
