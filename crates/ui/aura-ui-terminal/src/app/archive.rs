//! The agents panel's Archive tab, and moving agents and projects out of
//! the projects list.
//!
//! The Archive lists archived agents newest-archived first, under a header
//! for the day each was archived. Clicking an archived agent, or Enter on
//! it, opens its chat and stays on the Archive; only the row's right-click
//! "Unarchive" or `/unarchive` brings it back to the projects list.
//!
//! The highlight (`selected_archived`) is an index into the archived
//! agents, never a header; the scroll (`archived_scroll`) counts drawn
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

use super::{contains, follow_selection, App, NotificationType, ProjectListRow};
use crate::events::{AgentSummary, UiEvent};
use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// A row of the Archive as drawn: a day header or an archived agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveRow {
    /// The day the agents below it were archived: `Today`, `Yesterday`,
    /// a `%Y-%m-%d` date, or `Earlier` when the time is unknown.
    Day(String),
    /// The archived agent at `index` in [`App::archived`].
    Agent {
        /// Index into [`App::archived`].
        index: usize,
    },
}

/// Header of the day an agent archived at `ms` (Unix milliseconds) falls
/// on, as seen at `now`: `Today`, `Yesterday`, else its local date. An
/// unknown time reads `Earlier`.
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
        day.format("%Y-%m-%d").to_string()
    }
}

impl App {
    /// Archived agents, each followed by the agents under it.
    #[must_use]
    pub fn archived(&self) -> &[AgentSummary] {
        &self.archived
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
            let agent = &self.archived[index];
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
        let Some(depth) = self.archived.get(index).map(|agent| agent.depth) else {
            return self.archived.len();
        };
        (index + 1..self.archived.len())
            .find(|&i| self.archived[i].depth <= depth)
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
            .get(index)
            .is_some_and(|agent| !self.opened_archived.contains(&agent.id))
            && self.archived_has_spawns(index)
    }

    /// The archived agent that archived agent `index` sits under: the
    /// nearest one above it with a smaller depth.
    fn archived_parent(&self, index: usize) -> Option<usize> {
        let depth = self.archived.get(index)?.depth;
        (0..index).rev().find(|&i| self.archived[i].depth < depth)
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
        let index = self.archived.iter().position(|agent| agent.is_active)?;
        Some(self.shown_as(index))
    }

    /// Fold every archived agent over the agents under it, as on entering
    /// the Archive. A highlight inside a folded agent moves onto it.
    pub(super) fn fold_all_archived(&mut self) {
        self.opened_archived.clear();
        self.select_archived_row(self.selected_archived);
    }

    /// Fold or open archived agent `index`. One with nothing under it
    /// folds the agent it sits under instead. A highlight left inside a
    /// folded agent moves onto it.
    pub(super) fn toggle_archived(&mut self, index: usize) {
        let Some(index) = (if self.archived_has_spawns(index) {
            Some(index)
        } else {
            self.archived_parent(index)
        }) else {
            return;
        };
        let id = self.archived[index].id.clone();
        if !self.opened_archived.remove(&id) {
            self.opened_archived.insert(id);
        }
        self.select_archived_row(self.selected_archived);
    }

    /// Move the highlight `delta` drawn agent rows down (up when
    /// negative), stopping at either end.
    pub(super) fn move_archived_by(&mut self, delta: isize) {
        let shown = self.shown_archived();
        let Some(at) = shown.iter().position(|&i| i == self.selected_archived) else {
            self.select_archived_row(self.selected_archived);
            return;
        };
        let target = at
            .saturating_add_signed(delta)
            .min(shown.len().saturating_sub(1));
        if let Some(&index) = shown.get(target) {
            self.selected_archived = index;
        }
    }

    /// Highlighted archived agent: an index into [`Self::archived`].
    #[must_use]
    pub const fn selected_archived(&self) -> usize {
        self.selected_archived
    }

    /// First visible row of the Archive, counting day headers.
    #[must_use]
    pub const fn archived_scroll(&self) -> usize {
        self.archived_scroll
    }

