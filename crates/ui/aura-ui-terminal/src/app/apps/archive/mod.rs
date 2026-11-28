//! The agents panel's Archive tab, and moving agents and projects out of
//! the projects list.
//!
//! The Archive lists archived agents newest-archived first, under a header
//! for the day each was archived. Clicking an archived agent, or Enter on
//! it, opens its chat and stays on the Archive; only the row's right-click
//! "Unarchive" or `/unarchive` brings it back to the projects list.
//!
//! The highlight (`archived.selected`) is an index into the archived
//! agents, never a header; the scroll (`archived.scroll`) counts drawn
//! rows, headers included.
//!
//! An archived agent with agents under it starts folded over them each
//! time the Archive is entered, so the tab lists one row per archived
//! agent. Space on it, or a click on its ▸ marker, opens it; the
//! highlight only ever rests on a row that is drawn, so folding an agent
//! moves a highlight inside it onto the agent.
//!
//! Archiving is reversible, so it asks no confirmation: Delete on an agent
//! in the projects list, or `/archive`. Removing a project drops it and
//! its agents from the list for good (their records and folder stay on
//! disk), so it waits for `y`: Shift+Delete, or `/remove-project`.

use crate::app::wheel_views::WheelList;
use crate::app::{contains, App, NotificationType, ProjectListRow};
use crate::events::{AgentSummary, UiEvent};
use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// A row of the Archive as drawn: a day header or an archived agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveRow {
    /// The day the agents below it were archived: `Today`, `Yesterday`,
    /// a date such as `Wednesday, September 16 2026`, or `Earlier` when
    /// the time is unknown.
    Day(String),
    /// The archived agent at `index` in [`App::archived`].
    Agent {
        /// Index into [`App::archived`].
        index: usize,
    },
}

/// Header of the day an agent archived at `ms` (Unix milliseconds) falls
/// on, as seen at `now`: `Today`, `Yesterday`, else its local date in
/// full, as `Wednesday, September 16 2026`. An unknown time reads
/// `Earlier`.
pub(crate) fn archive_day(ms: Option<u64>, now: DateTime<Local>) -> String {
    let Some(at) = ms
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(DateTime::from_timestamp_millis)
    else {
        return "Earlier".to_string();
    };
    let day = at.with_timezone(&Local).date_naive();
    let today = now.date_naive();
    if day == today {
        "Today".to_string()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".to_string()
    } else {
        day.format("%A, %B %-d %Y").to_string()
    }
}

impl App {
    /// Archived agents, each followed by the agents under it.
    #[must_use]
    pub fn archived(&self) -> &[AgentSummary] {
        &self.archived.items
    }

    /// The Archive as drawn: each archived agent, after a header for its
    /// day whenever that day differs from the previous head's. Agents
    /// under a head stay with it, and are left out while an agent above
    /// them is folded.
    #[must_use]
    pub fn archive_rows(&self) -> Vec<ArchiveRow> {
        self.archive_rows_at(Local::now())
    }

    /// [`Self::archive_rows`] with the day headers read at `now`.
    fn archive_rows_at(&self, now: DateTime<Local>) -> Vec<ArchiveRow> {
        let mut rows = Vec::with_capacity(self.archived.len() + 1);
        let mut last_day: Option<String> = None;
        for index in self.shown_archived() {
            let agent = &self.archived.items[index];
            if agent.depth == 0 {
                let day = archive_day(agent.archived_at_ms, now);
                if last_day.as_ref() != Some(&day) {
                    rows.push(ArchiveRow::Day(day.clone()));
                    last_day = Some(day);
                }
            }
            rows.push(ArchiveRow::Agent { index });
        }
        rows
    }

    /// Indices of the archived agents that have a row, in order: all but
    /// the ones under a folded agent.
    fn shown_archived(&self) -> Vec<usize> {
        let mut shown = Vec::with_capacity(self.archived.len());
        let mut index = 0;
        while index < self.archived.len() {
            shown.push(index);
            index = if self.archived_folded(index) {
                self.archived_spawn_end(index)
            } else {
                index + 1
            };
        }
        shown
    }

