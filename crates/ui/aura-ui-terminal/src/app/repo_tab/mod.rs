//! The side panel's REPO tab: the git repo the agent on screen works in.
//!
//! A strip of filters on top picks what the list below it shows: all
//! that happened in the repo, newest first, or one kind of it (commits,
//! pushes, pulls, PRs, merges, resets, checkouts, stashes); or, past a
//! bar, the repo's branches, tags or worktrees. Above the timeline sit
//! the branch's sync with its upstream and the uncommitted changes.
//! Opening a row posts its diff in the chat.
//!
//! Invariants:
//! - The tab only shows the view of the agent on screen; one sent for
//!   another agent is kept but not drawn, and its rows can't be opened.
//! - While the tab is on screen it asks for the view at once, then every
//!   [`REFRESH_EVERY`], and asks again only once the last ask was
//!   answered or [`ANSWER_WAIT`] went by.
//! - The highlight stays on the same item across refreshes; a new filter
//!   starts at the top. A filter whose kind a refresh empties gives way
//!   to All.

mod actions;
mod filter;
mod rows;
#[cfg(test)]
mod tests;

pub use filter::RepoFilter;
pub use rows::RepoRow;

use super::{App, NotificationType, PanelFocus, SidePanelTab};
use crate::app::wheel_views::WheelList;
use crate::events::UiEvent;
use crate::repo::{RepoRequest, RepoView};
use ratatui::layout::Rect;
use rows::{key_of, rows_of, RowKey};
use std::time::{Duration, Instant};

/// How often the tab asks for the repo again while it is on screen.
const REFRESH_EVERY: Duration = Duration::from_secs(5);

/// How long an unanswered ask holds back the next one.
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The REPO tab's state.
#[derive(Debug, Default)]
pub(crate) struct RepoTab {
    /// The last view, of whichever agent it was sent for
    view: Option<RepoView>,
    filter: RepoFilter,
    /// Whether the changes are folded under their header
    changes_folded: bool,
    rows: Vec<RepoRow>,
    selected: usize,
    /// First visible row
    pub(in crate::app) scroll: usize,
    /// The agent last asked for, and when
    asked: Option<(String, Instant)>,
    /// Whether that ask is unanswered
    waiting: bool,
    /// Where each filter of the strip was drawn last frame
    chips: Vec<(Rect, RepoFilter)>,
    /// Where the strip's `‹` and `›` were drawn, when it was cut there
    steps: (Option<Rect>, Option<Rect>),
}

impl App {
    /// The REPO tab's view, when it is of the agent on screen.
    #[must_use]
    pub fn repo_view(&self) -> Option<&RepoView> {
        self.repo
            .view
            .as_ref()
            .filter(|view| !view.agent_id.is_empty() && view.agent_id == self.active_agent_id)
    }

    /// What the REPO tab's list shows.
    #[must_use]
    pub const fn repo_filter(&self) -> RepoFilter {
        self.repo.filter
    }

    /// The filters the REPO tab's strip shows: those with something to
    /// show.
    #[must_use]
    pub fn repo_filters(&self) -> Vec<RepoFilter> {
        self.repo_view().map_or_else(Vec::new, |view| {
            RepoFilter::ALL
                .into_iter()
                .filter(|filter| filter.has_any(view))
                .collect()
        })
    }

    /// The REPO tab's rows, empty while it has no view of the agent on
    /// screen.
    #[must_use]
    pub fn repo_rows(&self) -> &[RepoRow] {
        if self.repo_view().is_some() {
            &self.repo.rows
        } else {
            &[]
        }
    }

    /// The highlighted row of the REPO tab.
    #[must_use]
    pub const fn repo_selected(&self) -> usize {
        self.repo.selected
    }

    /// Whether the REPO tab's changes are folded under their header.
    #[must_use]
    pub const fn repo_changes_folded(&self) -> bool {
        self.repo.changes_folded
    }

    /// Keep the highlighted row inside `visible` rows, returning the first
    /// row to draw.
    pub fn sync_repo_scroll(&mut self, visible: usize) -> usize {
        let total = self.repo_rows().len();
        self.repo.scroll = self.panels.wheel.place(
            WheelList::Repo,
            (self.repo.scroll, self.repo.selected),
            (visible, total),
        );
        self.repo.scroll
    }

