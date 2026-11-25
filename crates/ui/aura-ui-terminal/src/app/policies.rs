//! The Kernel panel's POLICIES tab: the policy the kernel of the agent on
//! screen enforces, as a read-only document.
//!
//! The event loop sends each agent its [`PolicyView`] with
//! [`UiCommand::SetPolicies`]; it is kept in the agent's HUD, so it moves
//! with the agent's chat when another agent comes on screen. The view is
//! a snapshot: the event loop sends it again when the agent is shown, its
//! mode or approval level changes, or an "always allow" rule is added, and
//! the tab asks for it whenever it is opened.
//!
//! [`UiCommand::SetPolicies`]: crate::events::UiCommand::SetPolicies

use super::{processes::SIDE_PAGE, App, PanelFocus, RecordsTab};
use crate::events::{PolicyView, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// The policy of the agent on screen, once the event loop sent it.
    #[must_use]
    pub fn policies(&self) -> Option<&PolicyView> {
        self.hud.policies.as_ref()
    }

    /// Ask the event loop for the policy of the agent on screen.
    pub(super) fn request_policies(&self) {
        if self.active_agent_id.is_empty() {
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::RequestPolicies(self.active_agent_id.clone()));
        }
    }

    /// Show the Kernel panel's POLICIES tab, focused, as `/policies` does.
    /// In the channel app this only says so (see
    /// [`Self::show_inbox`]).
    pub(super) fn show_policies(&mut self) {
        if self.channel_app() {
            self.notify_channel_owns_column();
            return;
        }
        self.record_panel_visible = true;
        self.side_expanded = false;
        if self.records_tab == RecordsTab::Policies {
            // Already open: still bring the snapshot up to date.
            self.request_policies();
        }
        self.show_records_tab(RecordsTab::Policies);
        self.focus = PanelFocus::Records;
    }

    /// Scroll the POLICIES tab by `delta` lines, as the wheel does. The
    /// renderer keeps the scroll inside the document.
    pub(super) fn scroll_policies(&mut self, delta: isize) {
        self.policies_scroll = self.policies_scroll.saturating_add_signed(delta);
    }

    /// Keep the POLICIES tab's scroll inside a document `total` lines long
    /// shown `visible` lines at a time, returning the first line to show.
    pub fn sync_policies_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.policies_scroll = self.policies_scroll.min(total.saturating_sub(visible));
        self.policies_scroll
    }

    /// Navigation keys of the POLICIES tab.
    pub(super) fn handle_policies_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.scroll_policies(-1),
            KeyCode::Down => self.scroll_policies(1),
            KeyCode::PageUp => self.scroll_policies(-SIDE_PAGE),
            KeyCode::PageDown => self.scroll_policies(SIDE_PAGE),
            _ => {}
        }
    }

    /// Home/End of the POLICIES tab, which would otherwise go to the
    /// empty input.
    pub(super) fn handle_policies_shortcut(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Home => self.policies_scroll = 0,
            KeyCode::End => self.policies_scroll = usize::MAX,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, UiCommand};
    use crate::mode_command::PermissionMode;
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn app_with_agents() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut app = App::new().with_event_sender(tx);
        let agent = |id: &str| AgentSummary {
            id: id.into(),
            name: id.to_uppercase(),
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![agent("aa"), agent("bb")]));
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        (app, rx)
    }

    fn policy_requests(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                UiEvent::RequestPolicies(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn shift_right_cycles_through_every_tab() {
        let (mut app, _rx) = app_with_agents();
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        assert!(app.handle_records_shortcut(right));
        assert_eq!(app.records_tab(), RecordsTab::Members);
        assert!(app.handle_records_shortcut(right));
        assert_eq!(app.records_tab(), RecordsTab::Policies);
        assert!(app.handle_records_shortcut(right));
        assert_eq!(app.records_tab(), RecordsTab::Inbox);
        assert!(app.handle_records_shortcut(right));
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        assert!(app.handle_records_shortcut(left));
        assert_eq!(app.records_tab(), RecordsTab::Inbox);
        assert!(app.handle_records_shortcut(left));
        assert_eq!(app.records_tab(), RecordsTab::Policies);
    }

    #[test]
    fn opening_the_tab_asks_for_the_policy_of_the_agent_on_screen() {
        let (mut app, mut rx) = app_with_agents();
        app.show_records_tab(RecordsTab::Members);
        assert!(policy_requests(&mut rx).is_empty(), "MEMBERS asks nothing");
        app.show_records_tab(RecordsTab::Policies);
        assert_eq!(policy_requests(&mut rx), ["aa"]);
        app.show_records_tab(RecordsTab::Policies);
        assert!(policy_requests(&mut rx).is_empty(), "already open");
    }

    #[test]
    fn the_policies_command_shows_the_tab_focused_and_refreshes_it() {
        let (mut app, mut rx) = app_with_agents();
        app.handle_command("/kernel");
        assert!(!app.record_panel_visible());
        app.handle_command("/policies");
        assert!(app.record_panel_visible());
        assert_eq!(app.records_tab(), RecordsTab::Policies);
        assert_eq!(app.focus(), PanelFocus::Records);
        assert_eq!(policy_requests(&mut rx), ["aa"]);
        app.handle_command("/perms");
        assert_eq!(policy_requests(&mut rx), ["aa"], "asked again");
    }

    #[test]
    fn each_agent_keeps_the_policy_sent_for_it() {
        let (mut app, _rx) = app_with_agents();
        assert!(app.policies().is_none());
        app.process_command(UiCommand::ForAgent {
            agent_id: "bb".into(),
            command: Box::new(UiCommand::SetPolicies(PolicyView {
                mode: PermissionMode::Plan,
                ..PolicyView::default()
            })),
        });
        assert!(app.policies().is_none(), "sent for the agent off screen");
        app.process_command(UiCommand::SetActiveAgent("bb".into()));
        assert_eq!(app.policies().map(|p| p.mode), Some(PermissionMode::Plan));
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        assert!(app.policies().is_none());
    }

    #[test]
    fn the_scroll_stays_inside_the_document() {
        let (mut app, _rx) = app_with_agents();
        app.show_records_tab(RecordsTab::Policies);
        app.focus = PanelFocus::Records;
        app.handle_key(KeyEvent::from(KeyCode::Down));
        app.handle_key(KeyEvent::from(KeyCode::PageDown));
        assert_eq!(app.sync_policies_scroll(30, 10), 1 + 10);
        assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::End)));
        assert_eq!(app.sync_policies_scroll(30, 10), 20);
        app.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(app.sync_policies_scroll(30, 10), 19);
        assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::Home)));
        assert_eq!(app.sync_policies_scroll(30, 10), 0);
        assert_eq!(app.sync_policies_scroll(5, 10), 0, "a short document");
    }
}
