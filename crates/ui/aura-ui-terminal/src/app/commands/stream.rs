//! Replies and thinking streaming into the chat.

use crate::app::App;
use crate::components::{Activity, Message, MessageRole};
use crate::events::{CallUsage, MessageData, SourceUsage, UiCommand};

impl App {
    /// Apply a command that streams a reply or thinking into the chat, or
    /// shows a message; any other comes back.
    pub(super) fn stream_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::StartStreaming => self.cmd_start_streaming(),
            UiCommand::AppendText(text) => self.cmd_append_text(&text),
            UiCommand::FinishStreaming => self.cmd_finish_streaming(),
            UiCommand::SetReplyModel {
                model,
                usage,
                sources,
            } => self.cmd_set_reply_model(&model, usage, &sources),
            UiCommand::StartThinking => self.add_activity(Activity::thinking()),
            UiCommand::AppendThinking(thinking) => self.cmd_append_thinking(&thinking),
            UiCommand::FinishThinking => self.cmd_finish_thinking(),
            UiCommand::ShowMessage(data) => self.cmd_show_message(&data),
            UiCommand::UserSteered(text) => self.cmd_user_steered(&text),
            other => return Some(other),
        }
        None
    }

    fn cmd_start_streaming(&mut self) {
        self.streaming_content.clear();
        if self
            .messages
            .back()
            .is_some_and(|m| m.is_streaming() && m.content().is_empty())
        {
            return;
        }
        let mut msg = Message::new(MessageRole::Assistant, "");
        msg.set_streaming(true);
        self.add_message(msg);
    }

    fn cmd_append_text(&mut self, text: &str) {
        let before = self.streaming_content.len();
        self.streaming_content.push_str(text);
        if let Some(last_msg) = self.messages.back_mut() {
            if !last_msg.is_streaming() {
                return;
            }
            // Copying the whole reply on every delta grows with its length.
            if last_msg.content().len() == before {
                last_msg.append(text);
            } else {
                last_msg.set_content(&self.streaming_content);
            }
        }
    }

    fn cmd_finish_streaming(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if self.streaming_content.is_empty() {
                    self.messages.pop_back();
                } else {
                    last_msg.set_content(&self.streaming_content);
                }
            }
        }
        self.streaming_content.clear();
    }

    /// Each turn's split of tokens by sign-in, as its last reply's
    /// footer shows it. Every reply of a turn carries the turn's split
    /// so far, so only the last one before the next prompt counts.
    pub(in crate::app) fn turn_sources(&self) -> Vec<&[SourceUsage]> {
        let mut turns = Vec::new();
        let mut last: Option<&[SourceUsage]> = None;
        for message in &self.messages {
            if message.role() == MessageRole::User {
                turns.extend(last.take());
            } else if message.role() == MessageRole::Assistant && !message.sources().is_empty() {
                last = Some(message.sources());
            }
        }
        turns.extend(last);
        turns
    }

    fn cmd_set_reply_model(
        &mut self,
        model: &str,
        usage: Option<CallUsage>,
        sources: &[SourceUsage],
    ) {
        let replies = self
            .messages
            .iter_mut()
            .rev()
            .take_while(|m| m.role() != MessageRole::User)
            .filter(|m| m.role() == MessageRole::Assistant && m.activity_step().is_none());
        for message in replies {
            message.set_model(model);
            message.set_usage(usage);
            message.set_sources(sources);
        }
    }

    fn cmd_show_message(&mut self, data: &MessageData) {
        let mut msg = Message::new(data.role, &data.content);
        if data.is_streaming {
            msg.set_streaming(true);
        }
        if let Some(model) = &data.model {
            msg.set_model(model);
        }
        msg.set_usage(data.usage);
        msg.set_sources(&data.sources);
        self.add_message(msg);
    }

    pub(super) fn add_activity(&mut self, activity: Activity) {
        self.add_message(Message::activity(activity, self.activity_expanded));
    }

    /// The last thinking block still streaming.
    fn live_thinking(&mut self) -> Option<&mut Message> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Thinking(step)) if step.running))
    }

    fn cmd_append_thinking(&mut self, text: &str) {
        if self.live_thinking().is_none() {
            self.add_activity(Activity::thinking());
        }
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| {
                if let Activity::Thinking(step) = activity {
                    step.text.push_str(text);
                }
            });
        }
    }

    fn cmd_finish_thinking(&mut self) {
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| activity.finish(""));
        }
    }

    /// Show a message sent into the running turn. A reply still streaming
    /// is split around it, so the rest of the reply keeps streaming below.
    fn cmd_user_steered(&mut self, text: &str) {
        let was_streaming = self.messages.back().is_some_and(Message::is_streaming);
        if was_streaming {
            self.cmd_finish_streaming();
        }
        self.add_message(Message::new(MessageRole::User, text));
        if was_streaming {
            self.cmd_start_streaming();
        }
    }
}
