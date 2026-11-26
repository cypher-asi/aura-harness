//! Events from the tabs' UIs, sent on to the session.
use super::Tabs;
use aura_ui::events::{UiCommand, UiEvent};

impl Tabs {
    pub(super) fn forward(&mut self, from: usize, event: UiEvent) {
        if matches!(event, UiEvent::CreateProject { .. })
            && self.picking == Some(self.tabs[from].id)
        {
            self.picked(from, event);
            return;
        }
        if self.switched(from, &event) {
            return;
        }
        if matches!(
            event,
            UiEvent::CreateProject { .. } | UiEvent::CreateAgent(_) | UiEvent::RestoreAgent(_)
        ) {
            self.asker = Some(self.tabs[from].id);
        }
        self.send_from(from, event);
    }

    /// Ask the session to follow the agent the tab at `from` shows, then
    /// send it `event`.
    fn send_from(&mut self, from: usize, event: UiEvent) {
        let agent = self.shown_by(from).to_string();
        self.request(&agent, false);
        self.send(event);
    }

    /// The folder browser `+` opened in the tab at `from` picked a folder
    /// (`event` is its `CreateProject`).
    fn picked(&mut self, from: usize, event: UiEvent) {
        let UiEvent::CreateProject { folder } = &event else {
            return;
        };
        self.picking = None;
        match self.tab_of_folder(folder) {
            Some((owner, Some(agent))) => {
                self.front = owner;
                let shown = self.shown_by(owner);
                if self.root_of(shown) != self.root_of(&agent) {
                    self.request(&agent, true);
                }
            }
            // Listed with every agent archived: the session adds
            // one to it, in its tab.
            Some((owner, None)) => {
                self.front = owner;
                self.asker = Some(self.tabs[owner].id);
                self.send_from(owner, event);
            }
            None => {
                self.opening = true;
                self.asker = None;
                self.send_from(from, event);
            }
        }
    }

    /// Handle `event` when it switches the tab at `from` to an agent, of
    /// its own or of another tab. Returns whether it did.
    fn switched(&mut self, from: usize, event: &UiEvent) -> bool {
        match event {
            UiEvent::SelectAgent(agent) | UiEvent::FocusAgent(agent)
                if self.owner(agent) == Some(from) =>
            {
                let load = matches!(event, UiEvent::SelectAgent(_));
                self.switch_in_tab(from, agent.clone(), load);
                true
            }
            UiEvent::SelectAgent(agent) => {
                // An agent of another tab: go to that tab instead.
                let Some(owner) = self.owner(agent).filter(|owner| *owner != from) else {
                    return false;
                };
                self.front = owner;
                if self.tabs[owner].agent() != agent {
                    let agent = agent.clone();
                    self.request(&agent, true);
                }
                true
            }
            _ => false,
        }
    }

    /// Make the session follow the tab in front. A tab that has shown
    /// nothing yet opens on its first project with an agent listed, and so
    /// does one still showing an agent whose project moved to another tab,
    /// until the session shows it its own; with none, it asks for nothing.
    /// Nor does a tab in front waiting for the session to show it one of
    /// its agents: asking for the agent it shows now would undo that.
    pub(super) fn sync_focus(&mut self) {
        if self.front_awaits_agent() {
            return;
        }
        let shown = self.shown_by(self.front).to_string();
        if shown.is_empty() {
            if let Some(agent) = self.first_agent(self.front) {
                self.request(&agent, true);
            }
        } else {
            self.request(&shown, false);
        }
    }

    /// Whether the session was asked to switch to an agent of the tab in
    /// front other than the one it shows, and hasn't yet.
    pub(super) fn front_awaits_agent(&self) -> bool {
        self.focus_sent.as_deref().is_some_and(|sent| {
            sent != self.front().agent() && self.owner(sent) == Some(self.front)
        })
    }

    /// Whether the session has `agent` active, or was last asked to.
    pub(super) fn asked_for(&self, agent: &str) -> bool {
        match &self.focus_sent {
            Some(sent) => sent == agent,
            None => self.session_active == agent,
        }
    }

    /// The tab at `from` switched to `agent`, one of its own, on screen
    /// already (see `App::request_agent_switch`), and asks the session to
    /// follow: with the agent's chat (`load`) or without. Asking for the
    /// agent the tab shows first, as other events do, would turn the
    /// session to `agent` without its chat.
    ///
    /// The tab waits for the session's `SetActiveAgent`. When the session
    /// has `agent` active already and nothing is on its way, it would send
    /// none, so the answer is given here instead.
    pub(super) fn switch_in_tab(&mut self, from: usize, agent: String, load: bool) {
        if load {
            // Sent even to an agent the session has active: it sends the
            // chat of an agent selected again.
            self.focus_sent = Some(agent.clone());
            self.send(UiEvent::SelectAgent(agent));
        } else if !self.asked_for(&agent) {
            self.request(&agent, false);
        } else if self.focus_sent.is_none() {
            if let Some(tab) = self.tabs.get_mut(from) {
                tab.tui.process_command(UiCommand::SetActiveAgent(agent));
            }
        }
    }

    /// Ask the session to make `agent` its active agent, unless it is or
    /// was already asked to. `load` asks for the agent's chat as well,
    /// for a tab that doesn't hold it yet.
    pub(super) fn request(&mut self, agent: &str, load: bool) {
        if agent.is_empty() || self.asked_for(agent) {
            return;
        }
        self.focus_sent = Some(agent.to_string());
        self.send(if load {
            UiEvent::SelectAgent(agent.to_string())
        } else {
            UiEvent::FocusAgent(agent.to_string())
        });
    }

    pub(super) fn send(&self, event: UiEvent) {
        if let Err(e) = self.session.try_send(event) {
            tracing::warn!(error = %e, "the agent session did not take an event");
        }
    }
}
