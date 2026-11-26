//! Panels for a front end that draws their title bars itself: what each
//! bar shows, and the calls its tabs and dividers make.
//!
//! The grid keeps a framed, empty row where each title goes (see
//! [`App::set_host_chrome`]); the front end draws the title, the tabs and
//! the info over it, and maps clicks on them back here instead of passing
//! them to [`App::handle_mouse`].

use super::{AgentsTab, App, ChatTab, PanelFocus, RecordsTab, SidePanelTab};
use ratatui::{layout::Rect, style::Color};

/// The panels a front end can draw title bars for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Agents,
    Chat,
    Records,
    Side,
}

impl Pane {
    const fn focus(self) -> PanelFocus {
        match self {
            Self::Agents => PanelFocus::Swarm,
            Self::Chat => PanelFocus::Chat,
            Self::Records => PanelFocus::Records,
            Self::Side => PanelFocus::Sandbox,
        }
    }
}

/// A tab of any panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneTab {
    Agents(AgentsTab),
    Chat(ChatTab),
    Records(RecordsTab),
    Side(SidePanelTab),
}

impl PaneTab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agents(tab) => tab.label(),
            Self::Chat(tab) => tab.label(),
            Self::Records(tab) => tab.label(),
            Self::Side(tab) => tab.label(),
        }
    }

    /// Label for bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Agents(tab) => tab.short_label(),
            Self::Chat(tab) => tab.short_label(),
            Self::Records(tab) => tab.short_label(),
            Self::Side(tab) => tab.short_label(),
        }
    }

    const fn pane(self) -> Pane {
        match self {
            Self::Agents(_) => Pane::Agents,
            Self::Chat(_) => Pane::Chat,
            Self::Records(_) => Pane::Records,
            Self::Side(_) => Pane::Side,
        }
    }
}

/// What a panel's title bar shows: its tabs, or a title and an info text.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneChrome {
    pub pane: Pane,
    /// The whole panel, title bar and body, in screen cells
    pub area: Rect,
    /// The title bar's rows, framed and empty on the grid
    pub bar: Rect,
    pub tabs: Vec<PaneTab>,
    /// A tab drawn as its icon at the bar's end, left of the scope and
    /// expand buttons: HOST's computer
    pub icon_tab: Option<PaneTab>,
    pub active: Option<PaneTab>,
    /// Shown first, for panels without tabs
    pub title: Option<String>,
    /// Shown at the right end; `info_short` stands in when it doesn't fit
    pub info: Option<String>,
    pub info_short: Option<String>,
    pub info_color: Color,
    /// Whether the bar ends in a close button, which
    /// [`App::close_pane_view`] works
    pub closable: bool,
    /// Whether the bar has a button that expands the panel to fill the
    /// screen, which [`App::expand_pane`] works; `Some(true)` while it is
    /// expanded
    pub expand: Option<bool>,
    /// Whether the bar ends in a [`crate::SideScope::BUTTON`] box, which
    /// [`App::toggle_pane_scope`] works; `Some(true)` while the panel's
    /// lists show the whole project's items rather than the active agent's
    pub scope: Option<bool>,
    pub focused: bool,
}

/// Where the dividers between panels sit, as shares of the room they
/// split. `None` keeps the default share.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaneSplits {
    /// Width of the agents column, of the panels' width
    pub agents: Option<f32>,
    /// Width of the right column, of the panels' width
    pub right: Option<f32>,
    /// Height of the record panel, of the right column's height, when the
    /// panel below shares the column
    pub records: Option<f32>,
}

impl App {
    /// Leave the panels' title bars to the front end: the grid keeps a
    /// framed, empty row for each (see [`Self::pane_chrome`]).
    pub fn set_host_chrome(&mut self, on: bool) {
        self.host_chrome = on;
    }

    #[must_use]
    pub const fn host_chrome(&self) -> bool {
        self.host_chrome
    }

    /// The title bars of the panels in the last frame, while the front end
    /// draws them.
    #[must_use]
    pub fn pane_chrome(&self) -> &[PaneChrome] {
        &self.pane_chrome
    }

    pub(crate) fn clear_pane_chrome(&mut self) {
        self.pane_chrome.clear();
    }

    pub(crate) fn push_pane_chrome(&mut self, chrome: PaneChrome) {
        self.pane_chrome.push(chrome);
    }

    /// Move the dividers between panels; the next frame lays them out so.
    pub fn set_pane_splits(&mut self, splits: PaneSplits) {
        self.pane_splits = splits;
    }

    #[must_use]
    pub const fn pane_splits(&self) -> PaneSplits {
        self.pane_splits
    }

    /// Show `tab` and focus its panel, as clicking it in a drawn bar does.
    pub fn select_pane_tab(&mut self, tab: PaneTab) {
        self.focus_pane(tab.pane());
        match tab {
            PaneTab::Agents(tab) => self.show_agents_tab(tab),
            PaneTab::Chat(tab) => {
                if tab != self.chat_tab() {
                    self.show_chat_tab(tab);
                }
            }
            PaneTab::Records(tab) => self.show_records_tab(tab),
            PaneTab::Side(tab) => self.show_side_tab(tab),
        }
    }

