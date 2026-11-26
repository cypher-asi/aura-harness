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

use super::image_preview::open_with_system;
use super::processes::{on_row_icon, SIDE_PAGE};
use super::{
    contains, follow_selection, App, ChatTab, HoverTarget, NotificationType, PanelFocus,
    SidePanelTab,
};
use crate::events::UiEvent;
use crate::repo::{EventKind, PrState, RepoRequest, RepoTarget, RepoView};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the tab asks for the repo again while it is on screen.
const REFRESH_EVERY: Duration = Duration::from_secs(5);

/// How long an unanswered ask holds back the next one.
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// What the REPO tab's list shows, picked in the strip at its top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum RepoFilter {
    /// The sync state, the changes and every event, newest first
    #[default]
    All,
    /// The uncommitted changes alone
    Changes,
    Commits,
    Pushes,
    /// Pulls, and fetches that moved a remote-tracking branch
    Pulls,
    Prs,
    /// Merges and rebases
    Merges,
    Resets,
    /// Switches to another branch or commit
    Checkouts,
    Stashes,
    /// The repo's local branches
    Branches,
    Tags,
    Worktrees,
}

impl RepoFilter {
    /// Every filter, in the strip's order.
    pub const ALL: [Self; 13] = [
        Self::All,
        Self::Changes,
        Self::Commits,
        Self::Pushes,
        Self::Pulls,
        Self::Prs,
        Self::Merges,
        Self::Resets,
        Self::Checkouts,
        Self::Stashes,
        Self::Branches,
        Self::Tags,
        Self::Worktrees,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Changes => "Changes",
            Self::Commits => "Commits",
            Self::Pushes => "Pushes",
            Self::Pulls => "Pulls",
            Self::Prs => "PRs",
            Self::Merges => "Merges",
            Self::Resets => "Resets",
            Self::Checkouts => "Checkouts",
            Self::Stashes => "Stashes",
            Self::Branches => "Branches",
            Self::Tags => "Tags",
            Self::Worktrees => "Worktrees",
        }
    }

    /// Whether it lists the repo's branches, tags or worktrees rather than
    /// a timeline; the strip sets these apart behind a bar.
    #[must_use]
    pub const fn is_list(self) -> bool {
        matches!(self, Self::Branches | Self::Tags | Self::Worktrees)
    }

    /// Whether its timeline shows events of `kind`.
    #[must_use]
    pub const fn shows(self, kind: EventKind) -> bool {
        match self {
            Self::All => true,
            Self::Commits => matches!(kind, EventKind::Commit),
            Self::Pushes => matches!(kind, EventKind::Push),
            Self::Pulls => matches!(kind, EventKind::Pull | EventKind::Fetch),
            Self::Prs => matches!(kind, EventKind::Pr),
            Self::Merges => matches!(kind, EventKind::Merge | EventKind::Rebase),
            Self::Resets => matches!(kind, EventKind::Reset),
            Self::Checkouts => matches!(kind, EventKind::Checkout),
            Self::Stashes => matches!(kind, EventKind::Stash),
            Self::Changes | Self::Branches | Self::Tags | Self::Worktrees => false,
        }
    }

    /// Whether `view` has anything to show under it, which puts it in the
    /// strip. PRs are there whenever the repo is on GitHub, to say why
    /// none are listed when they can't be.
    #[must_use]
    pub fn has_any(self, view: &RepoView) -> bool {
        match self {
            Self::All => true,
            Self::Changes => !view.changes.is_empty(),
            Self::Prs => view.prs != PrState::NotGitHub,
            Self::Branches => !view.branches.is_empty(),
            Self::Tags => !view.tags.is_empty(),
            Self::Worktrees => view.worktrees.len() > 1,
            timeline => view.events.iter().any(|event| timeline.shows(event.kind)),
        }
    }

    /// The filter `/repo <word>` names: its label, singular or plural, in
    /// any case.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        let word = word.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|filter| {
                let label = filter.label().to_ascii_lowercase();
                let one = |suffix: &str| label.strip_suffix(suffix).is_some_and(|one| one == word);
                label == word || one("es") || one("s")
            })
            .or(match word.as_str() {
                "fetch" | "fetches" => Some(Self::Pulls),
                "rebase" | "rebases" => Some(Self::Merges),
                "switch" | "switches" => Some(Self::Checkouts),
                _ => None,
            })
    }
}