    /// Index just past archived agent `index` and the agents under it,
    /// which run up to the next agent no deeper than it.
    fn archived_spawn_end(&self, index: usize) -> usize {
        let Some(depth) = self.archived.items.get(index).map(|agent| agent.depth) else {
            return self.archived.len();
        };
        (index + 1..self.archived.len())
            .find(|&i| self.archived.items[i].depth <= depth)
            .unwrap_or(self.archived.len())
    }

    /// Whether archived agent `index` has agents under it, so it can fold.
    #[must_use]
    pub(crate) fn archived_has_spawns(&self, index: usize) -> bool {
        self.archived_spawn_end(index) > index + 1
    }

    /// Whether archived agent `index` is folded over the agents under it:
    /// it has some, and it was not opened since the Archive was entered.
    #[must_use]
    pub(crate) fn archived_folded(&self, index: usize) -> bool {
        self.archived
            .items
            .get(index)
            .is_some_and(|agent| !self.project_list.opened_archived.contains(&agent.id))
            && self.archived_has_spawns(index)
    }

    /// The archived agent that archived agent `index` sits under: the
    /// nearest one above it with a smaller depth.
    fn archived_parent(&self, index: usize) -> Option<usize> {
        let depth = self.archived.items.get(index)?.depth;
        (0..index)
            .rev()
            .find(|&i| self.archived.items[i].depth < depth)
    }

    /// The row standing for archived agent `index`: its own when drawn,
    /// else the outermost folded agent it is hidden under.
    fn shown_as(&self, index: usize) -> usize {
        std::iter::successors(self.archived_parent(index), |&i| self.archived_parent(i))
            .filter(|&ancestor| self.archived_folded(ancestor))
            .last()
            .unwrap_or(index)
    }

    /// The archived agent whose row stands for the agent on screen, if it
    /// is archived: that agent, or the folded agent it is hidden under.
    #[must_use]
    pub(crate) fn shown_active_archived(&self) -> Option<usize> {
        let index = self
            .archived
            .items
            .iter()
            .position(|agent| agent.is_active)?;
        Some(self.shown_as(index))
    }

    /// Fold or open archived agent `index`. One with nothing under it
    /// folds the agent it sits under instead. A highlight left inside a
    /// folded agent moves onto it.
    pub(in crate::app) fn toggle_archived(&mut self, index: usize) {
        let Some(index) = (if self.archived_has_spawns(index) {
            Some(index)
        } else {
            self.archived_parent(index)
        }) else {
            return;
        };
        let id = self.archived.items[index].id.clone();
        if !self.project_list.opened_archived.remove(&id) {
            self.project_list.opened_archived.insert(id);
        }
        self.select_archived_row(self.archived.selected);
    }

    /// Move the highlight `delta` drawn agent rows down (up when
    /// negative), stopping at either end.
    pub(in crate::app) fn move_archived_by(&mut self, delta: isize) {
        let shown = self.shown_archived();
        let Some(at) = shown.iter().position(|&i| i == self.archived.selected) else {
            self.select_archived_row(self.archived.selected);
            return;
        };
        let target = at
            .saturating_add_signed(delta)
            .min(shown.len().saturating_sub(1));
        if let Some(&index) = shown.get(target) {
            self.archived.selected = index;
        }
    }

    /// Highlighted archived agent: an index into [`Self::archived`].
    #[must_use]
    pub const fn selected_archived(&self) -> usize {
        self.archived.selected
    }

    /// First visible row of the Archive, counting day headers.
    #[must_use]
    pub const fn archived_scroll(&self) -> usize {
        self.archived.scroll
    }

