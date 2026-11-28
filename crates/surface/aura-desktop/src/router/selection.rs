//! The window's selection between runs: what each tab's UI had selected
//! (see [`LastSelection`]), as one file for all of them.
//!
//! - Every tab's UI is given the saved picks, so a tab comes back on the
//!   agent it was left on in its projects, and `Tabs::first_agent`
//!   shows that agent when the tab is first brought to the front.
//! - Only the tab the session first shows an agent in may bring CHANNELS
//!   back to the front with the channel that was open: the session has
//!   one active agent, and two tabs opening the channel would fight over
//!   it.
//! - Saving takes the channel from the tab in front, and each project's
//!   pick from the tab holding the project. Picks of projects in no tab
//!   are kept from the last run, for a project that comes back.

use super::Tabs;
use aura_ui_terminal::{LastSelection, ProjectPick};

/// The last run's selection, as handed out to the tabs.
#[derive(Debug, Default)]
pub(super) struct Remembered {
    /// For every tab: its picks, and the channel to open on going to
    /// CHANNELS
    for_tabs: LastSelection,
    /// The whole selection, for the tab the session first shows an agent
    /// in; `None` once handed out
    for_first_shown: Option<LastSelection>,
    /// Picks of the last run, for projects no tab holds
    picks: Vec<ProjectPick>,
}

impl Tabs {
    /// Bring back `saved`, the selection [`Self::last_selection`] gave in
    /// the last run. Call before the session's commands are applied.
    pub fn remember(&mut self, saved: LastSelection) {
        let for_tabs = saved.clone().without_channels_in_front();
        for tab in &mut self.tabs {
            tab.tui.restore_selection(for_tabs.clone());
        }
        self.remembered = Remembered {
            picks: saved.projects.clone(),
            for_tabs,
            for_first_shown: Some(saved),
        };
    }

    /// Hand the tab at `index`, which a new tab is, the picks to come
    /// back on.
    pub(super) fn give_remembered(&mut self, index: usize) {
        let saved = &self.remembered.for_tabs;
        if *saved == LastSelection::default() {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.tui.restore_selection(saved.clone());
        }
    }

    /// The session is showing its first agent, in the tab at `index`:
    /// that tab may bring back CHANNELS too.
    pub(super) fn give_first_shown(&mut self, index: usize) {
        let Some(saved) = self.remembered.for_first_shown.take() else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.tui.restore_selection(saved);
        }
    }

    /// The agent the tab at `index` was last left on in its projects, if
    /// it still holds it.
    pub(super) fn remembered_in(&self, index: usize) -> Option<String> {
        let agent = self.tabs.get(index)?.tui.remembered_agent()?;
        (self.owner(agent) == Some(index)).then(|| agent.to_string())
    }

    /// What the window has selected now, to save for the next run.
    pub fn last_selection(&self) -> LastSelection {
        let front = self.front().tui.last_selection();
        let mut picks: Vec<ProjectPick> = Vec::new();
        let others = (0..self.tabs.len()).filter(|&index| index != self.front);
        for index in std::iter::once(self.front).chain(others) {
            let tab = &self.tabs[index];
            let selection = if index == self.front {
                front.projects.clone()
            } else {
                tab.tui.last_selection().projects
            };
            let held = selection
                .into_iter()
                .filter(|pick| tab.projects.contains(&pick.project));
            push_new(&mut picks, held);
        }
        let unheld = self.remembered.picks.iter().filter(|pick| {
            !self
                .tabs
                .iter()
                .any(|tab| tab.projects.contains(&pick.project))
        });
        push_new(&mut picks, unheld.cloned());
        LastSelection {
            projects: picks,
            ..front
        }
    }
}

/// Add to `picks` those of `more` for projects not picked yet.
fn push_new(picks: &mut Vec<ProjectPick>, more: impl IntoIterator<Item = ProjectPick>) {
    for pick in more {
        if !picks.iter().any(|known| known.project == pick.project) {
            picks.push(pick);
        }
    }
}