    /// Keep the highlighted agent inside a list `visible_rows` tall. When
    /// it is the first agent of a day and comes in from above, its header
    /// comes in with it.
    pub fn sync_archived_scroll(&mut self, visible_rows: usize) {
        let rows = self.archive_rows();
        let Some(selected) = rows.iter().position(|row| {
            *row == ArchiveRow::Agent {
                index: self.selected_archived,
            }
        }) else {
            self.archived_scroll = 0;
            return;
        };
        let scroll = follow_selection(self.archived_scroll, selected, visible_rows, rows.len());
        let header_above = selected
            .checked_sub(1)
            .filter(|&above| matches!(rows.get(above), Some(ArchiveRow::Day(_))));
        self.archived_scroll = match header_above {
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
    pub(super) fn set_archived(&mut self, archived: Vec<AgentSummary>) {
        let highlighted = self
            .archived
            .get(self.selected_archived)
            .map(|agent| agent.id.clone());
        self.archived = archived;
        let index = match highlighted {
            Some(id) => self.archived.iter().position(|agent| agent.id == id),
            None => self.archived.iter().position(|agent| agent.is_active),
        };
        self.select_archived_row(index.unwrap_or(self.selected_archived));
    }

    /// Highlight archived agent `index`, clamped to the Archive; when it
    /// is folded away, the folded agent it is under.
    pub(super) fn select_archived_row(&mut self, index: usize) {
        let index = index.min(self.archived.len().saturating_sub(1));
        self.selected_archived = self.shown_as(index);
    }

    /// Archived agent drawn at a screen cell; `None` on a day header.
    pub(super) fn archived_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .archived_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let shown = self.archived_scroll + usize::from(row - rows.y);
        match self.archive_rows().get(shown)? {
            ArchiveRow::Agent { index } => Some(*index),
            ArchiveRow::Day(_) => None,
        }
    }

    /// Whether `column` is on the fold marker of archived agent `index`:
    /// its icon column, after the leading space and two columns per level.
    fn on_archived_marker(&self, index: usize, column: u16) -> bool {
        use crate::animation::AURA_WIDTH;
        let Some(rows) = self.areas.archived_rows else {
            return false;
        };
        let start = 1 + 2 * self.archived[index].depth;
        let offset = usize::from(column.saturating_sub(rows.x));
        self.archived_has_spawns(index) && (start..=start + AURA_WIDTH).contains(&offset)
    }

    /// Clicking an archived agent's fold marker folds or opens it;
    /// clicking the rest of its row opens its chat.
    pub(super) fn click_archived_row(&mut self, column: u16, row: u16) {
        if let Some(index) = self.archived_row_at(column, row) {
            self.selected_archived = index;
            if self.on_archived_marker(index, column) {
                self.toggle_archived(index);
            } else {
                self.open_selected_archived();
            }
        }
    }

    /// Keys of the Archive. Returns whether the key was used.
    pub(super) fn handle_archive_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => self.move_archived_by(-1),
            KeyCode::Down => self.move_archived_by(1),
            KeyCode::PageUp => self.move_archived_by(-5),
            KeyCode::PageDown => self.move_archived_by(5),
            KeyCode::Home => self.move_archived_by(isize::MIN),
            KeyCode::End => self.move_archived_by(isize::MAX),
            KeyCode::Enter => self.open_selected_archived(),
            KeyCode::Char(' ') => self.toggle_archived(self.selected_archived),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Show the highlighted archived agent's chat, staying on the Archive.
    /// It stays archived.
    fn open_selected_archived(&mut self) {
        if let Some(id) = self
            .archived
            .get(self.selected_archived)
            .map(|a| a.id.clone())
        {
            self.request_agent_switch(id);
        }
    }

    /// `/unarchive`: bring the agent on screen back from the Archive, or
    /// say it is not there.
    pub(super) fn unarchive_active_agent(&mut self) {
        let id = self.active_agent_id.clone();
        if self.archived.iter().any(|agent| agent.id == id) {
            self.restore_agent(id);
        } else {
            self.notification = Some((
                "This agent is not archived".to_string(),
                NotificationType::Warning,
            ));
        }
    }

    /// Bring the archived agent with hex id `id` back to the projects
    /// list and show that list.
    pub(super) fn restore_agent(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        self.send_event(UiEvent::RestoreAgent(id));
        self.show_agents_tab(super::AgentsTab::Projects);
    }

    /// Agent under the projects-list highlight, if it is on an agent.
    fn highlighted_agent(&self) -> Option<String> {
        match self.project_rows().get(self.selected_agent)? {
            ProjectListRow::Agent { index } => self.agents.get(*index).map(|a| a.id.clone()),
            _ => None,
        }
    }

    /// Delete in the projects list: archive the highlighted agent.
    pub(super) fn archive_selected_agent(&mut self) {
        if let Some(id) = self.highlighted_agent() {
            self.archive_agent(id);
        }
    }

    /// Shift+Delete in the projects list: ask to remove the highlighted
    /// agent's project.
    pub(super) fn remove_selected_project(&mut self) {
        if let Some(id) = self.highlighted_agent() {
            self.request_remove_project_of(&id);
        }
    }

    /// Ask to confirm removing the project agent `id` is in.
    pub(super) fn request_remove_project_of(&mut self, id: &str) {
        if let Some(key) = self.project_key_of_agent(id) {
            self.request_remove_project(key);
        }
    }

    /// Key of the project the listed agent `id` is in.
    pub(super) fn project_key_of_agent(&self, id: &str) -> Option<String> {
        let index = self.agents.iter().position(|a| a.id == id)?;
        self.project_key(self.project_root(index)?)
    }

    /// Move the agent with hex id `id`, and the agents under it, to the
    /// Archive.
    pub(super) fn archive_agent(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        self.send_event(UiEvent::ArchiveAgent(id));
    }

    /// Ask to confirm removing the project with key `key`.
    pub(super) fn request_remove_project(&mut self, key: String) {
        let named = |name: Option<&str>| {
            name.map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        };
        let summary = self.projects.iter().find(|p| p.id == key);
        let head = self
            .project_root_of_key(&key)
            .map(|root| &self.agents[root]);
        let Some(name) = named(summary.map(|p| p.name.as_str()))
            .or_else(|| head.and_then(|head| named(head.project.as_deref())))
            .or_else(|| head.map(|head| head.name.clone()))
        else {
            return;
        };
        self.notification = Some((
            format!(
                "Remove project {name} and its agents from the list? Its files and records \
                 stay. Press y to confirm"
            ),
            NotificationType::Warning,
        ));
        self.pending_remove_project = Some(key);
    }

    /// Resolve a pending project removal with `key`. Returns `false` when
    /// nothing was pending.
    pub(super) fn handle_pending_remove_project(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending_remove_project.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.send_event(UiEvent::RemoveProject(id));
        } else {
            self.notification = Some(("Cancelled".to_string(), NotificationType::Warning));
        }
        true
    }

