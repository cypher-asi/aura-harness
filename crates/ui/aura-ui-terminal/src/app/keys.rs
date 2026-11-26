//! Key event handling for the App.

use super::model_picker::PAGE_STEP;
use super::processes::SIDE_PAGE;
use super::{
    line_edit::{is_newline_key, LineEdit},
    App, AppState, ChatTab, NotificationType, PanelFocus, RecordsTab, SidePanelTab,
};
use crate::{events::UiEvent, input::InputHistory, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

impl App {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        self.notification = None;
        self.clear_selection();
        let just_queued = self.just_queued.take();

        // Enter right after queueing a message sends it now: the event
        // loop gets the message first, so its queue holds it even when the
        // `SetQueue` showing it has not arrived yet.
        if just_queued.as_deref() == Some(self.active_agent_id.as_str())
            && key.code == KeyCode::Enter
            && key.modifiers.is_empty()
            && self.input.is_empty()
            && self.state == AppState::Processing
            && self.context_menu.is_none()
            && self.detail.is_none()
        {
            self.send_queued_now();
            return KeyResult::continue_running();
        }

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
            AppState::NewChannel => {
                self.handle_channel_form_key(key);
                KeyResult::continue_running()
            }
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
        if self.handle_thread_key(key) {
            return;
        }
        if matches!(self.detail, Some(super::DetailView::Plan(_))) && key.code == KeyCode::Char('s')
        {
            self.show_plan_source();
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
        let models = &self.models;
        let picker = &mut self.model_picker;
        match key.code {
            KeyCode::Esc => {
                if !picker.clear_filter() {
                    self.state = AppState::Idle;
                }
            }
            KeyCode::Up => picker.move_by(models, -1),
            KeyCode::Down => picker.move_by(models, 1),
            KeyCode::PageUp => picker.move_by(models, -PAGE_STEP),
            KeyCode::PageDown => picker.move_by(models, PAGE_STEP),
            KeyCode::Home => picker.home(),
            KeyCode::End => picker.end(models),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                picker.switch_tab();
            }
            KeyCode::Backspace => picker.pop(),
            KeyCode::Enter => self.pick_in_model_selector(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.push(c);
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Take the highlighted model of the selector for what it was opened
    /// for: the session's model, or a benchmark row's. With nothing
    /// highlighted and nothing typed it closes.
    fn pick_in_model_selector(&mut self) {
        let picker = &self.model_picker;
        let picked = picker.selected(&self.models).map(|m| m.id.clone());
        match (picked, picker.target()) {
            (Some(id), super::PickerTarget::Session) => self.choose_model(&id),
            (Some(id), target) => {
                if let Some(field) = super::benchmarks::FormField::of_picker(target) {
                    self.pick_bench_model(field, &id);
                }
            }
            (None, _) if picker.filter().is_empty() => self.state = AppState::Idle,
            (None, _) => {}
        }
    }

    /// A mouse event while the model selector is open: a click on a model
    /// picks it, as Enter would, a click outside the selector closes it,
    /// and the wheel moves the highlight.
    pub(super) fn model_selector_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        let layout = &self.model_picker_layout;
        let inside = super::contains(Some(layout.modal), column, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => self.state = AppState::Idle,
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = layout.model_at(row) {
                    self.model_picker.select(&self.models, index);
                    self.pick_in_model_selector();
                }
            }
            MouseEventKind::ScrollUp => self.model_picker.move_by(&self.models, -1),
            MouseEventKind::ScrollDown => self.model_picker.move_by(&self.models, 1),
            _ => {}
        }
    }

    /// Handle key in normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        if self.handle_pending_delete(key)
            || self.handle_pending_remove_project(key)
            || self.handle_pending_archive_channel(key)
            || self.handle_pending_kill(key)
        {
            return KeyResult::continue_running();
        }
        // Before the menus, which take Enter to pick a row: a line break
        // is never a pick or a send.
        if self.insert_newline_key(key) {
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
                KeyCode::Char('t') => {
                    self.cycle_thinking();
                    return KeyResult::continue_running();
                }
                _ => {}
            }
        }

        let word_motion = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if self.focus == PanelFocus::Swarm
            && self.input.is_empty()
            && self.agents_panel_shortcut(key, word_motion)
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
        if self.focus == PanelFocus::Chat
            && self.chat_tab() == ChatTab::Log
            && self.detail.is_none()
            && self.input.is_empty()
            && self.handle_log_key(key)
        {
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Sandbox
            && self.input.is_empty()
            && !word_motion
            && (self.handle_side_tab_key(key) || self.handle_side_list_shortcut(key))
        {
            return KeyResult::continue_running();
        }
        if self.focus == PanelFocus::Records
            && self.input.is_empty()
            && !word_motion
            && self.handle_records_shortcut(key)
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
            PanelFocus::Records => match self.records_tab {
                RecordsTab::Kernel => self.handle_records_key(key),
                RecordsTab::Members => {
                    self.handle_members_key(key);
                    KeyResult::continue_running()
                }
                RecordsTab::Policies => {
                    self.handle_policies_key(key);
                    KeyResult::continue_running()
                }
                RecordsTab::Inbox => {
                    self.handle_inbox_key(key);
                    KeyResult::continue_running()
                }
            },
            PanelFocus::Swarm => self.handle_agents_panel_key(key),
            PanelFocus::Sandbox => match self.side_tab {
                SidePanelTab::Sandbox => self.handle_sandbox_key(key),
                SidePanelTab::Processes => {
                    self.handle_processes_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Host => {
                    self.host_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Trace => {
                    self.handle_trace_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::Tasks => {
                    self.handle_tasks_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::ChannelMembers => {
                    self.handle_channel_members_key(key);
                    KeyResult::continue_running()
                }
                SidePanelTab::ChannelInfo => KeyResult::continue_running(),
            },
        }
    }

    /// Shift+Enter, or Alt+Enter or Ctrl+J, breaks the line at the cursor
    /// in the chat or terminal prompt. Typing goes to the input from any
    /// panel, so the break does too, empty line or not. The one exception
    /// is an empty line while the focused chat panel shows GRAPH, TIME,
    /// LOG or BENCH: those tabs keep the key for their rows (Shift+Enter
    /// opens agent cards on GRAPH). Returns whether the key was used.
    fn insert_newline_key(&mut self, key: KeyEvent) -> bool {
        let tab_owns_keys = self.focus == PanelFocus::Chat
            && !matches!(self.chat_tab(), ChatTab::Chat | ChatTab::Terminal);
        let prompt_owns_keys =
            self.queue_selected().is_none() && (!self.input.is_empty() || !tab_owns_keys);
        if !(is_newline_key(key) && prompt_owns_keys) {
            return false;
        }
        self.insert_newline();
        self.reset_slash_menu();
        true
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

    fn panel_visible(&self, panel: PanelFocus) -> bool {
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
                if !self.input.trim().is_empty() {
                    return self.submit_input();
                }
                // Enter on an empty (or blank) prompt sends the oldest
                // queued message now, cancelling the turn that is running.
                if self.state == AppState::Processing && self.hud.queued() > 0 {
                    self.send_queued_now();
                }
            }
            // A recalled `/command` keeps the slash menu closed so Up/Down
            // stay on history. In a multi-line input they move between its
            // lines first, and recall history from the first or last one.
            KeyCode::Up => {
                if self.move_cursor_line(true) {
                    return KeyResult::continue_running();
                }
                if let Some(prev) = self.history().previous().map(str::to_string) {
                    self.input = prev;
                    self.cursor_pos = self.input.len();
                    self.slash_dismissed = true;
                }
            }
            KeyCode::Down => {
                if self.move_cursor_line(false) {
                    return KeyResult::continue_running();
                }
                // A typed draft is not history: Down on its last line only
                // goes to its end.
                if !self.history().is_browsing() {
                    self.cursor_pos = self.input.len();
                    return KeyResult::continue_running();
                }
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

    /// Submit the current input.
    pub(super) fn submit_input(&mut self) -> KeyResult {
        // An app showing nothing has nobody to send to; the draft stays
        // for when something is picked. Commands still run.
        if self.app_empty() && !self.input.trim_start().starts_with('/') {
            self.notification = Some((self.app_send_refusal(), NotificationType::Warning));
            return KeyResult::continue_running();
        }
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

        if let Some(message) = super::commands::strip_plan(&text) {
            // A `/plan` word anywhere sends the rest in plan mode, never
            // as a build request ("/plan implement auth" asks for a plan).
            // On its own it enters or leaves plan mode.
            if message.is_empty() {
                self.request_plan_toggle();
                return KeyResult::continue_running();
            }
            self.request_plan_mode();
            return self.send_user_message(message, busy, agent_id);
        }
        if text.starts_with('/') {
            return self.handle_command(&text);
        }
        if !busy && self.wants_build(&text) {
            self.request_build("");
            return KeyResult::continue_running();
        }
        self.send_user_message(text, busy, agent_id)
    }

    /// Show `text` in the chat (or queue it while `busy`) and send it to
    /// `agent_id`, with any attached images it names.
    fn send_user_message(&mut self, text: String, busy: bool, agent_id: String) -> KeyResult {
        let (images, previews) = self.take_images(&text);

        // A message to channel members, not the coordinator: the app posts
        // it to the channel, which shows it here, and the members answer.
        // Nothing waits for the coordinator, busy or not.
        if let Some(codes) = self.routed_members(&text) {
            self.notification = Some((
                format!("Sent to {}", codes.join(", ")),
                NotificationType::Success,
            ));
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::UserMessage {
                    agent_id,
                    text,
                    shown: true,
                    images,
                });
            }
            return KeyResult::continue_running();
        }

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
                agent_id: agent_id.clone(),
                text,
                shown: !busy,
                images,
            });
        }
        if busy {
            self.just_queued = Some(agent_id);
        }
        KeyResult::continue_running()
    }

    /// Ask the event loop to run the oldest queued message of the agent on
    /// screen now, cancelling its running turn.
    fn send_queued_now(&mut self) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SendQueuedNow(self.active_agent_id.clone()));
        }
        self.notification = Some((
            "Sending the queued message now".to_string(),
            NotificationType::Success,
        ));
    }
}