/// A row of the REPO tab's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoRow {
    /// The branch, its upstream and how far apart they are
    Summary,
    /// The header the uncommitted changes fold under
    Changes,
    /// An uncommitted change, by its index in the view's
    Change(usize),
    /// How many changes are past those listed
    MoreChanges,
    /// An event of the timeline, by its index in the view's
    Event(usize),
    Branch(usize),
    Tag(usize),
    Worktree(usize),
    /// Why no PRs are listed
    PrNote,
}

/// What a row is, kept across refreshes to keep the highlight on it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Summary,
    Changes,
    Change(String),
    MoreChanges,
    Event(EventKind, RepoTarget),
    Branch(String),
    Tag(String),
    Worktree(PathBuf),
    PrNote,
}

fn key_of(view: &RepoView, row: RepoRow) -> Option<RowKey> {
    Some(match row {
        RepoRow::Summary => RowKey::Summary,
        RepoRow::Changes => RowKey::Changes,
        RepoRow::Change(i) => RowKey::Change(view.changes.get(i)?.path.clone()),
        RepoRow::MoreChanges => RowKey::MoreChanges,
        RepoRow::Event(i) => {
            let event = view.events.get(i)?;
            RowKey::Event(event.kind, event.target.clone())
        }
        RepoRow::Branch(i) => RowKey::Branch(view.branches.get(i)?.name.clone()),
        RepoRow::Tag(i) => RowKey::Tag(view.tags.get(i)?.name.clone()),
        RepoRow::Worktree(i) => RowKey::Worktree(view.worktrees.get(i)?.path.clone()),
        RepoRow::PrNote => RowKey::PrNote,
    })
}

/// The rows `view` lists under `filter`, the changes folded or not.
fn rows_of(view: &RepoView, filter: RepoFilter, changes_folded: bool) -> Vec<RepoRow> {
    let mut rows = Vec::new();
    match filter {
        RepoFilter::Branches => rows.extend((0..view.branches.len()).map(RepoRow::Branch)),
        RepoFilter::Tags => rows.extend((0..view.tags.len()).map(RepoRow::Tag)),
        RepoFilter::Worktrees => rows.extend((0..view.worktrees.len()).map(RepoRow::Worktree)),
        _ => {
            rows.push(RepoRow::Summary);
            let changes = !view.changes.is_empty();
            if filter == RepoFilter::All && changes {
                rows.push(RepoRow::Changes);
            }
            let open = filter == RepoFilter::Changes || !changes_folded;
            if matches!(filter, RepoFilter::All | RepoFilter::Changes) && changes && open {
                rows.extend((0..view.changes.len()).map(RepoRow::Change));
                if view.changes_more > 0 {
                    rows.push(RepoRow::MoreChanges);
                }
            }
            if filter == RepoFilter::Prs && matches!(view.prs, PrState::Note(_)) {
                rows.push(RepoRow::PrNote);
            }
            let since = view.span_start.filter(|_| filter == RepoFilter::All);
            rows.extend(
                view.events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| filter.shows(event.kind))
                    .filter(|(_, event)| since.is_none_or(|start| event.at >= start))
                    .map(|(i, _)| RepoRow::Event(i)),
            );
        }
    }
    rows
}

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
    scroll: usize,
    /// The agent last asked for, and when
    asked: Option<(String, Instant)>,
    /// Whether that ask is unanswered
    waiting: bool,
    /// Where each filter of the strip was drawn last frame
    chips: Vec<(Rect, RepoFilter)>,
    /// Where the strip's `‹` and `›` were drawn, when it was cut there
    steps: (Option<Rect>, Option<Rect>),
}

/// What opening a row does.
enum Opened {
    /// Post this diff in the chat, under this title
    Diff(RepoTarget, String),
    /// Fold the changes or open them
    Fold,
    /// Say why there is nothing to post
    Say(String),
    Nothing,
}

/// `3 commits`, `1 commit`.
fn commits(n: usize) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

