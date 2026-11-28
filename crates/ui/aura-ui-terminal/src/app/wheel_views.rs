//! The wheel scrolls a list's rows, never its highlight.
//!
//! Each list keeps the first row it shows next to its highlight, and every
//! frame scrolls as little as needed to keep the highlight in view (see
//! [`follow_selection`]). The wheel moves that first row instead and
//! leaves the list where it put it, the highlight possibly off screen,
//! until the highlight moves (a key, a click, the session jumping to
//! another agent) or any key is pressed; then the list follows its
//! highlight again.
//!
//! The model selector and the folder browser, lists in dialogs, are
//! scrolled the same way (see `modal`).
//!
//! Every list goes through here: the wheel calls [`App::wheel_list`] (or
//! [`WheelViews::detach`]) and never a function that moves a highlight,
//! and each frame places the list with [`WheelViews::place`]. The
//! documents that scroll by lines (POLICIES, HOST, a channel's INFO and
//! QUEUE) have no highlight, so they scroll as they always have.
//! `app/tests/wheel_every_list.rs` turns the wheel over every tab of
//! every panel and does not build until a new tab is part of it.

use super::{follow_selection, AgentsTab, App, PanelFocus, RecordsTab, SidePanelTab};

/// The lists whose rows the wheel scrolls apart from their highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WheelList {
    Channels,
    Projects,
    Roster,
    Bench,
    Archive,
    Kernel,
    Members,
    Inbox,
    Sandbox,
    Repo,
    Processes,
    Trace,
    Tasks,
    ChannelMembers,
    ChannelHistory,
    /// The model selector's list, in its dialog
    ModelPicker,
    /// The folder browser's list, in its dialog
    FolderPicker,
}

/// A list the wheel scrolled, and where its highlight was when it was
/// first drawn so: `None` until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Detached {
    list: WheelList,
    highlight: Option<usize>,
}

/// The lists the wheel scrolled off their highlight.
#[derive(Debug, Default)]
pub(crate) struct WheelViews {
    detached: Vec<Detached>,
}

impl WheelViews {
    /// Leave `list` where the wheel puts it until its highlight moves.
    pub(crate) fn detach(&mut self, list: WheelList) {
        if !self.detached.iter().any(|d| d.list == list) {
            self.detached.push(Detached {
                list,
                highlight: None,
            });
        }
    }

    /// Every list follows its highlight again.
    pub(crate) fn clear(&mut self) {
        self.detached.clear();
    }

    /// Whether `list` is left where the wheel put it.
    pub(crate) fn is_detached(&self, list: WheelList) -> bool {
        self.detached.iter().any(|d| d.list == list)
    }

    /// The first row `list` shows: `scroll` as the wheel left it, kept
    /// inside the `total` rows, while its highlight is still at
    /// `selected`; else moved as little as needed to show `selected` in
    /// `visible` rows.
    pub(crate) fn place(
        &mut self,
        list: WheelList,
        (scroll, selected): (usize, usize),
        (visible, total): (usize, usize),
    ) -> usize {
        self.place_keyed(list, selected, (scroll, selected), (visible, total))
    }

    /// [`Self::place`], telling whether the highlight moved by `key`
    /// rather than by its row: a highlight that follows new rows as they
    /// come in moves without the user moving it.
    pub(crate) fn place_keyed(
        &mut self,
        list: WheelList,
        key: usize,
        (scroll, selected): (usize, usize),
        (visible, total): (usize, usize),
    ) -> usize {
        if self.held(list, key) {
            return scroll.min(total.saturating_sub(visible.max(1)));
        }
        follow_selection(scroll, selected, visible, total)
    }

    /// Whether `list` stays where the wheel left it: it was scrolled, and
    /// its highlight is still at `key`, where it was when first drawn so.
    /// A list whose highlight moved follows it again. For a list that
    /// keeps its highlight in view its own way rather than with
    /// [`follow_selection`], as the model selector does.
    pub(crate) fn held(&mut self, list: WheelList, key: usize) -> bool {
        let at = self.detached.iter().position(|d| d.list == list);
        let kept = at.is_some_and(|at| {
            let detached = &mut self.detached[at];
            *detached.highlight.get_or_insert(key) == key
        });
        if let Some(at) = at.filter(|_| !kept) {
            self.detached.remove(at);
        }
        kept
    }

    /// Whether [`Self::held`] would keep `list` where the wheel left it,
    /// without recording anything: for a frame that draws the list before
    /// it can tell.
    pub(crate) fn holds(&self, list: WheelList, key: usize) -> bool {
        self.detached
            .iter()
            .any(|d| d.list == list && d.highlight.is_none_or(|at| at == key))
    }

    /// `list` follows its highlight again, as a list shown afresh does.
    pub(crate) fn release(&mut self, list: WheelList) {
        self.detached.retain(|d| d.list != list);
    }
}

/// `scroll` moved `delta` rows, not above the first; the next frame keeps
/// it above the last.
pub(crate) const fn wheeled(scroll: usize, delta: isize) -> usize {
    scroll.saturating_add_signed(delta)
}

impl App {
    /// Scroll `list`'s rows `delta` rows, leaving its highlight alone:
    /// `scroll` is its first row shown, which this moves.
    pub(super) fn wheel_list(&mut self, list: WheelList, scroll: usize, delta: isize) -> usize {
        self.panels.wheel.detach(list);
        wheeled(scroll, delta)
    }