    fn send_event(&self, event: UiEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AgentsTab, PanelFocus};
    use super::*;
    use crate::events::UiCommand;
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn agent(id: &str, depth: usize) -> AgentSummary {
        AgentSummary {
            id: id.into(),
            name: id.to_uppercase(),
            depth,
            ..AgentSummary::default()
        }
    }

    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut app = App::new();
        app.event_tx = Some(tx);
        app.process_command(UiCommand::SetAgents(vec![
            agent("aa", 0),
            agent("bb", 1),
            agent("cc", 0),
        ]));
        app.process_command(UiCommand::SetActiveAgent("bb".into()));
        app.focus = PanelFocus::Swarm;
        (app, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The screen at `width` columns, a line per row.
    fn screen(app: &mut App, width: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        let theme = crate::Theme::cyber();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    #[test]
    fn the_empty_archive_says_so_inside_the_panels_padding() {
        let (mut app, _rx) = app();
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        for width in [160, 200] {
            let rows = screen(&mut app, width);
            let first = rows.iter().position(|row| row.contains("No archived"));
            let first = first.unwrap_or_else(|| panic!("no message:\n{}", rows.join("\n")));
            // The panel's left border, then two columns of padding.
            let border = rows[first].find('│').expect("the panel's border");
            let lines: Vec<&str> = rows[first..]
                .iter()
                .map(|row| {
                    row[border + '│'.len_utf8()..]
                        .split('│')
                        .next()
                        .unwrap_or("")
                })
                .take_while(|line| !line.trim().is_empty())
                .collect();
            assert_eq!(lines.len(), 1, "{width}: {lines:?}");
            let line = lines[0];
            assert!(line.starts_with("  "), "{width}: {line:?}");
            assert!(!line.starts_with("   "), "{width}: {line:?}");
            assert!(line.ends_with("  "), "{width}: {line:?}");
            assert_eq!(line.trim(), "No archived agents.");
        }
    }

    #[test]
    fn archived_agents_keep_their_indent_under_the_one_they_ran_for() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        app.handle_key(key(KeyCode::Char(' ')));
        let rows = screen(&mut app, 100);
        let column = |name: &str| {
            let row = rows.iter().find(|row| row.contains(name));
            let row = row.unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
            row[..row.find(name).unwrap_or(0)].chars().count()
        };
        assert_eq!(column("EE"), column("DD") + 2);
    }

    #[test]
    fn delete_archives_the_highlighted_agent() {
        let (mut app, mut rx) = app();
        app.handle_key(key(KeyCode::Delete));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    }

    #[test]
    fn shift_delete_removes_the_project_only_once_confirmed() {
        let (mut app, mut rx) = app();
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT));
        assert!(rx.try_recv().is_err());
        assert!(app
            .notification()
            .is_some_and(|(text, _)| text.contains("AA")));
        app.handle_key(key(KeyCode::Char('n')));
        assert!(rx.try_recv().is_err());

