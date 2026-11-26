//! Putting an agent on screen. The UI switches at once when the user picks
//! an agent, and asks the session, which answers with the agent it made
//! active; the session also switches on its own (a new agent, a
//! notification). Both end in [`App::apply_active_agent`], which swaps the
//! chat in and lets the app in front follow (see `apps`).

use super::App;
use crate::events::UiEvent;

impl App {
    /// Request a switch to another agent. Agents keep working while
    /// another one is shown.
    ///
    /// A listed agent comes on screen at once, without waiting for the
    /// session: a chat already held in memory is shown as it is and the
    /// session is only told the agent is in front (`FocusAgent`); any
    /// other chat is asked for (`SelectAgent`) and waits, blank, for its
    /// history. The event goes out before the local switch so the
    /// requests the switch makes (LOG, GRAPH) reach the session after it. An agent that is not listed, such as one shown in
    /// another tab of the desktop window, only gets the request; the
    /// screen changes once the session answers.
    pub(super) fn request_agent_switch(&mut self, agent_id: String) {
        if agent_id == self.active_agent_id {
            return;
        }
        if !self.agents.iter().any(|agent| agent.id == agent_id) {
            self.try_send_event(UiEvent::SelectAgent(agent_id));
            return;
        }
        self.switch_on_screen(&agent_id);
    }

    /// Put `agent_id` on screen now and ask the session for it: a chat
    /// held in memory shows as it is (`FocusAgent`), any other waits,
    /// blank, for its history (`SelectAgent`). [`Self::request_agent_switch`]
    /// does this for listed agents; an open channel does it for its
    /// coordinator, which the projects list leaves out. Returns whether
    /// the request went out, and with it the switch.
    pub(super) fn switch_on_screen(&mut self, agent_id: &str) -> bool {
        if agent_id == self.active_agent_id {
            return true;
        }
        let loaded = self
            .switching
            .views
            .get(agent_id)
            .is_some_and(|view| view.loaded);
        let event = if loaded {
            UiEvent::FocusAgent(agent_id.to_string())
        } else {
            UiEvent::SelectAgent(agent_id.to_string())
        };
        // A request that never left gets no answer, and a switch waiting
        // for one would drop every later `SetActiveAgent`.
        if !self.try_send_event(event) {
            return false;
        }
        if self.switching.switch_sent.is_none() && self.switching.confirmed_agent.is_empty() {
            self.switching
                .confirmed_agent
                .clone_from(&self.active_agent_id);
        }
        self.switching.switch_sent = Some(agent_id.to_string());
        self.apply_active_agent(agent_id);
        self.start_chat_loading();
        true
    }

    /// The session made `agent_id` the active agent. While a switch the
    /// UI already made on screen waits for its answer, only that answer
    /// counts: an echo of an earlier request (A, then B, then back to A
    /// before B was confirmed) is dropped, so the screen never jumps back
    /// to an agent the user left.
    pub(in crate::app) fn cmd_set_active_agent(&mut self, agent_id: &str) {
        let Some(sent) = self.switching.switch_sent.as_deref() else {
            self.switching.confirmed_agent = agent_id.to_string();
            self.apply_active_agent(agent_id);
            return;
        };
        // The session's active agent is `agent_id` either way, which is
        // where a failed switch should go back to.
        let confirms = sent == agent_id;
        self.switching.confirmed_agent = agent_id.to_string();
        if !confirms {
            return;
        }
        self.switching.switch_sent = None;
        if agent_id == self.active_agent_id {
            // The screen switched when the user asked; only the marks
            // the agents list draws may have been redrawn since.
            self.mark_active_agent();
        } else {
            self.apply_active_agent(agent_id);
        }
    }

    /// The session could not switch to `agent_id`. A screen that already
    /// shows it goes back to the agent the session still has active, so
    /// it doesn't wait for a chat that will not come.
    pub(in crate::app) fn cmd_switch_failed(&mut self, agent_id: &str) {
        if self.switching.switch_sent.as_deref() != Some(agent_id)
            && self.active_agent_id != agent_id
        {
            return;
        }
        self.switching.switch_sent = None;
        // The failed agent's chat keeps `loaded` unset, so the next
        // switch to it asks for its history again.
        self.switching.chat_loading_since = None;
        let back = self.switching.confirmed_agent.clone();
        if !back.is_empty() && back != self.active_agent_id {
            self.apply_active_agent(&back);
            // The session sends nothing more for the agent it kept, so a
            // chat of it still waiting shows what it holds.
            self.switching.chat_loading_since = None;
        }
    }

    /// Mark the agent on screen as the active one in the agents list and
    /// the Archive.
    pub(in crate::app) fn mark_active_agent(&mut self) {
        for agent in self.agents.iter_mut().chain(self.archived.items.iter_mut()) {
            agent.is_active = agent.id == self.active_agent_id;
        }
    }

    /// Put `agent_id` on screen: its chat, the tab it was left on, and
    /// fresh LOG and GRAPH content for it when those are open. BENCH,
    /// unless it shows the agent for a benchmark, gives way to the app
    /// that lists it.
    pub(super) fn apply_active_agent(&mut self, agent_id: &str) {
        let switched = agent_id != self.active_agent_id;
        // The LOG tab shows the agent coming on screen, never the one
        // leaving: its last log comes back at once, or none.
        self.log.show_agent(agent_id);
        let left_on = self.show_agent_chat(agent_id);
        if switched && self.bench_app() && self.bench.shown_agent() != Some(agent_id) {
            // Before its tab comes back, which BENCH would refuse; the
            // agent that left gets its own back as BENCH goes.
            self.follow_into_agent_app();
        }
        self.refresh_side_scope();
        if let Some(places) = self.switching.list_places_due.take() {
            self.put_back_list_places(places);
        }
        if self.log_wanted_for(agent_id) {
            // The switch was asked for to show the agent's log.
            self.show_chat_tab(super::ChatTab::Log);
        } else {
            // Each agent comes back on the tab it was left on, CHAT the
            // first time it is shown.
            if let Some(tab) = left_on {
                self.restore_chat_tab(tab);
            }
            if switched && self.log.is_open() {
                // The LOG tab shows the agent on screen and the agents below it.
                self.request_log();
            }
        }
        if self.flow.is_open() {
            // The GRAPH and TIME tabs show the flow of the agent on screen.
            self.request_flow();
        }
        self.mark_active_agent();
        if switched {
            // Switching to an agent folded out of sight opens the agents
            // above it; a folded project stays folded.
            if let Some(index) = self.agents.iter().position(|a| a.id == agent_id) {
                self.open_ancestors(index);
            }
        }
        self.agents_scroll_detached = false;
        self.select_visible_active();
        self.follow_active_agent(switched);
    }
}
