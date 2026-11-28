//! Channels and the mail agents send each other.

use crate::app::App;
use crate::events::UiCommand;

impl App {
    /// Apply a command about channels or mail; any other comes back.
    pub(super) fn channel_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::SetChannels(channels) => self.set_channels(channels),
            UiCommand::SetChannelTasks { .. } => {
                // The channel app shows only its chat, members, info and
                // merge queue; members' tasks are seen from PROJECTS.
            }
            UiCommand::ChannelPost { channel_id, post } => self.add_channel_post(channel_id, post),
            UiCommand::SetChannelPosts { channel_id, posts } => {
                self.set_channel_posts(channel_id, posts);
            }
            UiCommand::AddMail(mail) => self.add_mail(mail),
            UiCommand::SetThreadSubject { thread_id, subject } => {
                self.set_thread_subject(thread_id, &subject);
            }
            UiCommand::SetMergeQueue { channel_id, queue } => {
                self.set_merge_queue(channel_id, queue);
            }
            other => return Some(other),
        }
        None
    }
}