fn opened(view: &RepoView, row: RepoRow) -> Opened {
    match row {
        RepoRow::Summary => {
            let Some(branch) = &view.branch else {
                return Opened::Say(format!("HEAD is detached at {}", view.head));
            };
            let Some(upstream) = &view.upstream else {
                return Opened::Say(format!("{branch} has no upstream to compare with"));
            };
            if view.ahead == 0 {
                return Opened::Say(format!("Nothing to push: {upstream} has all of {branch}"));
            }
            let title = format!("{branch}: {} not on {upstream}", commits(view.ahead));
            Opened::Diff(RepoTarget::Unpushed, title)
        }
        RepoRow::Changes => Opened::Fold,
        RepoRow::Change(i) => view.changes.get(i).map_or(Opened::Nothing, |change| {
            let target = RepoTarget::Change {
                path: change.path.clone(),
                from: change.from.clone(),
            };
            Opened::Diff(target, format!("Uncommitted: {}", change.path))
        }),
        RepoRow::Event(i) => view.events.get(i).map_or(Opened::Nothing, |event| {
            let title = match &event.target {
                RepoTarget::Commit { sha } => {
                    format!("{} {}", sha.get(..8).unwrap_or(sha), event.title)
                }
                _ => event.title.clone(),
            };
            Opened::Diff(event.target.clone(), title)
        }),
        RepoRow::Branch(i) => view.branches.get(i).map_or(Opened::Nothing, |branch| {
            if branch.current {
                return Opened::Say(format!("{} is checked out here", branch.name));
            }
            let target = RepoTarget::Branch {
                name: branch.name.clone(),
            };
            Opened::Diff(target, format!("What {} has that HEAD lacks", branch.name))
        }),
        RepoRow::Tag(i) => view.tags.get(i).map_or(Opened::Nothing, |tag| {
            let target = RepoTarget::Tag {
                name: tag.name.clone(),
            };
            Opened::Diff(target, format!("Since {}", tag.name))
        }),
        RepoRow::Worktree(i) => view.worktrees.get(i).map_or(Opened::Nothing, |worktree| {
            let target = RepoTarget::Worktree {
                path: worktree.path.clone(),
            };
            Opened::Diff(
                target,
                format!("Uncommitted in {}", worktree.path.display()),
            )
        }),
        RepoRow::PrNote => match &view.prs {
            PrState::Note(note) => Opened::Say(note.clone()),
            _ => Opened::Nothing,
        },
        RepoRow::MoreChanges => Opened::Nothing,
    }
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
        self.repo.scroll = follow_selection(
            self.repo.scroll,
            self.repo.selected,
            visible,
            self.repo_rows().len(),
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
        self.side_tab == SidePanelTab::Repo
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
                self.notification = Some((
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
        self.sandbox_panel_visible = true;
        self.chat_expanded = false;
        self.show_side_tab(SidePanelTab::Repo);
        self.focus = PanelFocus::Sandbox;
        if self.repo_on_screen() {
            self.request_repo(false);
        }
    }

    /// Show `filter`'s rows, the highlight on the first.
    pub(super) fn set_repo_filter(&mut self, filter: RepoFilter) {
        self.repo.filter = filter;
        self.repo.selected = 0;
        self.repo.scroll = 0;
        self.relist_repo(None);
    }

    /// The next filter of the strip (`]`), or the one before (`[`),
    /// wrapping around.
    pub(super) fn step_repo_filter(&mut self, forward: bool) {
        let filters = self.repo_filters();
        if filters.is_empty() {
            return;
        }
        let at = filters
            .iter()
            .position(|&filter| filter == self.repo.filter)
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % filters.len()
        } else {
            (at + filters.len() - 1) % filters.len()
        };
        self.set_repo_filter(filters[next]);
    }

    pub(super) fn select_repo_row(&mut self, index: usize) {
        self.repo.selected = index.min(self.repo_rows().len().saturating_sub(1));
    }

    fn move_repo_selection(&mut self, delta: isize) {
        self.select_repo_row(self.repo.selected.saturating_add_signed(delta));
    }

    pub(super) fn scroll_repo(&mut self, delta: isize) {
        self.move_repo_selection(delta);
    }

    /// Whether the highlight is on the changes' header or one of them.
    fn on_repo_changes(&self) -> bool {
        matches!(
            self.repo_rows().get(self.repo.selected),
            Some(RepoRow::Changes | RepoRow::Change(_) | RepoRow::MoreChanges)
        )
    }

    /// Fold the changes under their header, the highlight going to it,
    /// or open them.
    fn fold_repo_changes(&mut self, folded: bool) {
        if self.repo.filter != RepoFilter::All || folded == self.repo.changes_folded {
            return;
        }
        self.repo.changes_folded = folded;
        self.relist_repo(Some(RowKey::Changes));
    }

    /// Space: fold or open the changes, from their header or one of them.
    pub(super) fn toggle_repo_changes(&mut self) {
        if self.on_repo_changes() {
            self.fold_repo_changes(!self.repo.changes_folded);
        }
    }

    /// Keys of the REPO tab: the arrows move and fold, Enter opens the
    /// row. `[`, `]`, `o`, `r` and Space are side-list shortcuts.
    pub(super) fn handle_repo_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_repo_selection(-1),
            KeyCode::Down => self.move_repo_selection(1),
            KeyCode::PageUp => self.move_repo_selection(-SIDE_PAGE),
            KeyCode::PageDown => self.move_repo_selection(SIDE_PAGE),
            KeyCode::Home => self.select_repo_row(0),
            KeyCode::End => self.select_repo_row(usize::MAX),
            KeyCode::Enter => self.open_selected_repo_row(),
            KeyCode::Left if self.on_repo_changes() => self.fold_repo_changes(true),
            KeyCode::Right
                if self.repo_rows().get(self.repo.selected) == Some(&RepoRow::Changes) =>
            {
                self.fold_repo_changes(false);
            }
            _ => {}
        }
    }

    /// Open the highlighted row: post its diff in the chat, which comes
    /// forward, scrolled to the end where the diff lands.
    pub(super) fn open_selected_repo_row(&mut self) {
        let Some(view) = self.repo_view() else {
            return;
        };
        let Some(&row) = self.repo.rows.get(self.repo.selected) else {
            return;
        };
        match opened(view, row) {
            Opened::Diff(target, title) => {
                let request = RepoRequest::Open { target, title };
                let agent_id = self.active_agent_id.clone();
                if self.try_send_event(UiEvent::Repo { agent_id, request }) {
                    self.close_detail();
                    if self.chat_tab() != ChatTab::Chat {
                        self.show_chat_tab(ChatTab::Chat);
                    }
                    self.scroll_chat_to_bottom();
                }
            }
            Opened::Fold => self.fold_repo_changes(!self.repo.changes_folded),
            Opened::Say(said) => self.notification = Some((said, NotificationType::Warning)),
            Opened::Nothing => {}
        }
    }

    /// `o`: open the highlighted PR's page on GitHub.
    pub(super) fn open_selected_repo_page(&mut self) {
        let url = match (self.repo_view(), self.repo_rows().get(self.repo.selected)) {
            (Some(view), Some(&RepoRow::Event(i))) => {
                view.events.get(i).and_then(|e| e.url.clone())
            }
            _ => None,
        };
        let Some(url) = url else {
            self.notification = Some((
                "Only a PR has a page to open".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        if let Err(e) = open_with_system(&url) {
            self.notification = Some((
                format!("Could not open {url}: {e}"),
                NotificationType::Error,
            ));
        }
    }

    /// Clicking a filter shows it, and `‹`/`›` step through them.
    /// Clicking a row highlights and opens it; the changes' header folds.
    pub(super) fn click_repo(&mut self, column: u16, row: u16) {
        if let Some(filter) = self.repo_chip_at(column, row) {
            self.set_repo_filter(filter);
            return;
        }
        let (back, on) = self.repo.steps;
        if contains(back, column, row) || contains(on, column, row) {
            self.step_repo_filter(contains(on, column, row));
            return;
        }
        let Some(index) = self.repo_row_at(column, row) else {
            return;
        };
        self.select_repo_row(index);
        let on_fold = self.repo_rows().get(index) == Some(&RepoRow::Changes)
            && self
                .areas
                .repo_rows
                .is_some_and(|rows| on_row_icon(column, rows.x, 0));
        if on_fold {
            self.fold_repo_changes(!self.repo.changes_folded);
        } else {
            self.open_selected_repo_row();
        }
    }

    fn repo_chip_at(&self, column: u16, row: u16) -> Option<RepoFilter> {
        self.repo
            .chips
            .iter()
            .find(|(area, _)| contains(Some(*area), column, row))
            .map(|&(_, filter)| filter)
    }

    fn repo_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .repo_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.repo_scroll + usize::from(row - rows.y);
        (index < self.repo_rows().len()).then_some(index)
    }

    /// The filter or row of the REPO tab at a screen cell.
    pub(super) fn hovered_repo(&self, column: u16, row: u16) -> Option<HoverTarget> {
        self.repo_chip_at(column, row)
            .map(HoverTarget::RepoFilter)
            .or_else(|| self.repo_row_at(column, row).map(HoverTarget::RepoRow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;
    use crate::repo::{ChangeKind, EventState, RepoBranch, RepoChange, RepoEvent, RepoWorktree};
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn event(kind: EventKind, at: i64, title: &str, target: RepoTarget) -> RepoEvent {
        RepoEvent {
            kind,
            state: EventState::Done,
            at,
            title: title.into(),
            detail: String::new(),
            target,
            url: None,
        }
    }

    fn commit(at: i64, title: &str, sha: &str) -> RepoEvent {
        event(
            EventKind::Commit,
            at,
            title,
            RepoTarget::Commit { sha: sha.into() },
        )
    }

    fn view(agent: &str) -> RepoView {
        let range = RepoTarget::Range {
            from: Some("aaaa1111".into()),
            to: "bbbb2222".into(),
        };
        RepoView {
            agent_id: agent.into(),
            root: PathBuf::from("/src/aura"),
            branch: Some("main".into()),
            head: "bbbb2222".into(),
            upstream: Some("origin/main".into()),
            ahead: 1,
            worktree: false,
            changes: vec![RepoChange {
                path: "src/lib.rs".into(),
                from: None,
                kind: ChangeKind::Modified,
                staged: false,
                unstaged: true,
            }],
            events: vec![
                commit(50, "Add REPO", "bbbb2222"),
                event(
                    EventKind::Push,
                    40,
                    "Pushed 2 commits to origin/main",
                    range,
                ),
                commit(30, "Start", "aaaa1111"),
            ],
            branches: vec![RepoBranch {
                name: "main".into(),
                current: true,
                upstream: Some("origin/main".into()),
                ahead: 1,
                behind: 0,
                gone: false,
                at: 50,
                subject: "Add REPO".into(),
            }],
            worktrees: vec![RepoWorktree {
                path: "/src/aura".into(),
                branch: Some("main".into()),
                head: "bbbb2222".into(),
                current: true,
                locked: false,
                prunable: false,
            }],
            ..RepoView::default()
        }
    }

    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "aa".into();
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Repo);
        (app, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn opened_target(rx: &mut mpsc::Receiver<UiEvent>) -> Option<(RepoTarget, String)> {
        while let Ok(event) = rx.try_recv() {
            if let UiEvent::Repo {
                request: RepoRequest::Open { target, title },
                ..
            } = event
            {
                return Some((target, title));
            }
        }
        None
    }

    #[test]
    fn filters_name_what_they_show() {
        assert_eq!(RepoFilter::named("PRs"), Some(RepoFilter::Prs));
        assert_eq!(RepoFilter::named("push"), Some(RepoFilter::Pushes));
        assert_eq!(RepoFilter::named("branch"), Some(RepoFilter::Branches));
        assert_eq!(RepoFilter::named("changes"), Some(RepoFilter::Changes));
        assert_eq!(RepoFilter::named("fetch"), Some(RepoFilter::Pulls));
        assert_eq!(RepoFilter::named("nope"), None);

        let view = view("aa");
        let shown: Vec<RepoFilter> = RepoFilter::ALL
            .into_iter()
            .filter(|filter| filter.has_any(&view))
            .collect();
        assert_eq!(
            shown,
            [
                RepoFilter::All,
                RepoFilter::Changes,
                RepoFilter::Commits,
                RepoFilter::Pushes,
                RepoFilter::Branches
            ],
            "one worktree is no list, and a repo off GitHub has no PRs"
        );
    }

    #[test]
    fn all_lists_the_summary_changes_and_timeline_and_a_filter_narrows_it() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
        assert_eq!(
            app.repo_rows(),
            [
                RepoRow::Summary,
                RepoRow::Changes,
                RepoRow::Change(0),
                RepoRow::Event(0),
                RepoRow::Event(1),
                RepoRow::Event(2),
            ]
        );
        app.handle_key(key(KeyCode::Char(']')));
        assert_eq!(app.repo_filter(), RepoFilter::Changes);
        assert_eq!(app.repo_rows(), [RepoRow::Summary, RepoRow::Change(0)]);
        app.handle_key(key(KeyCode::Char(']')));
        assert_eq!(app.repo_filter(), RepoFilter::Commits);
        assert_eq!(
            app.repo_rows(),
            [RepoRow::Summary, RepoRow::Event(0), RepoRow::Event(2)]
        );
        app.handle_key(key(KeyCode::Char('[')));
        app.handle_key(key(KeyCode::Char('[')));
        app.handle_key(key(KeyCode::Char('[')));
        assert_eq!(app.repo_filter(), RepoFilter::Branches, "it wraps around");
        assert_eq!(app.repo_rows(), [RepoRow::Branch(0)]);
    }

    #[test]
    fn the_changes_fold_and_the_highlight_keeps_its_row() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Change(0));
        app.handle_key(key(KeyCode::Left));
        assert!(app.repo_changes_folded());
        assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Changes);
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(!app.repo_changes_folded());

        // A refresh with a new commit on top keeps the highlight on its push.
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        let mut newer = view("aa");
        newer.events.insert(0, commit(60, "Newer", "cccc3333"));
        app.process_command(UiCommand::SetRepo(Box::new(newer)));
        assert_eq!(app.repo_rows()[app.repo_selected()], RepoRow::Event(2));

        // One whose kind is gone gives way to All.
        app.set_repo_filter(RepoFilter::Pushes);
        let mut pushed_none = view("aa");
        pushed_none.events.retain(|e| e.kind != EventKind::Push);
        app.process_command(UiCommand::SetRepo(Box::new(pushed_none)));
        assert_eq!(app.repo_filter(), RepoFilter::All);
    }

    #[test]
    fn enter_posts_the_rows_diff_and_brings_the_chat_forward() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            opened_target(&mut rx),
            Some((
                RepoTarget::Unpushed,
                "main: 1 commit not on origin/main".into()
            ))
        );
        app.select_repo_row(3);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            opened_target(&mut rx),
            Some((
                RepoTarget::Commit {
                    sha: "bbbb2222".into()
                },
                "bbbb2222 Add REPO".into()
            ))
        );
        assert_eq!(app.chat_tab(), ChatTab::Chat);

        app.set_repo_filter(RepoFilter::Branches);
        app.handle_key(key(KeyCode::Enter));
        assert!(
            opened_target(&mut rx).is_none(),
            "the branch checked out has no diff of its own"
        );
        assert!(app.notification().is_some());
    }

    #[test]
    fn another_agents_view_is_not_shown_or_opened() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetRepo(Box::new(view("bb"))));
        assert!(app.repo_view().is_none());
        assert!(app.repo_rows().is_empty());
        app.handle_key(key(KeyCode::Enter));
        assert!(opened_target(&mut rx).is_none());
    }

    #[test]
    fn the_tab_asks_at_once_then_waits_for_its_answer() {
        let (mut app, mut rx) = app();
        app.sandbox_panel_visible = true;
        app.refresh_repo_tab();
        let asked = |rx: &mut mpsc::Receiver<UiEvent>| {
            let mut n = 0;
            while let Ok(event) = rx.try_recv() {
                if matches!(
                    event,
                    UiEvent::Repo {
                        request: RepoRequest::Load { .. },
                        ..
                    }
                ) {
                    n += 1;
                }
            }
            n
        };
        assert_eq!(asked(&mut rx), 1);
        app.refresh_repo_tab();
        assert_eq!(asked(&mut rx), 0, "the ask is out");
        app.process_command(UiCommand::SetRepo(Box::new(view("aa"))));
        app.refresh_repo_tab();
        assert_eq!(asked(&mut rx), 0, "answered just now");
        let then = Instant::now().checked_sub(REFRESH_EVERY).unwrap();
        app.repo.asked = Some(("aa".into(), then));
        app.refresh_repo_tab();
        assert_eq!(asked(&mut rx), 1);
        app.handle_key(key(KeyCode::Char('r')));
        assert_eq!(asked(&mut rx), 1, "r asks at once");
    }
}