        // It names the project, by the key of the one `bb` is in.
        app.handle_command("/remove-project");
        app.handle_key(key(KeyCode::Char('y')));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RemoveProject(id)) if id == "aa"));
    }

    #[test]
    fn enter_in_the_archive_opens_the_agent_and_stays_there() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        assert_eq!(app.archived().len(), 2);

        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_archived(), 1);
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
        assert!(rx.try_recv().is_err(), "nothing is restored");
        assert_eq!(app.agents_tab(), AgentsTab::Archive);

        app.handle_command("/archive");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    }

    fn click(app: &mut App, (column, row): (u16, u16)) {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// The cell of the first row whose text contains `name`.
    fn cell_of(rows: &[String], name: &str) -> (u16, u16) {
        let row = rows
            .iter()
            .position(|row| row.contains(name))
            .unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
        let line = &rows[row];
        let column = line[..line.find(name).unwrap_or(0)].chars().count();
        (u16::try_from(column).unwrap(), u16::try_from(row).unwrap())
    }

    #[test]
    fn clicking_an_archived_agent_opens_it_and_stays_on_the_archive() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        app.handle_key(key(KeyCode::Char(' ')));
        let rows = screen(&mut app, 100);
        click(&mut app, cell_of(&rows, "EE"));
        assert_eq!(app.selected_archived(), 1);
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
        assert!(rx.try_recv().is_err(), "nothing is restored");
        assert_eq!(app.agents_tab(), AgentsTab::Archive);

        // A day header opens nothing.
        let rows = screen(&mut app, 100);
        click(&mut app, cell_of(&rows, "Earlier"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unarchive_restores_only_an_archived_agent_on_screen() {
        let (mut app, mut rx) = app();
        app.handle_command("/unarchive");
        assert!(rx.try_recv().is_err(), "bb is listed");
        assert!(app
            .notification()
            .is_some_and(|(text, _)| text == "This agent is not archived"));

        let mut ee = agent("ee", 0);
        ee.is_active = true;
        app.process_command(UiCommand::SetArchivedAgents(vec![ee]));
        app.process_command(UiCommand::SetActiveAgent("ee".into()));
        app.handle_command("/unarchive");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "ee"));
    }

    /// Noon local time on `day` of January 2024.
    fn noon(day: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(2024, 1, day, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn ms_of(time: DateTime<Local>) -> Option<u64> {
        u64::try_from(time.timestamp_millis()).ok()
    }

    #[test]
    fn archive_day_names_today_yesterday_a_date_or_earlier() {
        let now = noon(10);
        let cases = [
            (ms_of(noon(10)), "Today"),
            (ms_of(noon(9)), "Yesterday"),
            (ms_of(noon(3)), "2024-01-03"),
            (None, "Earlier"),
        ];
        for (ms, want) in cases {
            assert_eq!(archive_day(ms, now), want, "{ms:?}");
        }
    }

    fn archived_at(id: &str, depth: usize, at: DateTime<Local>) -> AgentSummary {
        AgentSummary {
            archived_at_ms: ms_of(at),
            ..agent(id, depth)
        }
    }

    #[test]
    fn archive_rows_put_a_header_before_each_new_day() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            archived_at("dd", 0, noon(10)),
            // Under dd: it stays with its head whatever its own time.
            archived_at("ee", 1, noon(3)),
            archived_at("ff", 0, noon(10)),
            archived_at("gg", 0, noon(9)),
            agent("hh", 0),
        ]));
        app.toggle_archived(0);
        let agent_row = |index| ArchiveRow::Agent { index };
        assert_eq!(
            app.archive_rows_at(noon(10)),
            vec![
                ArchiveRow::Day("Today".into()),
                agent_row(0),
                agent_row(1),
                agent_row(2),
                ArchiveRow::Day("Yesterday".into()),
                agent_row(3),
                ArchiveRow::Day("Earlier".into()),
                agent_row(4),
            ]
        );

        app.process_command(UiCommand::SetArchivedAgents(Vec::new()));
        assert!(app.archive_rows().is_empty());
    }

    #[test]
    fn the_archive_draws_its_day_headers() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            archived_at("dd", 0, Local::now()),
            agent("ee", 0),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        let rows = screen(&mut app, 100);
        let row_of = |name: &str| cell_of(&rows, name).1;
        assert!(row_of("Today") < row_of("DD"));
        assert!(row_of("DD") < row_of("Earlier"));
        assert!(row_of("Earlier") < row_of("EE"));
    }

    #[test]
    fn the_highlight_follows_its_agent_when_a_newer_one_comes_in() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 0),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_archived(), 1);

        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("zz", 0),
            agent("dd", 0),
            agent("ee", 0),
        ]));
        assert_eq!(app.archived()[app.selected_archived()].id, "ee");
    }

    #[test]
    fn the_first_archive_highlights_the_agent_on_screen() {
        let (mut app, _rx) = app();
        let mut ee = agent("ee", 0);
        ee.is_active = true;
        app.process_command(UiCommand::SetArchivedAgents(vec![agent("dd", 0), ee]));
        assert_eq!(app.selected_archived(), 1);
    }

    /// An Archive of `dd` over `ee` over `ff`, then `gg` alone, then `hh`
    /// over `ii`, shown in the Archive tab.
    fn nested_archive() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
            agent("ff", 2),
            agent("gg", 0),
            agent("hh", 0),
            agent("ii", 1),
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        (app, rx)
    }

    /// The archived agents drawn, by id.
    fn shown_ids(app: &App) -> Vec<&str> {
        app.archive_rows()
            .into_iter()
            .filter_map(|row| match row {
                ArchiveRow::Agent { index } => Some(app.archived()[index].id.as_str()),
                ArchiveRow::Day(_) => None,
            })
            .collect()
    }

    #[test]
    fn entering_the_archive_folds_every_agent_over_its_subagents() {
        let (mut app, _rx) = nested_archive();
        assert_eq!(shown_ids(&app), ["dd", "gg", "hh"]);
        assert!(app.archived_folded(0) && app.archived_folded(4));
        assert!(!app.archived_folded(3), "gg has nothing to fold");

        // Opened ones stay open while on the Archive, one level at a time.
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(shown_ids(&app), ["dd", "ee", "gg", "hh"]);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(shown_ids(&app), ["dd", "ee", "ff", "gg", "hh"]);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Right));
        assert_eq!(shown_ids(&app).len(), 5, "other tabs leave it alone");

        // Coming back folds them all again.
        app.show_agents_tab(AgentsTab::Projects);
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(shown_ids(&app), ["dd", "gg", "hh"]);
        assert_eq!(app.selected_archived(), 0, "the highlight moved onto dd");
    }

    #[test]
    fn the_highlight_moves_over_drawn_rows_only() {
        let (mut app, _rx) = nested_archive();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.archived()[app.selected_archived()].id, "gg");
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.archived()[app.selected_archived()].id, "hh");
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_archived(), 0);

        // Space on a subagent with nothing under it folds its parent and
        // moves the highlight onto it.
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.archived()[app.selected_archived()].id, "ii");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.archived()[app.selected_archived()].id, "hh");
        assert!(app.archived_folded(4));
    }

    #[test]
    fn a_hidden_agent_on_screen_lights_the_folded_agent_over_it() {
        let (mut app, _rx) = app();
        let mut ff = agent("ff", 2);
        ff.is_active = true;
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
            ff,
        ]));
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(app.shown_active_archived(), Some(0));
        assert_eq!(app.selected_archived(), 0);
        app.toggle_archived(0);
        assert_eq!(app.shown_active_archived(), Some(1));
    }

    #[test]
    fn the_marker_folds_and_the_rest_of_the_row_opens_the_chat() {
        let (mut app, mut rx) = nested_archive();
        let rows = screen(&mut app, 100);
        assert!(rows
            .iter()
            .any(|row| row.contains("▸") && row.contains("DD")));
        assert!(!rows.iter().any(|row| row.contains("EE")));
        click(&mut app, cell_of(&rows, "▸"));
        assert!(rx.try_recv().is_err(), "the marker opens no chat");
        let rows = screen(&mut app, 100);
        assert!(rows
            .iter()
            .any(|row| row.contains("▾") && row.contains("DD")));
        assert!(rows.iter().any(|row| row.contains("EE")));

        click(&mut app, cell_of(&rows, "EE"));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "ee"));
    }
}
