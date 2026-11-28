//! Bringing back what the agents panel had selected in the last run.
//!
//! A front end loads the saved [`LastSelection`] and hands it to
//! [`App::restore_selection`] before the session's commands arrive. The
//! picks seed PROJECTS' memory, and the channel CHANNELS', at once. The
//! rest waits for the session to put its first agent on screen, once the
//! lists and channels it sends first are in:
//!
//! - CHANNELS in front, with its channel still listed: CHANNELS comes to
//!   the front with that channel open.
//! - Otherwise, while PROJECTS is in front: the latest pick whose agent
//!   is still listed comes on screen.
//! - Nothing of it still listed: the screen stays as the session opened
//!   it, as without a saved selection.
//!
//! [`App::last_selection`] is what to save when the app closes.

#[cfg(test)]
mod tests;

use super::{AgentsTab, App, LastSelection};
use crate::app::last_selection::note_pick;
use crate::events::AgentSummary;

impl App {
    /// Bring back `saved`, a selection [`Self::last_selection`] gave in
    /// an earlier run. Ids no longer listed are passed over.
    pub fn restore_selection(&mut self, saved: LastSelection) {
        self.apps.project_picks.clone_from(&saved.projects);
        self.apps.channel.clone_from(&saved.channel);
        self.apps.restore = Some(saved);
    }

    /// What the agents panel has selected now, to save for the next run's
    /// [`Self::restore_selection`]. Until a restored selection has been
    /// brought back, that selection.
    #[must_use]
    pub fn last_selection(&self) -> LastSelection {
        if let Some(saved) = &self.apps.restore {
            return saved.clone();
        }
        let open = self.open_channel().map(|channel| channel.id.clone());
        let mut selection = LastSelection {
            channels_in_front: self.channel_app(),
            channel: open.or_else(|| self.apps.channel.clone()),
            projects: self.apps.project_picks.clone(),
        };
        if self.agents_tab == AgentsTab::Projects {
            if let Some(project) = self.listed_project_of(&self.active_agent_id) {
                selection.note_pick(project, &self.active_agent_id);
            }
        }
        selection
    }

    /// The agent PROJECTS was last left on that is still listed: the
    /// latest of its picks, one per project.
    #[must_use]
    pub fn remembered_agent(&self) -> Option<&str> {
        self.apps
            .project_picks
            .iter()
            .map(|pick| pick.agent.as_str())
            .find(|agent| self.is_listed(agent))
    }

    /// Note the listed `agent_id` as the one PROJECTS was left on in its
    /// project. Not while a restored selection waits to be brought back:
    /// the agent the session opens on is not the user's pick.
    pub(super) fn note_project_pick(&mut self, agent_id: &str) {
        if self.apps.restore.is_some() {
            return;
        }
        if let Some(project) = self.listed_project_of(agent_id).map(str::to_string) {
            note_pick(&mut self.apps.project_picks, &project, agent_id);
        }
    }

    /// Key of the project the listed `agent_id` is in.
    fn listed_project_of(&self, agent_id: &str) -> Option<&str> {
        self.agents
            .iter()
            .find(|agent| agent.id == agent_id)
            .map(AgentSummary::project_key)
    }

    /// The session put an agent on screen. The first time after
    /// [`Self::restore_selection`], bring the selection back (see the
    /// module docs).
    pub(in crate::app) fn bring_back_selection(&mut self) {
        let Some(saved) = self.apps.restore.take() else {
            return;
        };
        if saved.channels_in_front && self.channel_to_reopen().is_some() {
            self.show_agents_tab(AgentsTab::Channels);
            // Leaving PROJECTS noted the agent the session opened on as
            // its pick; the last run's picks are the user's.
            self.apps.project_picks = saved.projects;
            return;
        }
        if self.agents_tab != AgentsTab::Projects {
            return;
        }
        if let Some(agent) = self.remembered_agent().map(str::to_string) {
            self.request_agent_switch(agent);
        }
    }
}