    /// The wheel over `panel` scrolls its rows `delta` rows, or the chat
    /// as it always has; no highlight moves.
    pub(super) fn wheel_panel_rows(&mut self, panel: PanelFocus, delta: isize) {
        let lines = delta.unsigned_abs();
        match panel {
            PanelFocus::Chat if delta < 0 => self.scroll_panel_up(panel, lines),
            PanelFocus::Chat => self.scroll_panel_down(panel, lines),
            PanelFocus::Swarm => self.wheel_agents_panel(delta),
            PanelFocus::Records => self.wheel_records(delta),
            PanelFocus::Sandbox => self.wheel_side_panel(delta),
        }
    }

    /// The wheel over the agents panel scrolls the app in front's list.
    fn wheel_agents_panel(&mut self, delta: isize) {
        match self.agents_tab {
            AgentsTab::Channels => {
                let scroll = self.channels.scroll;
                self.channels.scroll = self.wheel_list(WheelList::Channels, scroll, delta);
            }
            AgentsTab::Projects => self.scroll_project_rows(delta),
            AgentsTab::Roster => {
                let scroll = self.definitions.scroll;
                self.definitions.scroll = self.wheel_list(WheelList::Roster, scroll, delta);
            }
            AgentsTab::Bench => {
                let scroll = self.bench.list_scroll;
                self.bench.list_scroll = self.wheel_list(WheelList::Bench, scroll, delta);
            }
            AgentsTab::Archive => {
                let scroll = self.archived.scroll;
                self.archived.scroll = self.wheel_list(WheelList::Archive, scroll, delta);
            }
            // Its few rows always fit; the wheel moves no highlight.
            AgentsTab::Integrations => {}
        }
    }

    /// The wheel over the Record panel scrolls its open tab.
    fn wheel_records(&mut self, delta: isize) {
        match self.records.tab {
            RecordsTab::Kernel => {
                let scroll = self.records.scroll;
                self.records.scroll = self.wheel_list(WheelList::Kernel, scroll, delta);
            }
            RecordsTab::Members => {
                let scroll = self.members.scroll;
                self.members.scroll = self.wheel_list(WheelList::Members, scroll, delta);
            }
            RecordsTab::Policies => self.scroll_policies(delta),
            RecordsTab::Inbox => {
                let scroll = self.inbox.scroll;
                self.inbox.scroll = self.wheel_list(WheelList::Inbox, scroll, delta);
            }
        }
    }

    /// The wheel over the side panel scrolls its open tab. A member of a
    /// channel's MEMBERS takes two rows, so it scrolls a member a notch.
    fn wheel_side_panel(&mut self, delta: isize) {
        let (list, scroll) = match self.side.tab {
            SidePanelTab::Host => return self.scroll_host(delta),
            SidePanelTab::ChannelInfo => return self.scroll_channel_info(delta),
            SidePanelTab::ChannelQueue => return self.scroll_channel_queue(delta),
            // Two rows an item, like MEMBERS: an item a notch.
            SidePanelTab::ChannelHistory => return self.scroll_channel_history(delta.signum()),
            SidePanelTab::ChannelMembers => {
                let scroll = self.channels.member_scroll;
                let list = WheelList::ChannelMembers;
                self.channels.member_scroll = self.wheel_list(list, scroll, delta.signum());
                return;
            }
            SidePanelTab::Sandbox => (WheelList::Sandbox, &mut self.side.sandbox_scroll),
            SidePanelTab::Repo => (WheelList::Repo, &mut self.repo.scroll),
            SidePanelTab::Processes => (WheelList::Processes, &mut self.side.processes.scroll),
            SidePanelTab::Trace => (WheelList::Trace, &mut self.side.trace_scroll),
            SidePanelTab::Tasks => (WheelList::Tasks, &mut self.side.tasks_scroll),
        };
        *scroll = wheeled(*scroll, delta);
        self.panels.wheel.detach(list);
    }

    /// Every list follows its highlight again, as a key press asks.
    pub(super) fn follow_highlights(&mut self) {
        self.panels.wheel.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheeled_list_stays_put_until_its_highlight_moves() {
        let mut views = WheelViews::default();
        let list = WheelList::Trace;
        assert_eq!(views.place(list, (0, 2), (5, 20)), 0, "in view already");
        views.detach(list);
        // Scrolled past the highlight, which stays at row 2.
        assert_eq!(views.place(list, (9, 2), (5, 20)), 9);
        assert_eq!(
            views.place(list, (40, 2), (5, 20)),
            15,
            "kept above the end"
        );
        // The highlight moved: back to following it.
        assert_eq!(views.place(list, (15, 3), (5, 20)), 3);
        assert!(!views.is_detached(list));
    }

    #[test]
    fn a_short_list_does_not_scroll() {
        let mut views = WheelViews::default();
        views.detach(WheelList::Repo);
        assert_eq!(views.place(WheelList::Repo, (3, 0), (10, 4)), 0);
    }

    #[test]
    fn the_wheel_stops_at_the_first_row() {
        assert_eq!(wheeled(2, -5), 0);
        assert_eq!(wheeled(2, 3), 5);
    }
}
