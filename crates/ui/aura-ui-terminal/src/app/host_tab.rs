//! Where each agent runs, the footer switch that moves it, and the HOST
//! side tab: the vital facts of the host the agent on screen runs on.
//!
//! The switch sits at the right end of the footer, after the status
//! (`● Ready  ⌂ local`). Clicking it, `/cloud` and `/local` send
//! [`UiEvent::SetPlacement`]; the app answers with
//! [`crate::events::UiCommand::SetPlacement`] once the move is under way
//! and again when it is done, so the switch only shows where the agent
//! really is. A busy agent is not moved.

use std::collections::HashMap;

use super::{contains, App, NotificationType, SidePanelTab};
use crate::events::UiEvent;
use crate::host::{HostAction, HostView, PlacementState, PlacementView};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Columns kept between the status and the switch.
pub const SWITCH_GAP: u16 = 2;

/// Cloud state the app keeps.
#[derive(Debug, Default)]
pub(crate) struct CloudUi {
    /// Placement of every agent the app was told about, by hex id.
    placements: HashMap<String, PlacementState>,
    /// The HOST tab of the agent on screen.
    host: Option<HostView>,
    /// First row of the HOST tab shown.
    pub(crate) host_scroll: usize,
    /// The footer row of the last frame.
    footer: Option<Rect>,
    /// Whether cloud mode is set up, from the last placement update.
    cloud_ready: bool,
}

/// The switch's text for `state`.
#[must_use]
pub fn switch_label(state: &PlacementState) -> String {
    if let Some(moving) = &state.moving {
        return format!("◐ {moving}");
    }
    if let Some(owner) = state
        .owner
        .as_deref()
        .filter(|_| state.placement == PlacementView::Cloud)
    {
        return format!("{} {owner}", PlacementView::Cloud.mark());
    }
    let mut label = format!("{} {}", state.placement.mark(), state.placement.label());
    if let Some(machine) = state
        .machine_state
        .as_deref()
        .filter(|_| state.placement == PlacementView::Cloud)
    {
        if machine != "ready" {
            label.push_str(" · ");
            label.push_str(machine);
        }
    }
    label
}

/// Where the switch goes in the footer `row`: its right end.
#[must_use]
pub fn switch_rect(row: Rect, label: &str) -> Option<Rect> {
    let width = u16::try_from(unicode_width::UnicodeWidthStr::width(label)).unwrap_or(u16::MAX);
    if row.width < width + SWITCH_GAP + 8 || row.height == 0 {
        return None;
    }
    Some(Rect {
        x: row.right() - width,
        width,
        height: 1,
        ..row
    })
}

impl App {
    fn notify(&mut self, text: &str, kind: NotificationType) {
        self.notification = Some((text.to_string(), kind));
    }

    /// Where the agent on screen runs, for the footer switch.
    #[must_use]
    pub fn placement(&self) -> PlacementState {
        self.cloud
            .placements
            .get(&self.active_agent_id)
            .cloned()
            .unwrap_or(PlacementState {
                cloud_ready: self.cloud.cloud_ready,
                ..PlacementState::default()
            })
    }

    /// The projects-list badge of `agent_id`: ` ☁ ` for a cloud agent,
    /// ` ☁  alice` for another user's, with whether it is someone else's.
    #[must_use]
    pub fn cloud_badge(&self, agent_id: &str) -> Option<(String, bool)> {
        let state = self.cloud.placements.get(agent_id)?;
        let mark = PlacementView::Cloud.mark();
        (state.placement == PlacementView::Cloud).then(|| match &state.owner {
            Some(owner) => (format!(" {mark} {owner}"), true),
            None => (format!(" {mark}"), false),
        })
    }

    /// Where `agent_id` runs.
    #[must_use]
    pub fn placement_of(&self, agent_id: &str) -> PlacementView {
        self.cloud
            .placements
            .get(agent_id)
            .map_or(PlacementView::Local, |state| state.placement)
    }

