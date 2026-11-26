//! Turns starting and ending, notices, and the conversation as a whole.

use crate::app::{App, AppState, NotificationType, MAX_MESSAGES};
use crate::components::{Activity, BootConsole, Message, MessageRole};
use crate::events::{MessageData, UiCommand, ERROR_PREFIX, SUCCESS_PREFIX, WARNING_PREFIX};

impl App {
    /// Apply a command about the turn, a notice or the whole
    /// conversation; any other comes back.
    pub(super) fn turn_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::SetStatus(status) => self.status = status,
            UiCommand::ShowError(msg) => {
                self.cmd_notification(&msg, ERROR_PREFIX, NotificationType::Error);
            }
            UiCommand::ShowSuccess(msg) => {
                self.cmd_notification(&msg, SUCCESS_PREFIX, NotificationType::Success);
            }
            UiCommand::ShowWarning(msg) => {
                self.cmd_notification(&msg, WARNING_PREFIX, NotificationType::Warning);
            }
            UiCommand::Complete => self.cmd_complete(),
            UiCommand::TurnStarted { prompt } => self.cmd_turn_started(prompt),
            UiCommand::ClearConversation => self.cmd_clear_conversation(),
            UiCommand::LoadConversation(history) => self.cmd_load_conversation(history),
            UiCommand::ChatReady => self.reveal_chat(),
            other => return Some(other),
        }
        None
    }

    /// End every activity block still in progress, e.g. when the turn ends.
    fn finish_live_activity(&mut self) {
        for message in &mut self.messages {
            if message.is_live_activity() {
                message.update_activity(|activity| activity.finish("interrupted"));
            }
        }
    }

    fn cmd_notification(&mut self, msg: &str, prefix: &str, kind: NotificationType) {
        self.add_message(Message::new(MessageRole::System, &format!("{prefix}{msg}")));
        self.notifications.current = Some((msg.to_string(), kind));
    }

    fn cmd_complete(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if !self.streaming_content.is_empty() {
                    last_msg.set_content(&self.streaming_content);
                } else if last_msg.content().is_empty() {
                    self.messages.pop_back();
                }
            }
        }
        self.streaming_content.clear();
        self.finish_live_activity();
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.refresh_sandbox();
        if self.log.is_open() {
            self.request_log();
        }
    }

    fn cmd_turn_started(&mut self, prompt: Option<String>) {
        if let Some(prompt) = prompt {
            let images = self.take_queued_previews(&prompt);
            self.add_message(Message::new(MessageRole::User, &prompt).with_images(images));
        }
        // A turn the user did not start (a background completion notice)
        // must not hide a login prompt or an approval on screen.
        if self.state == AppState::Idle {
            self.state = AppState::Processing;
            self.status = "Thinking...".to_string();
        }
    }

    fn cmd_clear_conversation(&mut self) {
        self.messages.clear();
    }

    fn cmd_load_conversation(&mut self, history: Vec<MessageData>) {
        self.cmd_clear_conversation();
        self.streaming_content.clear();
        for data in history {
            let mut msg = if let Some(boot) = &data.boot {
                Message::boot(BootConsole::replay(boot, data.timestamp_ms))
            } else if let Some(activity) = &data.activity {
                Message::activity(Activity::from(activity), self.activity_expanded)
            } else if let Some(command) = data.command.as_deref() {
                let mut msg = Message::shell(command);
                msg.finish_shell("", None);
                msg
            } else {
                Message::new(data.role, &data.content)
            };
            if let Some(ts) = data
                .timestamp_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                msg = msg.with_timestamp(ts);
            }
            if let Some(model) = &data.model {
                msg.set_model(model);
            }
            msg.set_usage(data.usage);
            self.messages.push_back(msg);
        }
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
        self.reveal_chat();
    }
}
