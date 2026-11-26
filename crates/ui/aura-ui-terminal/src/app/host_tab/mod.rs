//! Where each agent runs, the footer switch that moves it, and the HOST
//! side tab: the vital facts of the host the agent on screen runs on.
//!
//! The switch sits at the right end of the footer, after the status
//! (`● Ready  ⌂ local`). Clicking it, `/cloud` and `/local` send
//! [`UiEvent::SetPlacement`] and show the move at once; the app answers
//! with [`crate::events::UiCommand::SetPlacement`] as the move starts and
//! again when it ends, which confirms the switch or puts it back. A busy
//! agent is not moved.

mod switch;
#[cfg(test)]
mod tests;

pub(crate) use switch::switch_button;
pub use switch::{switch_label, switch_rect, SWITCH_GAP};

use std::collections::HashMap;

use super::scroll::InfoScroll;
use super::{App, NotificationType, SidePanelTab};
use crate::events::UiEvent;
use crate::host::{HostAction, HostView, PlacementState, PlacementView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Cloud state the app keeps.
#[derive(Debug, Default)]
pub(crate) struct CloudUi {
    /// Placement of every agent the app was told about, by hex id.
    placements: HashMap<String, PlacementState>,
    /// The HOST tab of the agent on screen.
    host: Option<HostView>,
    /// How far the HOST tab is scrolled.
    pub(crate) host_scroll: InfoScroll,
    /// The switch's button in the last frame: its cells and padding.
    switch: Option<Rect>,
    /// The last move that landed, for the sweep across its new label.
    landed: Option<switch::Landing>,
    /// Whether cloud mode is set up, from the last placement update.
    cloud_ready: bool,
}

impl App {
    fn notify(&mut self, text: &str, kind: NotificationType) {
        self.notifications.current = Some((text.to_string(), kind));
    }

    /// Where the agent on screen runs, for the footer switch.
    #[must_use]
    pub fn placement(&self) -> PlacementState {
        self.host
            .cloud
            .placements
            .get(&self.active_agent_id)
            .cloned()
            .unwrap_or(PlacementState {
                cloud_ready: self.host.cloud.cloud_ready,
                ..PlacementState::default()
            })
    }

    /// The projects-list badge of `agent_id`: ` ☁ ` for a cloud agent,
    /// ` ☁  alice` for another user's, with whether it is someone else's.
    #[must_use]
    pub fn cloud_badge(&self, agent_id: &str) -> Option<(String, bool)> {
        let state = self.host.cloud.placements.get(agent_id)?;
        let mark = PlacementView::Cloud.mark();
        (state.placement == PlacementView::Cloud).then(|| match &state.owner {
            Some(owner) => (format!(" {mark} {owner}"), true),
            None => (format!(" {mark}"), false),
        })
    }

    /// Where `agent_id` runs.
    #[must_use]
    pub fn placement_of(&self, agent_id: &str) -> PlacementView {
        self.host
            .cloud
            .placements
            .get(agent_id)
            .map_or(PlacementView::Local, |state| state.placement)
    }

    /// The HOST tab of the agent on screen, when the app sent one.
    #[must_use]
    pub fn host_view(&self) -> Option<&HostView> {
        self.host
            .cloud
            .host
            .as_ref()
            .filter(|view| view.agent_id == self.active_agent_id)
    }

    /// Keep the HOST tab's scroll inside a document `total` lines long
    /// shown `visible` lines at a time, returning the first line to show.
    pub fn sync_host_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.host.cloud.host_scroll.sync(total, visible)
    }

    pub(super) fn cmd_set_placement(&mut self, agent_id: String, state: PlacementState) {
        self.host.cloud.cloud_ready = state.cloud_ready;
        let woke = !state.asleep && agent_id == self.active_agent_id && self.machine_asleep();
        if woke && self.sandbox.is_remote() {
            self.ticks.sandbox_stale = true;
        }
        self.note_landing(&agent_id, &state);
        self.host.cloud.placements.insert(agent_id, state);
    }

    /// Whether the agent on screen has a cloud machine that is asleep, so
    /// nothing is read from it.
    pub(crate) fn machine_asleep(&self) -> bool {
        self.host
            .cloud
            .placements
            .get(&self.active_agent_id)
            .is_some_and(|placement| placement.asleep)
    }

    pub(super) fn cmd_set_host(&mut self, view: HostView) {
        if self
            .host
            .cloud
            .host
            .as_ref()
            .is_none_or(|old| old.agent_id != view.agent_id)
        {
            self.host.cloud.host_scroll = InfoScroll::default();
        }
        self.host.cloud.host = Some(view);
    }

    /// Work a HOST button for the agent on screen.
    pub(super) fn request_host_action(&mut self, action: HostAction) {
        if action == HostAction::Flip {
            self.request_placement(self.placement().placement.flipped());
            return;
        }
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::HostAction {
                agent_id: self.active_agent_id.clone(),
                action,
            });
        }
    }

    /// `/cloud [setup|pull|sleep|wake]`, `/local` and `/host`.
    pub(super) fn handle_cloud_command(&mut self, cmd: &str, arg: &str) {
        match (cmd, arg) {
            ("local", _) => self.request_placement(PlacementView::Local),
            ("host", _) => self.show_side_tab(SidePanelTab::Host),
            ("cloud", "") => self.request_placement(PlacementView::Cloud),
            ("cloud", "setup") => {
                if let Some(tx) = &self.link.event_tx {
                    let _ = tx.try_send(UiEvent::CloudSetup);
                }
            }
            ("cloud", "pull") => self.request_host_action(HostAction::Pull),
            ("cloud", "sleep") => self.request_host_action(HostAction::Sleep),
            ("cloud", "wake") => self.request_host_action(HostAction::Wake),
            ("cloud", "host" | "status") => self.show_side_tab(SidePanelTab::Host),
            (_, other) => self.notify(
                &format!("Unknown /cloud option `{other}`: use setup, pull, sleep or wake"),
                NotificationType::Warning,
            ),
        }
    }

    /// Keys of the HOST tab: arrows scroll, a button's letter works it.
    /// Returns whether the key was the tab's.
    pub(super) fn host_key(&mut self, key: KeyEvent) -> bool {
        if self.host.cloud.host_scroll.key(key.code) {
            return true;
        }
        match key.code {
            KeyCode::Char(ch) => {
                let action = self
                    .host_view()
                    .and_then(|view| view.actions.iter().copied().find(|a| a.key() == ch));
                match action {
                    Some(action) => {
                        self.request_host_action(action);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Scroll the HOST tab by `delta` rows.
    pub(super) fn scroll_host(&mut self, delta: isize) {
        self.host.cloud.host_scroll.by(delta);
    }
}
