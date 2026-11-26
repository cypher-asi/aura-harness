//! Keys of the chat prompt: editing, history, scrolling and sending.

use crate::app::line_edit::LineEdit;
use crate::app::{slash, App, AppState, NotificationType};
use crate::components::{Message, MessageRole};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle key when chat panel is focused.
    pub(in crate::app) fn handle_chat_key(&mut self, key: KeyEvent) -> KeyResult {
        if self.chat_scroll_edge_key(key) {
            return KeyResult::continue_running();
        }
        if let Some(edit) = self.edit_input(key) {
            if edit == LineEdit::Changed {
                self.input.reset_slash_menu();
            }
            return KeyResult::continue_running();
        }
        match key.code {
            KeyCode::Enter => {
                if !self.input.text.trim().is_empty() {
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
            KeyCode::Up => self.chat_up_key(),
            KeyCode::Down => self.chat_down_key(),
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

    /// Ctrl+Home and Ctrl+End scroll the chat to its top or bottom.
    /// Returns whether the key was used.
    fn chat_scroll_edge_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Home => self.scroll_chat_to_top(),
            KeyCode::End => self.scroll_chat_to_bottom(),
            _ => return false,
        }
        true
    }

    /// Up in the prompt: the line above, or the previous history entry.
    fn chat_up_key(&mut self) {
        if self.move_cursor_line(true) {
            return;
        }
        if let Some(prev) = self.history().previous().map(str::to_string) {
            self.input.text = prev;
            self.input.cursor_pos = self.input.text.len();
            self.input.slash_dismissed = true;
        }
    }

    /// Down in the prompt: the line below, or the next newer history
    /// entry (an empty prompt past the newest).
    fn chat_down_key(&mut self) {
        if self.move_cursor_line(false) {
            return;
        }
        // A typed draft is not history: Down on its last line only
        // goes to its end.
        if !self.history().is_browsing() {
            self.input.cursor_pos = self.input.text.len();
            return;
        }
        if let Some(newer) = self.history().next_newer().map(str::to_string) {
            self.input.text = newer;
            self.input.cursor_pos = self.input.text.len();
            self.input.slash_dismissed = true;
        } else {
            self.input.text.clear();
            self.input.cursor_pos = 0;
        }
    }

    /// Submit the current input.
    pub(in crate::app) fn submit_input(&mut self) -> KeyResult {
        // With no agent on screen there is nobody to send to; the draft
        // stays for when one is. Commands still run.
        if self.no_agent_on_screen() && !self.input.text.trim_start().starts_with('/') {
            self.notifications.current = Some((self.app_send_refusal(), NotificationType::Warning));
            return KeyResult::continue_running();
        }
        let text = std::mem::take(&mut self.input.text);
        self.input.cursor_pos = 0;
        self.history().add(&text);
        self.scroll_chat_to_bottom();

        let busy = self.state == AppState::Processing;
        let agent_id = self.active_agent_id.clone();
        if self.terminal_mode {
            return self.run_terminal_command(text, busy, agent_id);
        }

        if let Some(message) = slash::strip_plan(&text) {
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

    /// Run `text` at the terminal prompt as a command of `agent_id`; while
    /// `busy` it stays in the prompt instead.
    fn run_terminal_command(&mut self, text: String, busy: bool, agent_id: String) -> KeyResult {
        let command = text.trim().to_string();
        if command.is_empty() {
            return KeyResult::continue_running();
        }
        if busy {
            self.input.text = text;
            self.input.cursor_pos = self.input.text.len();
            self.notifications.current = Some((
                "Wait for this agent's turn to finish before running a command".to_string(),
                NotificationType::Warning,
            ));
            return KeyResult::continue_running();
        }
        self.add_message(Message::shell(&command));
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
        KeyResult::continue_running()
    }

    /// Show `text` in the chat (or queue it while `busy`) and send it to
    /// `agent_id`, with any attached images it names.
    fn send_user_message(&mut self, text: String, busy: bool, agent_id: String) -> KeyResult {
        let (images, previews) = self.take_images(&text);

        // A message to channel members, not the coordinator: the app posts
        // it to the channel, which shows it here, and the members answer.
        // Nothing waits for the coordinator, busy or not.
        if let Some(codes) = self.routed_members(&text) {
            self.notifications.current = Some((
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
            self.notifications.current = Some((
                "Queued: the agent gets the images when its current work ends".to_string(),
                NotificationType::Success,
            ));
        } else if busy {
            self.notifications.current = Some((
                "Queued: sent when the current work ends. Enter on an empty prompt sends it now"
                    .to_string(),
                NotificationType::Success,
            ));
        } else {
            self.add_message(Message::new(MessageRole::User, &text).with_images(previews));
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
            self.pending.just_queued = Some(agent_id);
        }
        KeyResult::continue_running()
    }
}