    /// Keep the highlighted agent inside a list `visible_rows` tall. When
    /// it is the first agent of a day and comes in from above, its header
    /// comes in with it.
    pub fn sync_archived_scroll(&mut self, visible_rows: usize) {
        let rows = self.archive_rows();
        let Some(selected) = rows.iter().position(|row| {
            *row == ArchiveRow::Agent {
                index: self.archived.selected,
            }
        }) else {
            self.archived.scroll = 0;
            return;
        };
        let scroll = self.panels.wheel.place(
            WheelList::Archive,
            (self.archived.scroll, selected),
            (visible_rows, rows.len()),
        );
        if self.panels.wheel.is_detached(WheelList::Archive) {
            // Left where the wheel put it.
            self.archived.scroll = scroll;
            return;
        }
        let header_above = selected
            .checked_sub(1)
            .filter(|&above| matches!(rows.get(above), Some(ArchiveRow::Day(_))));
        self.archived.scroll = match header_above {
            Some(header) if scroll == selected && visible_rows > 1 => header,
            _ => scroll,
        };
    }

    /// Record where the Archive rows were drawn this frame.
    pub fn set_archived_rows_area(&mut self, area: Rect) {
        self.areas.archived_rows = Some(area);
    }

    /// Take a new Archive. The highlight stays on the agent it was on;
    /// with none before, it goes to the agent on screen if archived.
    pub(in crate::app) fn set_archived(&mut self, archived: Vec<AgentSummary>) {
        let highlighted = self.archived.selected_item().map(|agent| agent.id.clone());
        self.archived.items = archived;
        let index = match highlighted {
            Some(id) => self.archived.items.iter().position(|agent| agent.id == id),
            None => self.archived.items.iter().position(|agent| agent.is_active),
        };
        self.select_archived_row(index.unwrap_or(self.archived.selected));
    }

    /// Highlight archived agent `index`, clamped to the Archive; when it
    /// is folded away, the folded agent it is under.
    pub(in crate::app) fn select_archived_row(&mut self, index: usize) {
        let index = index.min(self.archived.len().saturating_sub(1));
        self.archived.selected = self.shown_as(index);
    }

