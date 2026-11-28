//! The user's `/plan` requests on HISTORY.
//!
//! A request typed with `/plan` goes to a new member, its planner, in the
//! planner's own chat; the channel's chat never shows it. The session
//! keeps it as a plan post naming the planner, which the UI files apart
//! from the channel's posts, so only HISTORY lists it. Picking one opens
//! the planner's chat, as its MEMBERS row does, and jumps to the request
//! there once that chat shows it. A planner no longer in the channel
//! leaves nothing to open: a notice says so.

use super::HistoryItem;
use crate::app::{App, NotificationType};
use crate::events::prompt_key;

/// What HISTORY says when the planner of a request has left the channel.
pub(crate) const PLANNER_GONE: &str = "The agent that planned this is no longer in the channel";

impl App {
    /// Open the chat of the member that plans `item`, a `/plan` request,
    /// and jump to the request in it.
    pub(super) fn jump_to_plan_request(&mut self, item: &HistoryItem) {
        let planner = item.planner.as_deref().unwrap_or_default();
        let index = self
            .open_channel()
            .and_then(|channel| channel.members.iter().position(|m| m.id == planner));
        let Some(index) = index else {
            self.notifications.current =
                Some((PLANNER_GONE.to_string(), NotificationType::Warning));
            return;
        };
        self.open_channel_member(index);
        self.request_chat_jump_in_member(planner, prompt_key(&item.text), item.at_ms);
    }
}