    /// Where the REPO rows were drawn, for mouse clicks.
    pub fn set_repo_rows_area(&mut self, area: Option<Rect>, scroll: usize) {
        self.areas.repo_rows = area;
        self.areas.repo_scroll = scroll;
    }

    /// Where the REPO strip's filters were drawn, and its `‹` and `›`.
    pub fn set_repo_strip(
        &mut self,
        chips: Vec<(Rect, RepoFilter)>,
        steps: (Option<Rect>, Option<Rect>),
    ) {
        self.repo.chips = chips;
        self.repo.steps = steps;
    }

    /// A REPO view came: keep it, and when it is of the agent on screen,
    /// list it.
    pub(super) fn cmd_set_repo(&mut self, view: RepoView) {
        if self
            .repo
            .asked
            .as_ref()
            .is_some_and(|(agent, _)| *agent == view.agent_id)
        {
            self.repo.waiting = false;
        }
        if view.agent_id != self.active_agent_id {
            return;
        }
        let kept = self.selected_repo_key();
        if !self.repo.filter.has_any(&view) {
            self.repo.filter = RepoFilter::All;
        }
        self.repo.view = Some(view);
        self.relist_repo(kept);
    }

    fn selected_repo_key(&self) -> Option<RowKey> {
        let view = self.repo_view()?;
        key_of(view, *self.repo.rows.get(self.repo.selected)?)
    }

    /// List the rows again, the highlight on `kept` when it is still
    /// listed.
    fn relist_repo(&mut self, kept: Option<RowKey>) {
        let (rows, found) = match self.repo_view() {
            Some(view) => {
                let rows = rows_of(view, self.repo.filter, self.repo.changes_folded);
                let found = kept.and_then(|key| {
                    rows.iter()
                        .position(|&row| key_of(view, row).as_ref() == Some(&key))
                });
                (rows, found)
            }
            None => (Vec::new(), None),
        };
        self.repo.rows = rows;
        self.repo.selected = found
            .unwrap_or(self.repo.selected)
            .min(self.repo.rows.len().saturating_sub(1));
    }

    /// Whether the REPO tab is on screen.
    fn repo_on_screen(&self) -> bool {
        self.side.tab == SidePanelTab::Repo
            && self.sandbox_panel_visible()
            && !self.channel_open()
            && !self.active_agent_id.is_empty()
    }

    /// Ask for the repo while the tab is on screen, as often as the
    /// module's invariants say.
    pub(super) fn refresh_repo_tab(&mut self) {
        if !self.repo_on_screen() {
            return;
        }
        let due = match &self.repo.asked {
            None => true,
            Some((agent, _)) if *agent != self.active_agent_id => true,
            Some((_, at)) if self.repo.waiting => at.elapsed() >= ANSWER_WAIT,
            Some((_, at)) => self.repo_view().is_none() || at.elapsed() >= REFRESH_EVERY,
        };
        if due {
            self.request_repo(false);
        }
    }

    /// Ask for the repo of the agent on screen; `force` reads its PRs
    /// again too.
    pub(super) fn request_repo(&mut self, force: bool) {
        if self.active_agent_id.is_empty() {
            return;
        }
        let agent_id = self.active_agent_id.clone();
        let request = RepoRequest::Load { force };
        if self.try_send_event(UiEvent::Repo {
            agent_id: agent_id.clone(),
            request,
        }) {
            self.repo.asked = Some((agent_id, Instant::now()));
            self.repo.waiting = true;
        }
    }

    /// Show the REPO tab (`/repo [filter]`), on `arg`'s filter when given.
    pub(super) fn show_repo(&mut self, arg: &str) {
        if self.agent_panels_hidden() {
            self.notify_no_agent_panels();
            return;
        }
        let arg = arg.trim();
        if !arg.is_empty() {
            let Some(filter) = RepoFilter::named(arg) else {
                self.notifications.current = Some((
                    format!(
                        "No REPO filter `{arg}`: use changes, commits, pushes, pulls, prs, \
                         merges, resets, checkouts, stashes, branches, tags or worktrees"
                    ),
                    NotificationType::Warning,
                ));
                return;
            };
            self.set_repo_filter(filter);
        }
        self.panels.sandbox_visible = true;
        self.panels.chat_expanded = false;
        self.show_side_tab(SidePanelTab::Repo);
        self.focus = PanelFocus::Sandbox;
        if self.repo_on_screen() {
            self.request_repo(false);
        }
    }
}
