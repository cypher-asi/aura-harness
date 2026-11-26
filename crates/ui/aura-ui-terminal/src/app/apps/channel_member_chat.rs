//! A member's own chat in the channel app. The channel's chat is its
//! coordinator's chat with the channel's posts merged in, so a member's
//! chat is that member put on screen while the channel stays open (see
//! `follow_active_agent_out_of_channel`, which keeps it open for any
//! member).
//!
//! Nothing is stored: [`App::member_chat`] is whichever member of the open
//! channel is on screen, other than the coordinator. So the chat panel
//! names it, with a close button, however it came on screen: a click or
//! Enter on its MEMBERS row, or the session switching to it (a
//! notification, say). Closing it, with the button, Esc or the desktop's
//! close button, puts the coordinator back on screen, and with it the
//! channel's chat. Opening another channel does the same by itself.

use crate::app::mentions::member_role;
use crate::app::App;
use crate::events::AgentSummary;
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// The member of the open channel whose own chat is on screen: the
    /// active agent, when it is a member and not the coordinator.
    #[must_use]
    pub(crate) fn member_chat(&self) -> Option<&AgentSummary> {
        let channel = self.open_channel()?;
        if channel.coordinator_id == self.active_agent_id {
            return None;
        }
        channel
            .members
            .iter()
            .find(|member| member.id == self.active_agent_id)
    }

    /// The chat panel's title over a member's chat, and what it says
    /// beside it: what the member works on (its title, a subagent's
    /// subtask), else its name, then its code, role and the channel's
    /// `#slug`, as `RE36 · reviewer · #zode`. The role is the permanent
    /// agent it is an instance of, else what it is in the channel.
    #[must_use]
    pub(crate) fn member_chat_title(&self) -> Option<(String, String)> {
        let channel = self.open_channel()?;
        let member = self.member_chat()?;
        let title = member
            .title
            .as_deref()
            .or(member.subtitle.as_deref())
            .unwrap_or(&member.name);
        let role = member
            .definition
            .as_deref()
            .unwrap_or_else(|| member_role(member, &channel.coordinator_id));
        let slug = format!("#{}", channel.slug);
        let info = [member.code.as_deref(), Some(role), Some(slug.as_str())]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        Some((title.to_string(), info))
    }

    /// Open the chat of member `index` of the open channel, highlighting
    /// its row. The coordinator's row goes back to the channel's chat.
    pub(in crate::app) fn open_channel_member(&mut self, index: usize) {
        let Some(id) = self
            .open_channel()
            .and_then(|channel| channel.members.get(index))
            .map(|member| member.id.clone())
        else {
            return;
        };
        self.select_channel_member(index);
        if self.switch_on_screen(&id) {
            self.show_channel_chat();
        }
    }

    /// Go back from a member's chat to the channel's: put the coordinator
    /// on screen. Returns whether a member's chat was open and the switch
    /// went out.
    pub(in crate::app) fn close_member_chat(&mut self) -> bool {
        if self.member_chat().is_none() {
            return false;
        }
        let Some(coordinator) = self
            .open_channel()
            .map(|channel| channel.coordinator_id.clone())
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        self.switch_on_screen(&coordinator)
    }

    /// Close what the chat panel's close button closes: a view over the
    /// chat first, else a member's chat.
    pub(in crate::app) fn close_chat_view(&mut self) {
        if self.detail.view.is_some() {
            self.close_detail();
        } else {
            self.close_member_chat();
        }
    }

    /// Esc on an empty input goes back from a member's chat to the
    /// channel's. Called once the menus and the queue have had the key.
    /// Returns whether the key was used.
    pub(in crate::app) fn esc_closes_member_chat(&mut self, key: KeyEvent) -> bool {
        key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && self.input.text.is_empty()
            && self.close_member_chat()
    }

    /// The channel list changed under a member's chat (`was_open` before
    /// it did): a member no longer in the open channel gives way to the
    /// coordinator, so the chat is never an outsider's under the
    /// channel's name.
    ///
    /// When the switch request cannot go out, the fallback is best-effort:
    /// the UI puts the coordinator on screen by itself, while the session
    /// may still have the dropped member active. Until a later switch
    /// request gets through, events that carry no agent id (`/new`,
    /// sign-in, the gate view, cloud actions) go to the session's active
    /// agent, the member, not the coordinator shown; messages name their
    /// agent and reach the coordinator. A late `SetActiveAgent` for the
    /// member can still put it back on screen, but it is no member any
    /// more, so `follow_active_agent_out_of_channel` closes the channel:
    /// it never shows under `#slug`. Retrying on the next list would not
    /// do: with the member gone from it nothing knows a retry is due.
    ///
    /// The coordinator's chat may not be loaded, yet it does not start
    /// loading: no request went out, so no history would answer and the
    /// chat would wait blank for good. It shows what it holds, any wait
    /// it was left in dropped, as [`Self::cmd_switch_failed`] does.
    pub(in crate::app) fn leave_dropped_member_chat(&mut self, was_open: bool) {
        let Some(coordinator) = self
            .open_channel()
            .map(|channel| channel.coordinator_id.clone())
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        if was_open
            && self.member_chat().is_none()
            && self.active_agent_id != coordinator
            && !self.switch_on_screen(&coordinator)
        {
            self.apply_active_agent(&coordinator);
            self.switching.chat_loading_since = None;
        }
    }
}
