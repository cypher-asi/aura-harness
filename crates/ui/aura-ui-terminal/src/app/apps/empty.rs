//! What the app in front says when it has nothing to show, or no agent:
//! the chat's title and message, the input's placeholder, and why a
//! message cannot be sent.

use super::AgentsTab;
use crate::app::{App, NotificationType};

impl App {
    /// Whether the app in front has nothing to show: no channel open, no
    /// instance of the permanent agent picked, no benchmark or form, no
    /// archived agent opened. The chat says so, and the right column
    /// hides.
    #[must_use]
    pub(crate) fn app_empty(&self) -> bool {
        match self.agents_tab {
            AgentsTab::Channels => !self.channel_open(),
            AgentsTab::Projects => false,
            AgentsTab::Roster => !self.roster_shows_instance(),
            AgentsTab::Bench => !self.bench.showing(),
            AgentsTab::Archive => !self.is_archived_agent(&self.active_agent_id),
            AgentsTab::Integrations => true,
        }
    }

    /// Whether no agent is on screen: BENCH shows a benchmark or the form,
    /// and an app showing nothing shows no agent either. The agent's side
    /// column, queue, stop button, todos and benchmark progress stay away.
    #[must_use]
    pub(crate) fn no_agent_on_screen(&self) -> bool {
        self.bench_draws_center() || self.app_empty()
    }

    /// Whether the app in front shows only CHAT of the chat panel's tabs:
    /// CHANNELS, and BENCH but for an agent of a benchmark.
    #[must_use]
    pub(crate) fn chat_only(&self) -> bool {
        !self.has_agent_tabs(self.agents_tab)
    }

    /// Whether the agent panels (`/kernel`, `/files`, `/inbox`, ...) stay
    /// off screen: CHANNELS has its own, and BENCH shows no agent but for
    /// one of a benchmark's.
    pub(in crate::app) fn agent_panels_hidden(&self) -> bool {
        self.channel_app() || self.bench_draws_center()
    }

    /// Say why the agent panels are not shown ([`Self::agent_panels_hidden`]).
    pub(in crate::app) fn notify_no_agent_panels(&mut self) {
        if self.bench_app() {
            self.notifications.current = Some((
                "BENCH shows no agent here; pick one of a benchmark's for agent panels".to_string(),
                NotificationType::Warning,
            ));
        } else {
            self.notify_channel_owns_column();
        }
    }

    /// The chat panel's title in an app without the agent's tabs, and
    /// what it says beside it: the open channel's `#slug` and title, or
    /// the benchmark BENCH shows and its title. `None` elsewhere, or with
    /// nothing open.
    #[must_use]
    pub(crate) fn app_chat_title(&self) -> Option<(String, String)> {
        match self.agents_tab {
            AgentsTab::Channels => self.channel_chat_title(),
            AgentsTab::Bench => self.bench_chat_title(),
            _ => None,
        }
    }

    /// What the chat says while [`Self::app_empty`].
    #[must_use]
    pub(crate) fn app_empty_message(&self) -> String {
        match self.agents_tab {
            AgentsTab::Channels if self.channels().is_empty() => {
                "No channels yet. Start one with + New channel on the left.".to_string()
            }
            AgentsTab::Channels => "Pick a channel on the left to open its chat.".to_string(),
            AgentsTab::Roster if self.definitions.is_empty() => {
                "No permanent agents yet. Make one with + New agent on the left.".to_string()
            }
            AgentsTab::Roster => match self.apps.roster.as_deref() {
                Some(name) if self.latest_instance(name).is_none() => {
                    super::roster::no_instance_message(name)
                }
                _ => "Pick a permanent agent on the left to open its latest chat.".to_string(),
            },
            AgentsTab::Bench => self.bench_empty_message().to_string(),
            AgentsTab::Archive if self.archived.is_empty() => "Nothing is archived.".to_string(),
            AgentsTab::Archive => {
                "Pick an archived agent on the left to open its chat.".to_string()
            }
            AgentsTab::Projects => String::new(),
            AgentsTab::Integrations => "Pick an integration on the left: c connects it, e changes \
                                        a setting, t tests it, d disconnects it."
                .to_string(),
        }
    }

    /// What there is to pick in the app in front, as "Pick …" says it.
    fn app_pick(&self) -> &'static str {
        match self.agents_tab {
            AgentsTab::Channels => "a channel",
            AgentsTab::Roster => "a permanent agent",
            AgentsTab::Bench => "a benchmark",
            AgentsTab::Archive => "an archived agent",
            AgentsTab::Projects => "an agent",
            AgentsTab::Integrations => "an integration",
        }
    }

    /// What the empty input says: who a message goes to in the channel
    /// app, what BENCH takes while it shows no agent, or what to pick
    /// while the app in front shows nothing. `None` otherwise.
    #[must_use]
    pub(crate) fn app_placeholder(&self) -> Option<String> {
        if self.channel_app() {
            return self.channel_placeholder();
        }
        if self.bench_draws_center() {
            return Some(self.bench_placeholder());
        }
        self.app_empty()
            .then(|| format!("Pick {}", self.app_pick()))
    }

    /// Why a message cannot be sent: a benchmark has no chat, and an app
    /// showing nothing has nobody to send it to.
    pub(in crate::app) fn app_send_refusal(&self) -> String {
        if self.bench_draws_center() {
            return "BENCH has no chat here; pick one of a benchmark's agents to message it"
                .to_string();
        }
        format!("Pick {} to send a message", self.app_pick())
    }
}