    /// Focus `pane`, as clicking its title bar does.
    pub fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane.focus();
        self.clear_selection();
    }

    /// Close what covers `pane` and focus it, as its bar's close button
    /// does: on the chat, a view over it, else a channel member's chat.
    pub fn close_pane_view(&mut self, pane: Pane) {
        self.focus_pane(pane);
        if pane == Pane::Chat {
            self.close_chat_view();
        }
    }

    /// Expand `pane` to fill the screen, or the side panel the right
    /// column, or restore it, and focus it, as its bar's expand button
    /// does.
    pub fn expand_pane(&mut self, pane: Pane) {
        self.focus_pane(pane);
        match pane {
            Pane::Chat => self.toggle_chat_expanded(),
            Pane::Side => self.toggle_side_expanded(),
            Pane::Agents | Pane::Records => {}
        }
    }

    /// Switch `pane`'s lists between the active agent's items and the
    /// whole project's, and focus it, as its bar's scope box does.
    pub fn toggle_pane_scope(&mut self, pane: Pane) {
        self.focus_pane(pane);
        if pane == Pane::Side {
            self.toggle_side_scope();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_tab_shows_it_and_focuses_its_panel() {
        let mut app = App::new();
        app.select_pane_tab(PaneTab::Side(SidePanelTab::Trace));
        assert_eq!(app.side_tab(), SidePanelTab::Trace);
        assert_eq!(app.focus(), PanelFocus::Sandbox);

        app.select_pane_tab(PaneTab::Chat(ChatTab::Terminal));
        assert_eq!(app.chat_tab(), ChatTab::Terminal);
        assert_eq!(app.focus(), PanelFocus::Chat);

        app.select_pane_tab(PaneTab::Agents(AgentsTab::Roster));
        assert_eq!(app.agents_tab(), AgentsTab::Roster);
        assert_eq!(app.focus(), PanelFocus::Swarm);

        app.select_pane_tab(PaneTab::Records(RecordsTab::Members));
        assert_eq!(app.records_tab(), RecordsTab::Members);
        assert_eq!(app.focus(), PanelFocus::Records);

        app.focus_pane(Pane::Chat);
        app.focus_pane(Pane::Records);
        assert_eq!(app.focus(), PanelFocus::Records);
        assert_eq!(
            app.records_tab(),
            RecordsTab::Members,
            "focus keeps the tab"
        );
    }

    #[test]
    fn closing_the_chat_pane_view_uncovers_the_chat() {
        let mut app = App::new();
        app.open_detail(super::super::DetailView::Graph);
        app.close_pane_view(Pane::Chat);
        assert!(app.detail_view().is_none());
        assert_eq!(app.focus(), PanelFocus::Chat);
    }

    #[test]
    fn the_chat_expands_over_the_other_panels_and_back() {
        let mut app = App::new();
        app.focus_pane(Pane::Records);
        app.expand_pane(Pane::Chat);
        assert!(app.chat_expanded());
        assert_eq!(app.focus(), PanelFocus::Chat);
        assert!(!app.swarm_panel_visible());
        assert!(!app.record_panel_visible());
        assert!(!app.sandbox_panel_visible());

        app.expand_pane(Pane::Chat);
        assert!(!app.chat_expanded());
        assert!(app.swarm_panel_visible());
        assert!(app.record_panel_visible());
        assert!(app.sandbox_panel_visible());
    }

    #[test]
    fn the_side_panel_maximizes_over_the_record_panel_and_back() {
        let mut app = App::new();
        let splits = PaneSplits {
            records: Some(0.3),
            ..PaneSplits::default()
        };
        app.set_pane_splits(splits);
        app.focus_pane(Pane::Records);
        app.expand_pane(Pane::Side);
        assert!(app.side_expanded());
        assert_eq!(app.focus(), PanelFocus::Sandbox);
        assert!(!app.record_panel_visible());
        assert!(app.sandbox_panel_visible());
        assert!(app.swarm_panel_visible());

        // The chat expands over it and gives it back maximized.
        app.expand_pane(Pane::Chat);
        assert!(!app.sandbox_panel_visible());
        app.expand_pane(Pane::Chat);
        assert!(app.side_expanded());
        assert!(!app.record_panel_visible());

        app.expand_pane(Pane::Side);
        assert!(!app.side_expanded());
        assert!(app.record_panel_visible());
        assert_eq!(app.pane_splits(), splits, "the split comes back");
    }

    #[test]
    fn hiding_the_maximized_side_panel_brings_the_record_panel_back() {
        let mut app = App::new();
        app.expand_pane(Pane::Side);
        let _ = app.handle_command("/files");
        assert!(!app.side_expanded());
        assert!(app.record_panel_visible());

        let _ = app.handle_command("/files");
        app.expand_pane(Pane::Side);
        let _ = app.handle_command("/kernel");
        assert!(!app.side_expanded(), "/kernel shows the record panel");
        assert!(app.record_panel_visible());
        assert!(app.sandbox_panel_visible());
    }

    #[test]
    fn tab_labels_come_from_their_panel() {
        let tab = PaneTab::Side(SidePanelTab::Processes);
        assert_eq!(tab.label(), "PROCESSES");
        assert_eq!(tab.short_label(), "PRC");
    }
}