    /// The HOST tab of the agent on screen, when the app sent one.
    #[must_use]
    pub fn host_view(&self) -> Option<&HostView> {
        self.cloud
            .host
            .as_ref()
            .filter(|view| view.agent_id == self.active_agent_id)
    }

    /// First row of the HOST tab shown.
    #[must_use]
    pub const fn host_scroll(&self) -> usize {
        self.cloud.host_scroll
    }

    /// Note the footer row drawn this frame, for clicks on the switch.
    pub fn set_footer_area(&mut self, area: Rect) {
        self.cloud.footer = Some(Rect { height: 1, ..area });
    }

    pub(super) fn cmd_set_placement(&mut self, agent_id: String, state: PlacementState) {
        self.cloud.cloud_ready = state.cloud_ready;
        self.cloud.placements.insert(agent_id, state);
    }

    pub(super) fn cmd_set_host(&mut self, view: HostView) {
        if self
            .cloud
            .host
            .as_ref()
            .is_none_or(|old| old.agent_id != view.agent_id)
        {
            self.cloud.host_scroll = 0;
        }
        self.cloud.host = Some(view);
    }

    /// Ask for the agent on screen to run at `placement`.
    pub(super) fn request_placement(&mut self, placement: PlacementView) {
        let current = self.placement();
        if let Some(owner) = current
            .owner
            .as_deref()
            .filter(|_| current.placement == PlacementView::Cloud)
        {
            let note = format!("Placed by {owner}; sign in as {owner} to manage it");
            self.notify(&note, NotificationType::Warning);
            return;
        }
        if current.moving.is_some() {
            self.notify("A move is already under way", NotificationType::Success);
            return;
        }
        if current.placement == placement {
            let note = format!(
                "This agent already runs {}",
                match placement {
                    PlacementView::Local => "on this machine",
                    PlacementView::Cloud => "in the cloud",
                }
            );
            self.notify(&note, NotificationType::Success);
            return;
        }
        if self.is_busy() {
            self.notify(
                "The agent is working; stop it or wait before moving it",
                NotificationType::Warning,
            );
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetPlacement {
                agent_id: self.active_agent_id.clone(),
                placement,
            });
        }
    }

    /// Work a HOST button for the agent on screen.
    pub(super) fn request_host_action(&mut self, action: HostAction) {
        if action == HostAction::Flip {
            self.request_placement(self.placement().placement.flipped());
            return;
        }
        if let Some(tx) = &self.event_tx {
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
                if let Some(tx) = &self.event_tx {
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

    /// A left press on the footer switch flips the agent's placement.
    /// Returns whether the press was on it.
    pub(super) fn click_placement_switch(&mut self, mouse: MouseEvent) -> bool {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let Some(row) = self.cloud.footer else {
            return false;
        };
        let state = self.placement();
        let on_switch = switch_rect(row, &switch_label(&state))
            .is_some_and(|rect| contains(Some(rect), mouse.column, mouse.row));
        if !on_switch {
            return false;
        }
        self.request_placement(state.placement.flipped());
        true
    }

    /// Keys of the HOST tab: arrows scroll, a button's letter works it.
    /// Returns whether the key was the tab's.
    pub(super) fn host_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => {
                self.cloud.host_scroll = self.cloud.host_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                self.cloud.host_scroll += 1;
                true
            }
            KeyCode::PageUp => {
                self.cloud.host_scroll = self
                    .cloud
                    .host_scroll
                    .saturating_sub(super::processes::SIDE_PAGE.unsigned_abs());
                true
            }
            KeyCode::PageDown => {
                self.cloud.host_scroll += super::processes::SIDE_PAGE.unsigned_abs();
                true
            }
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
        self.cloud.host_scroll = self.cloud.host_scroll.saturating_add_signed(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;
    use crossterm::event::KeyModifiers;

    fn app() -> (App, tokio::sync::mpsc::Receiver<UiEvent>) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut app = App::new();
        app.event_tx = Some(tx);
        app.active_agent_id = "aa".into();
        (app, rx)
    }

    #[test]
    fn labels_show_where_and_how() {
        assert_eq!(switch_label(&PlacementState::default()), "⌂ local");
        let cloud = PlacementState {
            placement: PlacementView::Cloud,
            machine_state: Some("asleep".into()),
            ..PlacementState::default()
        };
        assert_eq!(switch_label(&cloud), "☁  cloud · asleep");
        let moving = PlacementState {
            moving: Some("moving to cloud".into()),
            ..PlacementState::default()
        };
        assert_eq!(switch_label(&moving), "◐ moving to cloud");
        let foreign = PlacementState {
            placement: PlacementView::Cloud,
            owner: Some("Alice".into()),
            ..PlacementState::default()
        };
        assert_eq!(switch_label(&foreign), "☁  Alice");
    }

    #[test]
    fn another_users_cloud_agent_is_marked_and_not_moved() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetPlacement {
            agent_id: "aa".into(),
            state: PlacementState {
                placement: PlacementView::Cloud,
                owner: Some("Alice".into()),
                ..PlacementState::default()
            },
        });
        app.process_command(UiCommand::SetPlacement {
            agent_id: "bb".into(),
            state: PlacementState {
                placement: PlacementView::Cloud,
                ..PlacementState::default()
            },
        });
        assert_eq!(app.cloud_badge("aa"), Some((" ☁  Alice".into(), true)));
        assert_eq!(app.cloud_badge("bb"), Some((" ☁ ".into(), false)));
        assert_eq!(app.cloud_badge("cc"), None);
        app.handle_command("/local");
        assert!(rx.try_recv().is_err(), "only Alice can bring it back");
        assert!(app
            .notification
            .as_ref()
            .is_some_and(|(text, _)| text.contains("Placed by Alice")));
    }

    #[test]
    fn cloud_and_local_commands_ask_for_a_move() {
        let (mut app, mut rx) = app();
        app.handle_command("/cloud");
        assert!(matches!(
            rx.try_recv(),
            Ok(UiEvent::SetPlacement { agent_id, placement: PlacementView::Cloud }) if agent_id == "aa"
        ));
        app.handle_command("/local");
        assert!(rx.try_recv().is_err(), "already local: nothing to do");
        app.handle_command("/cloud setup");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::CloudSetup)));
    }

    #[test]
    fn a_move_under_way_is_not_asked_for_again() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetPlacement {
            agent_id: "aa".into(),
            state: PlacementState {
                moving: Some("moving to cloud".into()),
                ..PlacementState::default()
            },
        });
        app.handle_command("/cloud");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn clicking_the_switch_flips_the_agent() {
        let (mut app, mut rx) = app();
        let row = Rect::new(0, 30, 100, 1);
        app.set_footer_area(row);
        let rect = switch_rect(row, &switch_label(&app.placement())).expect("fits");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: 30,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.click_placement_switch(click));
        assert!(matches!(
            rx.try_recv(),
            Ok(UiEvent::SetPlacement {
                placement: PlacementView::Cloud,
                ..
            })
        ));
        let beside = MouseEvent { column: 2, ..click };
        assert!(!app.click_placement_switch(beside));
    }

    #[test]
    fn host_buttons_work_by_key() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetHost(Box::new(HostView {
            agent_id: "aa".into(),
            placement: PlacementView::Cloud,
            actions: vec![HostAction::Pull, HostAction::Sleep],
            ..HostView::default()
        })));
        assert!(app.host_view().is_some());
        assert!(app.host_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)));
        assert!(matches!(
            rx.try_recv(),
            Ok(UiEvent::HostAction {
                action: HostAction::Pull,
                ..
            })
        ));
        assert!(!app.host_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)));
    }

    #[test]
    fn another_agents_host_view_is_not_shown() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetHost(Box::new(HostView {
            agent_id: "bb".into(),
            ..HostView::default()
        })));
        assert!(app.host_view().is_none());
    }
}
