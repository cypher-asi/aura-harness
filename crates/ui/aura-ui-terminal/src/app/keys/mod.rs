//! Key event handling for the App.

mod chat;
mod modal;
mod normal;
mod panels;

use super::{App, AppState, NotificationType, PanelFocus};
use crate::{events::UiEvent, input::InputHistory, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyResult {
        self.notifications.current = None;
        self.clear_selection();
        let just_queued = self.pending.just_queued.take();

        // Enter right after queueing a message sends it now: the event
        // loop gets the message first, so its queue holds it even when the
        // `SetQueue` showing it has not arrived yet.
        if just_queued.as_deref() == Some(self.active_agent_id.as_str())
            && key.code == KeyCode::Enter
            && key.modifiers.is_empty()
            && self.input.text.is_empty()
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
            AppState::ChannelForm => {
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

    /// History for the current prompt: commands at the terminal prompt,
    /// messages otherwise.
    pub(super) fn history(&mut self) -> &mut InputHistory {
        if self.terminal_mode {
            &mut self.input.command_history
        } else {
            &mut self.input.history
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

    /// Ask the event loop to run the oldest queued message of the agent on
    /// screen now, cancelling its running turn.
    fn send_queued_now(&mut self) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SendQueuedNow(self.active_agent_id.clone()));
        }
        self.notifications.current = Some((
            "Sending the queued message now".to_string(),
            NotificationType::Success,
        ));
    }
}
