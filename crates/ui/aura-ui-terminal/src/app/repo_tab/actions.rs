//! What the REPO tab's keys and clicks do: picking a filter, moving the
//! highlight, folding the changes and opening rows.

use super::rows::{opened, Opened, RowKey};
use super::{RepoFilter, RepoRow};
use crate::app::image_preview::open_with_system;
use crate::app::processes::{on_row_icon, SIDE_PAGE};
use crate::app::{contains, App, ChatTab, HoverTarget, NotificationType};
use crate::events::UiEvent;
use crate::repo::RepoRequest;
use crossterm::event::{KeyCode, KeyEvent};

/// What `o` may open: pages on github.com, nothing the system's opener
/// would read from disk or run.
const GITHUB_PAGES: &str = "https://github.com/";

impl App {
    /// Show `filter`'s rows, the highlight on the first.
    pub(in crate::app) fn set_repo_filter(&mut self, filter: RepoFilter) {
        self.repo.filter = filter;
        self.repo.selected = 0;
        self.repo.scroll = 0;
        self.relist_repo(None);
    }

    /// The next filter of the strip (`]`), or the one before (`[`),
    /// wrapping around.
    pub(in crate::app) fn step_repo_filter(&mut self, forward: bool) {
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

    pub(in crate::app) fn select_repo_row(&mut self, index: usize) {
        self.repo.selected = index.min(self.repo_rows().len().saturating_sub(1));
    }

    fn move_repo_selection(&mut self, delta: isize) {
        self.select_repo_row(self.repo.selected.saturating_add_signed(delta));
    }

    pub(in crate::app) fn scroll_repo(&mut self, delta: isize) {
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
    pub(in crate::app) fn toggle_repo_changes(&mut self) {
        if self.on_repo_changes() {
            self.fold_repo_changes(!self.repo.changes_folded);
        }
    }

    /// Keys of the REPO tab: the arrows move and fold, Enter opens the
    /// row. `[`, `]`, `o`, `r` and Space are side-list shortcuts.
    pub(in crate::app) fn handle_repo_key(&mut self, key: KeyEvent) {
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
    pub(in crate::app) fn open_selected_repo_row(&mut self) {
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
            Opened::Say(said) => {
                self.notifications.current = Some((said, NotificationType::Warning));
            }
            Opened::Nothing => {}
        }
    }

    /// The page `o` opens for the highlighted row: a PR's, on github.com.
    pub(super) fn selected_repo_page(&self) -> Option<String> {
        let Some(&RepoRow::Event(i)) = self.repo_rows().get(self.repo.selected) else {
            return None;
        };
        let url = self.repo_view()?.events.get(i)?.url.clone()?;
        url.starts_with(GITHUB_PAGES).then_some(url)
    }

    /// `o`: open the highlighted PR's page on GitHub.
    pub(in crate::app) fn open_selected_repo_page(&mut self) {
        let Some(url) = self.selected_repo_page() else {
            self.notifications.current = Some((
                "Only a PR has a page to open".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        if let Err(e) = open_with_system(&url) {
            self.notifications.current = Some((
                format!("Could not open {url}: {e}"),
                NotificationType::Error,
            ));
        }
    }

    /// Clicking a filter shows it, and `‹`/`›` step through them.
    /// Clicking a row highlights and opens it; the changes' header folds.
    pub(in crate::app) fn click_repo(&mut self, column: u16, row: u16) {
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
    pub(in crate::app) fn hovered_repo(&self, column: u16, row: u16) -> Option<HoverTarget> {
        self.repo_chip_at(column, row)
            .map(HoverTarget::RepoFilter)
            .or_else(|| self.repo_row_at(column, row).map(HoverTarget::RepoRow))
    }
}
