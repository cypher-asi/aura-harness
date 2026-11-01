//! Key event handling for the App.

use super::{line_edit::LineEdit, AgentsTab, App, AppState, NotificationType, PanelFocus};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        self.notification = None;
        self.clear_selection();

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
            AppState::LoginEmail | AppState::LoginPassword | AppState::LinkCode => {
                self.handle_login_key(key)
            }
            AppState::SelectingModel => self.handle_model_selector_key(key),
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
                self.pending_model = None;
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
                    AppState::LinkCode => {
                        self.state = AppState::Idle;
                        self.status = "Linking...".to_string();
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.try_send(UiEvent::LinkCode(value.trim().to_string()));
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                self.edit_input(key);
            }
        }
        KeyResult::continue_running()
    }

    /// Handle key while the model selector is open.
    fn handle_model_selector_key(&mut self, key: KeyEvent) -> KeyResult {
        let last = self.models.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.state = AppState::Idle,
            KeyCode::Up | KeyCode::Char('k') => {
                self.model_cursor = self.model_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.model_cursor = (self.model_cursor + 1).min(last);
            }
            KeyCode::Home => self.model_cursor = 0,
            KeyCode::End => self.model_cursor = last,
            KeyCode::Enter => {
                if let Some(id) = self.models.get(self.model_cursor).map(|m| m.id.clone()) {
                    self.choose_model(&id);
                } else {
                    self.state = AppState::Idle;
                }
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key in normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        if self.handle_pending_delete(key) {
            return KeyResult::continue_running();
        }
        if let Some(result) = self.handle_slash_menu_key(key) {
            return result;
        }
        if key.code == KeyCode::Tab {
            self.focus = self.next_panel_focus();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            self.create_agent();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.toggle_all_activity();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Up => {
                    self.switch_agent_by(-1);
                    return KeyResult::continue_running();
                }
                KeyCode::Down => {
                    self.switch_agent_by(1);
                    return KeyResult::continue_running();
                }
                _ => {}
            }
        }

        let word_motion = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if self.focus == PanelFocus::Swarm
            && self.agents_tab == AgentsTab::Roster
            && self.input.is_empty()
            && !word_motion
            && self.handle_definitions_shortcut(key)
        {
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
            KeyCode::Left | KeyCode::Right if word_motion || !self.input.is_empty() => {
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
            PanelFocus::Swarm => match self.agents_tab {
                AgentsTab::Projects => self.handle_swarm_key(key),
                AgentsTab::Roster => {
                    self.handle_definitions_key(key);
                    KeyResult::continue_running()
                }
            },
            PanelFocus::Sandbox => self.handle_sandbox_key(key),
        }
    }

    /// Get the next visible panel when Tab is pressed.
    fn next_panel_focus(&self) -> PanelFocus {
        const CYCLE: [PanelFocus; 4] = [
            PanelFocus::Chat,
            PanelFocus::Records,
            PanelFocus::Sandbox,
            PanelFocus::Swarm,
        ];
        let current = CYCLE.iter().position(|&p| p == self.focus).unwrap_or(0);
        (1..CYCLE.len())
            .map(|step| CYCLE[(current + step) % CYCLE.len()])
            .find(|&panel| self.panel_visible(panel))
            .unwrap_or(PanelFocus::Chat)
    }

    const fn panel_visible(&self, panel: PanelFocus) -> bool {
        match panel {
            PanelFocus::Chat => true,
            PanelFocus::Records => self.record_panel_visible,
            PanelFocus::Sandbox => self.sandbox_panel_visible,
            PanelFocus::Swarm => self.swarm_panel_visible,
        }
    }

    /// Handle key when chat panel is focused.
    pub(super) fn handle_chat_key(&mut self, key: KeyEvent) -> KeyResult {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Home => {
                    self.scroll_chat_to_top();
                    return KeyResult::continue_running();
                }
                KeyCode::End => {
                    self.scroll_chat_to_bottom();
                    return KeyResult::continue_running();
                }
                _ => {}
            }
        }
        if let Some(edit) = self.edit_input(key) {
            if edit == LineEdit::Changed {
                self.reset_slash_menu();
            }
            return KeyResult::continue_running();
        }
        match key.code {
            KeyCode::Enter => {
                if !self.input.is_empty() && self.state != AppState::Processing {
                    return self.submit_input();
                }
            }
            // A recalled `/command` keeps the slash menu closed so Up/Down
            // stay on history.
            KeyCode::Up => {
                if let Some(prev) = self.input_history.previous() {
                    self.input = prev.to_string();
                    self.cursor_pos = self.input.len();
                    self.slash_dismissed = true;
                }
            }
            KeyCode::Down => {
                if let Some(newer) = self.input_history.next_newer() {
                    self.input = newer.to_string();
                    self.cursor_pos = self.input.len();
                    self.slash_dismissed = true;
                } else {
                    self.input.clear();
                    self.cursor_pos = 0;
                }
            }
            KeyCode::PageUp => {
                self.scroll_up(self.chat_page_rows());
            }
            KeyCode::PageDown => {
                self.scroll_down(self.chat_page_rows());
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

    /// Handle key when sandbox panel is focused: arrows move through the
    /// tree, Enter/Right toggles a directory, Left goes up a level.
    fn handle_sandbox_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Up => self.sandbox.move_by(-1),
            KeyCode::Down => self.sandbox.move_by(1),
            KeyCode::PageUp => self.sandbox.move_by(-5),
            KeyCode::PageDown => self.sandbox.move_by(5),
            KeyCode::Enter | KeyCode::Right => self.sandbox.toggle_selected(),
            KeyCode::Left => self.sandbox.collapse_or_parent(),
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Handle key when the agents panel shows Projects. Moving the
    /// highlight onto an agent switches to it; Enter on "+ New project"
    /// creates one.
    fn handle_swarm_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Left | KeyCode::Right => self.toggle_agents_tab(),
            KeyCode::Up => self.select_agent_row(self.selected_agent.saturating_sub(1)),
            KeyCode::Down => self.select_agent_row(self.selected_agent + 1),
            KeyCode::Enter => self.activate_agent_row(),
            KeyCode::PageUp => self.select_agent_row(self.selected_agent.saturating_sub(5)),
            KeyCode::PageDown => self.select_agent_row(self.selected_agent + 5),
            KeyCode::Home => self.select_agent_row(0),
            KeyCode::End => self.select_agent_row(self.agents.len().saturating_sub(1)),
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
        self.scroll_chat_to_bottom();

        if text.starts_with('/') {
            return self.handle_command(&text);
        }

        if let Some(command) = super::shell_input::parse(&text) {
            self.add_message(crate::components::Message::shell(&command));
            self.state = AppState::Processing;
            self.status = "Running...".to_string();
            let cols = self.command_columns();
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::RunCommand { command, cols });
            }
            return KeyResult::continue_running();
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
