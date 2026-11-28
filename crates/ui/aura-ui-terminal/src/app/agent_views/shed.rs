//! Letting go of the chats of agents long off screen.
//!
//! Every agent the UI showed or heard from keeps a view, and a view holds
//! its agent's whole chat: up to `MAX_MESSAGES` messages with every tool
//! call's arguments and result, the lines laid out for them, and the
//! Record panel's rows. With many agents at work that is most of the
//! UI's memory, and none of it is the only copy: the agent's record has
//! the chat and its rows.
//!
//! So only the [`KEPT_VIEWS`] agents most recently on screen, and the
//! open channel's coordinator (its chat feeds the channel's HISTORY),
//! keep their chats whole. Any other view that is settled (no turn in
//! flight, nothing streaming, no approval waiting) is shed: its messages,
//! lines and records go, and it counts as not holding its history. What
//! the user left of it stays: its scroll place, tab, draft, HUD, open
//! groups and Sandbox tree. Coming back to it asks the session for its
//! history and records, as for an agent never shown, and shows it once
//! they arrive.
//!
//! A shed agent that starts a turn before it is shown again shows only
//! that turn while it runs, as an agent never shown does. When the turn
//! ends on screen the UI asks for the history once more; should the
//! session still see the turn running, the next visit loads it.

use super::AgentView;
use crate::app::agent_switch::KEPT_VIEWS;
use crate::app::App;
use crate::events::UiEvent;
use crate::renderer::ChatLineCache;
use std::collections::VecDeque;

impl AgentView {
    /// Whether nothing in the view is under way, so it can be rebuilt
    /// from the agent's record.
    fn is_settled(&self) -> bool {
        !self.busy
            && self.pending_approvals.is_empty()
            && self.streaming_content.is_empty()
            && !self
                .messages
                .iter()
                .any(|m| m.is_live_activity() || m.is_live_boot())
    }

    /// Whether the view still holds anything shedding would let go of.
    fn holds_chat(&self) -> bool {
        !self.messages.is_empty() || !self.records.is_empty() || self.loaded
    }

    /// Let go of the chat, its lines and its records.
    fn shed(&mut self) {
        self.messages = VecDeque::new();
        self.chat_lines = ChatLineCache::default();
        self.records = VecDeque::new();
        self.selected_record = 0;
        self.records_scroll = 0;
        self.loaded = false;
        self.loading_since = None;
        self.revealed_at = None;
        self.shed = true;
    }
}

impl App {
    /// Note that `agent_id` just left the screen, the most recent of the
    /// agents whose views are kept whole.
    pub(super) fn note_left_screen(&mut self, agent_id: &str) {
        let recent = &mut self.switching.recent;
        recent.retain(|id| id != agent_id);
        recent.push_front(agent_id.to_string());
        recent.truncate(KEPT_VIEWS);
    }

    /// Shed every settled view but the kept ones and `except`'s, the
    /// agent a command just came for: it may be the chat the session sent
    /// ahead of a switch to it.
    pub(super) fn shed_views(&mut self, except: Option<&str>) {
        let coordinator = self.open_channel().map(|c| c.coordinator_id.clone());
        let switching = &mut self.switching;
        for (id, view) in &mut switching.views {
            let kept = switching.recent.contains(id)
                || coordinator.as_ref() == Some(id)
                || except == Some(id.as_str());
            if !kept && view.holds_chat() && view.is_settled() {
                view.shed();
            }
        }
    }

    /// Once the turn on screen ended in a chat that was shed and showed
    /// only that turn, ask for its whole history, once.
    pub(in crate::app) fn reload_shed_chat(&mut self) {
        if self.switching.reload_after_turn.as_deref() != Some(self.active_agent_id.as_str()) {
            return;
        }
        self.switching.reload_after_turn = None;
        if !self.switching.chat_loaded {
            self.try_send_event(UiEvent::SelectAgent(self.active_agent_id.clone()));
        }
    }

    /// Whether the view of `agent_id` off screen holds its chat, for tests.
    #[cfg(test)]
    pub(crate) fn holds_chat_of(&self, agent_id: &str) -> bool {
        self.switching
            .views
            .get(agent_id)
            .is_some_and(AgentView::holds_chat)
    }
}
