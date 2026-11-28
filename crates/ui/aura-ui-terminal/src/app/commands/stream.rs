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
        self.chat.streaming_content.clear();
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

    /// Where the reply still streaming is. It is the last message until
    /// something else is shown mid-reply (a notice, an inbox line, an
    /// error), which goes below it: the rest of the reply still streams
    /// into it, so the line reads after the whole reply, never inside it.
    pub(super) fn live_reply(&self) -> Option<usize> {
        self.messages.iter().rposition(Message::is_streaming)
    }

    fn cmd_append_text(&mut self, text: &str) {
        let before = self.chat.streaming_content.len();
        self.chat.streaming_content.push_str(text);
        let Some(at) = self.live_reply() else {
            return;
        };
        let reply = &mut self.messages[at];
        // Copying the whole reply on every delta grows with its length.
        if reply.content().len() == before {
            reply.append(text);
        } else {
            reply.set_content(&self.chat.streaming_content);
        }
    }

    fn cmd_finish_streaming(&mut self) {
        self.end_live_reply(true);
    }

    /// End the reply still streaming with all it streamed. One that
    /// streamed nothing goes, when `drop_empty` or it has no text at all.
    pub(super) fn end_live_reply(&mut self, drop_empty: bool) {
        if let Some(at) = self.live_reply() {
            let reply = &mut self.messages[at];
            reply.set_streaming(false);
            if !self.chat.streaming_content.is_empty() {
                reply.set_content(&self.chat.streaming_content);
            } else if drop_empty || reply.content().is_empty() {
                self.messages.remove(at);
            }
        }
        self.chat.streaming_content.clear();
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
        self.add_message(Message::activity(activity, self.chat.activity_expanded));
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
        let was_streaming = self.live_reply().is_some();
        if was_streaming {
            self.cmd_finish_streaming();
        }
        self.add_message(Message::new(MessageRole::User, text));
        if was_streaming {
            self.cmd_start_streaming();
        }
    }
}