    /// Archived agent drawn at a screen cell; `None` on a day header.
    pub(in crate::app) fn archived_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .archived_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let shown = self.archived.scroll + usize::from(row - rows.y);
        match self.archive_rows().get(shown)? {
            ArchiveRow::Agent { index } => Some(*index),
            ArchiveRow::Day(_) => None,
        }
    }

    /// Whether `column` is on the fold marker of archived agent `index`:
    /// its icon column, which `tree_row` puts after the row's inset and
    /// the tree lines, `AURA_WIDTH + 2` columns per level, and the blank
    /// after it.
    fn on_archived_marker(&self, index: usize, column: u16) -> bool {
        use crate::{animation::AURA_WIDTH, components::ROW_INSET};
        let Some(rows) = self.areas.archived_rows else {
            return false;
        };
        let start = ROW_INSET + self.archived.items[index].depth * (AURA_WIDTH + 2);
        let offset = usize::from(column.saturating_sub(rows.x));
        self.archived_has_spawns(index) && (start..=start + AURA_WIDTH).contains(&offset)
    }

    /// Clicking an archived agent's fold marker folds or opens it;
    /// clicking the rest of its row opens its chat.
    pub(in crate::app) fn click_archived_row(&mut self, column: u16, row: u16) {
        if let Some(index) = self.archived_row_at(column, row) {
            self.archived.selected = index;
            if self.on_archived_marker(index, column) {
                self.toggle_archived(index);
            } else {
                self.open_selected_archived();
            }
        }
    }

    /// Keys of the Archive. Returns whether the key was used.
    pub(in crate::app) fn handle_archive_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => self.move_archived_by(-1),
            KeyCode::Down => self.move_archived_by(1),
            KeyCode::PageUp => self.move_archived_by(-5),
            KeyCode::PageDown => self.move_archived_by(5),
            KeyCode::Home => self.move_archived_by(isize::MIN),
            KeyCode::End => self.move_archived_by(isize::MAX),
            KeyCode::Enter => self.open_selected_archived(),
            KeyCode::Char(' ') => self.toggle_archived(self.archived.selected),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Show the highlighted archived agent's chat, staying on the Archive.
    /// It stays archived.
    fn open_selected_archived(&mut self) {
        if let Some(id) = self.archived.selected_item().map(|a| a.id.clone()) {
            self.switch_on_screen(&id);
        }
    }

    /// `/unarchive`: bring the agent on screen back from the Archive, or
    /// say it is not there.
    pub(in crate::app) fn unarchive_active_agent(&mut self) {
        let id = self.active_agent_id.clone();
        if self.archived.items.iter().any(|agent| agent.id == id) {
            self.restore_agent(id);
        } else {
            self.notifications.current = Some((
                "This agent is not archived".to_string(),
                NotificationType::Warning,
            ));
        }
    }

    /// Bring the archived agent with hex id `id` back to the projects
    /// list and show that list.
    pub(in crate::app) fn restore_agent(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        self.send_event(UiEvent::RestoreAgent(id));
        self.switch_app(crate::app::AgentsTab::Projects);
    }

    /// Agent under the projects-list highlight, if it is on an agent.
    pub(in crate::app) fn highlighted_agent(&self) -> Option<String> {
        match self.project_rows().get(self.project_list.selected)? {
            ProjectListRow::Agent { index } => self.agents.get(*index).map(|a| a.id.clone()),
            _ => None,
        }
    }

    /// Delete in the projects list: archive the highlighted agent.
    pub(in crate::app) fn archive_selected_agent(&mut self) {
        if let Some(id) = self.highlighted_agent() {
            self.archive_agent(id);
        }
    }

    /// Shift+Delete in the projects list: ask to remove the highlighted
    /// agent's project.
    pub(in crate::app) fn remove_selected_project(&mut self) {
        if let Some(id) = self.highlighted_agent() {
            self.request_remove_project_of(&id);
        }
    }

    /// Ask to confirm removing the project agent `id` is in.
    pub(in crate::app) fn request_remove_project_of(&mut self, id: &str) {
        if let Some(key) = self.project_key_of_agent(id) {
            self.request_remove_project(key);
        }
    }

    /// Key of the project the listed agent `id` is in.
    pub(in crate::app) fn project_key_of_agent(&self, id: &str) -> Option<String> {
        let index = self.agents.iter().position(|a| a.id == id)?;
        self.project_key(self.project_root(index)?)
    }

    /// Move the agent with hex id `id`, and the agents under it, to the
    /// Archive.
    pub(in crate::app) fn archive_agent(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        self.send_event(UiEvent::ArchiveAgent(id));
    }

    /// Ask to confirm removing the project with key `key`.
    pub(in crate::app) fn request_remove_project(&mut self, key: String) {
        let named = |name: Option<&str>| {
            name.map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        };
        let summary = self.project_list.projects.iter().find(|p| p.id == key);
        let head = self
            .project_root_of_key(&key)
            .map(|root| &self.agents[root]);
        let Some(name) = named(summary.map(|p| p.name.as_str()))
            .or_else(|| head.and_then(|head| named(head.project.as_deref())))
            .or_else(|| head.map(|head| head.name.clone()))
        else {
            return;
        };
        self.notifications.current = Some((
            format!(
                "Remove project {name} and its agents from the list? Its files and records \
                 stay. Press y to confirm"
            ),
            NotificationType::Warning,
        ));
        self.pending.remove_project = Some(key);
    }

    /// Resolve a pending project removal with `key`. Returns `false` when
    /// nothing was pending.
    pub(in crate::app) fn handle_pending_remove_project(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending.remove_project.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.send_event(UiEvent::RemoveProject(id));
        } else {
            self.notifications.current = Some(("Cancelled".to_string(), NotificationType::Warning));
        }
        true
    }

    fn send_event(&self, event: UiEvent) {
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(event);
        }
    }
}

#[cfg(test)]
mod tests;
