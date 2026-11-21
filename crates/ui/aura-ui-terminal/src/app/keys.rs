//! Key event handling for the App.

use super::processes::SIDE_PAGE;
use super::{
    line_edit::LineEdit, AgentsTab, App, AppState, ChatTab, NotificationType, PanelFocus,
    SidePanelTab,
};
use crate::{events::UiEvent, input::InputHistory, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        self.notification = None;
        self.clear_selection();

        if self.context_menu.is_some() && self.handle_context_menu_key(key) {
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('x')
            && matches!(self.state, AppState::Idle | AppState::Processing)
        {
            self.toggle_chat_expanded();
            return KeyResult::continue_running();
        }
        if self.detail.is_some() {
            self.handle_detail_key(key);
            return KeyResult::continue_running();
        }

        let result = self.dispatch_key(key);
        self.sync_mentions();
        result
    }

    fn dispatch_key(&mut self, key: KeyEvent) -> KeyResult {
        match self.state {
            AppState::AwaitingApproval => self.handle_approval_key(key),
            AppState::ShowingHelp => {
                self.state = AppState::Idle;
                KeyResult::continue_running()
            }
            AppState::ShowingProjectInfo => {
                self.close_project_info();
                KeyResult::continue_running()
            }
            AppState::LoginEmail | AppState::LoginPassword | AppState::LinkCode => {
                self.handle_login_key(key)
            }
            AppState::SelectingModel => self.handle_model_selector_key(key),
            AppState::SelectingFolder => self.handle_folder_picker_key(key),
            AppState::Idle | AppState::Processing => self.handle_normal_key(key),
        }
    }

    /// Handle key while a record or file covers the chat panel. A file
    /// takes vim keys; on a record, Esc, `q` or Enter closes it and the
    /// rest scroll it.
    fn handle_detail_key(&mut self, key: KeyEvent) {
        let screen = self.detail_screen();
        if let Some((file, scroll)) = self.file_view_mut() {
            if file.handle_key(key, scroll, screen) == super::vim::Outcome::Close
                && !self.return_to_graph()
            {
                self.close_detail();
            }
            return;
        }
        if self.detail == Some(super::DetailView::Graph) {
            if self.handle_graph_key(key) {
                self.close_detail();
            }
            return;
        }
        if matches!(self.detail, Some(super::DetailView::Image(_))) {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.close_detail(),
                KeyCode::Char('o') => self.open_image_externally(),
                _ => {}
            }
            return;
        }
        let page = self.detail_page_rows();
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.close_detail(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_detail_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_detail_down(1),
            KeyCode::PageUp => self.scroll_detail_up(page),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_detail_down(page),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_detail_up(usize::MAX),
            KeyCode::End | KeyCode::Char('G') => self.scroll_detail_down(usize::MAX),
            _ => {}
        }
    }

    /// Handle key while the new-project folder browser is open.
    fn handle_folder_picker_key(&mut self, key: KeyEvent) -> KeyResult {
        let Some(picker) = self.folder_picker.as_mut() else {
            self.state = AppState::Idle;
            return KeyResult::continue_running();
        };
        if picker.new_folder().is_some() {
            match key.code {
                KeyCode::Esc => picker.cancel_new_folder(),
                KeyCode::Enter => {
                    if let Err(problem) = picker.create_new_folder() {
                        self.notification = Some((problem, NotificationType::Warning));
                    }
                }
                KeyCode::Backspace => picker.pop_new_folder_char(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.push_new_folder_char(c);
                }
                _ => {}
            }
            return KeyResult::continue_running();
        }
        let moved = match key.code {
            KeyCode::Esc => {
                self.cancel_folder_picker();
                return KeyResult::continue_running();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                picker.move_by(-1);
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.move_by(1);
                Ok(None)
            }
            KeyCode::PageUp => {
                picker.move_by(-10);
                Ok(None)
            }
            KeyCode::PageDown => {
                picker.move_by(10);
                Ok(None)
            }
            KeyCode::Home => {
                picker.select_first();
                Ok(None)
            }
            KeyCode::End => {
                picker.select_last();
                Ok(None)
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => picker.activate(),
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                picker.go_up().map(|()| None)
            }
            KeyCode::Char(' ') => Ok(picker.highlighted_folder()),
            KeyCode::Char('~') => picker.go_home().map(|()| None),
            KeyCode::Char('.') => {
                picker.toggle_hidden();
                Ok(None)
            }
            KeyCode::Char('n') => {
                picker.start_new_folder();
                Ok(None)
            }
            _ => Ok(None),
        };
        match moved {
            Ok(Some(folder)) => self.choose_project_folder(folder),
            Ok(None) => {}
            Err(e) => {
                self.notification = Some((
                    format!("Cannot open folder: {e}"),
                    NotificationType::Warning,
                ));
            }
        }
        KeyResult::continue_running()
    }

    /// Handle key in approval mode: `y` allows the call once, `a` always
    /// (in this project), `n` or Esc denies it. The turn goes on either
    /// way; Ctrl+C cancels it instead.
    fn handle_approval_key(&mut self, key: KeyEvent) -> KeyResult {
        let always_offered = self
            .pending_approvals
            .front()
            .is_some_and(|a| !a.always.is_empty());
        let answer = match key.code {
            KeyCode::Char('y' | 'Y') => "Approved",
            KeyCode::Char('a' | 'A') if always_offered => "Always allowed",
            KeyCode::Char('n' | 'N') | KeyCode::Esc => "Denied",
            _ => return KeyResult::continue_running(),
        };
        if let Some(approval) = self.pending_approvals.pop_front() {
            self.mark_awaiting_approval(&approval.id, false);
            let event = match key.code {
                KeyCode::Char('y' | 'Y') => UiEvent::Approve(approval.id),
                KeyCode::Char('a' | 'A') => UiEvent::ApproveAlways(approval.id),
                _ => UiEvent::Deny(approval.id),
            };
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(event);
            }
            self.status = format!("{answer}, continuing...");
        }
        if self.pending_approvals.is_empty() {
            self.state = AppState::Processing;
        } else {
            self.after_approval_closed();
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
        if self.handle_pending_delete(key)
            || self.handle_pending_remove_project(key)
            || self.handle_pending_kill(key)
        {
            return KeyResult::continue_running();
        }
        if let Some(result) = self.handle_mention_menu_key(key) {
            return result;
        }
        if let Some(result) = self.handle_slash_menu_key(key) {
            return result;
        }
        if let Some(result) = self.handle_queue_key(key) {
            return result;
        }
        if self.toggle_terminal_mode(key) {
            return KeyResult::continue_running();
        }
        if key.code == KeyCode::Tab {
            self.focus = self.next_panel_focus();
            return KeyResult::continue_running();
        }
        if key.code == KeyCode::BackTab {
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::CycleApproval(self.active_agent_id.clone()));
            }
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            self.prompt_new_project();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.toggle_all_activity();
            return KeyResult::continue_running();
        }
        // Cmd+V (Super), for terminals that report it.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
            && key.code == KeyCode::Char('v')
        {
            self.request_paste();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.edit_input_in_editor();
            return KeyResult::continue_running();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
            self.stop_command();
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
        if self.focus == PanelFocus::Swarm
            && self.agents_tab == AgentsTab::Projects
            && self.input.is_empty()
            && !word_motion
            && key.code == KeyCode::Char(' ')
        {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.toggle_selected_project();
            } else {
                self.toggle_selected_fold();
            }
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Swarm
            && self.agents_tab == AgentsTab::Projects
            && self.input.is_empty()
            && key.code == KeyCode::Delete
        {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.remove_selected_project();
            } else {
                self.archive_selected_agent();
            }
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Swarm
            && self.agents_tab == AgentsTab::Archive
            && self.input.is_empty()
            && !word_motion
            && self.handle_archive_key(key)
        {
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Chat
            && self.input.is_empty()
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            self.shift_chat_tab(key.code == KeyCode::Right);
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Chat
            && matches!(self.chat_tab(), ChatTab::Graph | ChatTab::Time)
            && self.input.is_empty()
            && self.handle_flow_key(key)
        {
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Chat && self.chat_tab() == ChatTab::Bench {
            if key.code == KeyCode::Enter && key.modifiers.is_empty() && self.bench_fill_field() {
                return KeyResult::continue_running();
            }
            if self.input.is_empty() && self.handle_bench_key(key) {
                return KeyResult::continue_running();
            }
        }
        if self.focus == PanelFocus::Sandbox
            && self.input.is_empty()
            && !word_motion
            && (self.handle_side_tab_key(key) || self.handle_side_list_shortcut(key))
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
                AgentsTab::Archive => {
                    self.handle_archive_key(key);
                    KeyResult::continue_running()
                }
            },
            PanelFocus::Sandbox => match self.side_tab {
                SidePanelTab::Sandbox => self.handle_sandbox_key(key),
                SidePanelTab::Processes => {
                    self.handle_processes_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Trace => {
                    self.handle_trace_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Plans => {
                    self.handle_plans_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Tasks => {
                    self.handle_tasks_key(key);
                    KeyResult::continue_running()
                }
            },
        }
    }

    /// `$` on an empty line opens the TERMINAL tab; Esc, or Backspace on an
    /// empty line, goes back to CHAT. Returns whether the key was used.
    fn toggle_terminal_mode(&mut self, key: KeyEvent) -> bool {
        let entering = !self.terminal_mode
            && key.code == KeyCode::Char('$')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.input.is_empty();
        let leaving = self.terminal_mode
            && (key.code == KeyCode::Esc
                || (key.code == KeyCode::Backspace && self.input.is_empty()));
        if !(entering || leaving) {
            return false;
        }
        self.show_chat_tab(if entering {
            ChatTab::Terminal
        } else {
            ChatTab::Chat
        });
        true
    }

    /// History for the current prompt: commands at the terminal prompt,
    /// messages otherwise.
    pub(super) fn history(&mut self) -> &mut InputHistory {
        if self.terminal_mode {
            &mut self.command_history
        } else {
            &mut self.input_history
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
            PanelFocus::Records => self.record_panel_visible(),
            PanelFocus::Sandbox => self.sandbox_panel_visible(),
            PanelFocus::Swarm => self.swarm_panel_visible(),
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
                if !self.input.is_empty() {
                    return self.submit_input();
                }
                // Enter on an empty prompt sends the oldest queued message
                // now, cancelling the turn that is running.
                if self.state == AppState::Processing && self.hud.queued() > 0 {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::SendQueuedNow(self.active_agent_id.clone()));
                    }
                    self.notification = Some((
                        "Sending the queued message now".to_string(),
                        NotificationType::Success,
                    ));
                }
            }
            // A recalled `/command` keeps the slash menu closed so Up/Down
            // stay on history.
            KeyCode::Up => {
                if let Some(prev) = self.history().previous().map(str::to_string) {
                    self.input = prev;
                    self.cursor_pos = self.input.len();
                    self.slash_dismissed = true;
                }
            }
            KeyCode::Down => {
                if let Some(newer) = self.history().next_newer().map(str::to_string) {
                    self.input = newer;
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
                    self.open_detail(super::DetailView::Record);
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

    /// Handle key when the agents panel shows Projects. Moving the
    /// highlight onto an agent switches to it; project headers are skipped.
    /// Space folds the highlighted agent over its spawns, or its project
    /// when it has none; Shift+Space folds its project either way. Enter
    /// on "+ New project" creates one.
    fn handle_swarm_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            KeyCode::Up => self.select_agent_row(self.selected_agent.saturating_sub(1)),
            KeyCode::Down => self.select_agent_row(self.selected_agent + 1),
            KeyCode::Enter => self.activate_agent_row(),
            KeyCode::PageUp => self.select_agent_row(self.selected_agent.saturating_sub(5)),
            KeyCode::PageDown => self.select_agent_row(self.selected_agent + 5),
            KeyCode::Home => self.select_agent_row(0),
            KeyCode::End => self.select_last_agent_row(),
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
        self.history().add(&text);
        self.scroll_chat_to_bottom();

        let busy = self.state == AppState::Processing;
        let agent_id = self.active_agent_id.clone();
        if self.terminal_mode {
            let command = text.trim().to_string();
            if command.is_empty() {
                return KeyResult::continue_running();
            }
            if busy {
                self.input = text;
                self.cursor_pos = self.input.len();
                self.notification = Some((
                    "Wait for this agent's turn to finish before running a command".to_string(),
                    NotificationType::Warning,
                ));
                return KeyResult::continue_running();
            }
            self.add_message(crate::components::Message::shell(&command));
            self.state = AppState::Processing;
            self.status = "Running...".to_string();
            let cols = self.command_columns();
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::RunCommand {
                    agent_id,
                    command,
                    cols,
                });
            }
            return KeyResult::continue_running();
        }

        if text.starts_with('/') {
            return self.handle_command(&text);
        }
        if !busy && self.wants_build(&text) {
            self.request_build("");
            return KeyResult::continue_running();
        }
        let (images, previews) = self.take_images(&text);

        // A busy agent reads the message before its next step, or once its
        // current work ends; the chat shows it at that point, so it lands
        // in order with the reply still streaming.
        if busy && !images.is_empty() {
            self.queue_image_previews(&text, previews);
            self.notification = Some((
                "Queued: the agent gets the images when its current work ends".to_string(),
                NotificationType::Success,
            ));
        } else if busy {
            self.notification = Some((
                "Queued: sent when the current work ends. Enter on an empty prompt sends it now"
                    .to_string(),
                NotificationType::Success,
            ));
        } else {
            self.add_message(
                crate::components::Message::new(crate::components::MessageRole::User, &text)
                    .with_images(previews),
            );
            self.state = AppState::Processing;
            self.status = "Thinking...".to_string();
        }

        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::UserMessage {
                agent_id,
                text,
                shown: !busy,
                images,
            });
        }
        KeyResult::continue_running()
    }
}
