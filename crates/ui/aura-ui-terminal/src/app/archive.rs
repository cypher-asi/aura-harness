//! The agents panel's Archive tab, and moving agents and projects out of
//! the projects list.
//!
//! Archiving is reversible, so it asks no confirmation: Delete on an agent
//! in the projects list, or `/archive`. Removing a project drops it and
//! its agents from the list for good (their records and folder stay on
//! disk), so it waits for `y`: Shift+Delete, or `/remove-project`.

use super::{contains, follow_selection, App, NotificationType, ProjectListRow};
use crate::events::{AgentSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

impl App {
    /// Archived agents, each followed by the agents under it.
    #[must_use]
    pub fn archived(&self) -> &[AgentSummary] {
        &self.archived
    }

    /// Highlighted row of the Archive.
    #[must_use]
    pub const fn selected_archived(&self) -> usize {
        self.selected_archived
    }

    /// First visible row of the Archive.
    #[must_use]
    pub const fn archived_scroll(&self) -> usize {
        self.archived_scroll
    }

    /// Keep the highlighted row inside a list `visible_rows` tall.
    pub fn sync_archived_scroll(&mut self, visible_rows: usize) {
        self.archived_scroll = follow_selection(
            self.archived_scroll,
            self.selected_archived,
            visible_rows,
            self.archived.len(),
        );
    }

    /// Record where the Archive rows were drawn this frame.
    pub fn set_archived_rows_area(&mut self, area: Rect) {
        self.areas.archived_rows = Some(area);
    }

    pub(super) fn set_archived(&mut self, archived: Vec<AgentSummary>) {
        self.archived = archived;
        self.select_archived_row(self.selected_archived);
    }

    pub(super) fn select_archived_row(&mut self, row: usize) {
        self.selected_archived = row.min(self.archived.len().saturating_sub(1));
    }

    /// Archive row drawn at a screen cell.
    pub(super) fn archived_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .archived_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.archived_scroll + usize::from(row - rows.y);
        (index < self.archived.len()).then_some(index)
    }

    /// Clicking an archived agent restores it.
    pub(super) fn click_archived_row(&mut self, column: u16, row: u16) {
        if let Some(index) = self.archived_row_at(column, row) {
            self.selected_archived = index;
            self.restore_selected();
        }
    }

    /// Keys of the Archive. Returns whether the key was used.
    pub(super) fn handle_archive_key(&mut self, key: KeyEvent) -> bool {
        let last = self.archived.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.select_archived_row(self.selected_archived.saturating_sub(1)),
            KeyCode::Down => self.select_archived_row(self.selected_archived + 1),
            KeyCode::PageUp => self.select_archived_row(self.selected_archived.saturating_sub(5)),
            KeyCode::PageDown => self.select_archived_row(self.selected_archived + 5),
            KeyCode::Home => self.select_archived_row(0),
            KeyCode::End => self.select_archived_row(last),
            KeyCode::Enter => self.restore_selected(),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Bring the highlighted archived agent back to the projects list.
    fn restore_selected(&mut self) {
        if let Some(id) = self
            .archived
            .get(self.selected_archived)
            .map(|a| a.id.clone())
        {
            self.restore_agent(id);
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
        let Some(root) = self.project_root_of_key(&key) else {
            return;
        };
        let head = &self.agents[root];
        let name = head
            .project
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&head.name);
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
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        for width in [100, 160] {
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
            assert!(lines.len() >= 2, "{lines:?}");
            for line in &lines {
                assert!(line.starts_with("  "), "{width}: {line:?}");
                assert!(!line.starts_with("   "), "{width}: {line:?}");
                assert!(line.ends_with("  "), "{width}: {line:?}");
            }
            let text = lines
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                text,
                "No archived agents. Delete on an agent in PROJECTS archives it."
            );
        }
    }

    #[test]
    fn archived_agents_keep_their_indent_under_the_one_they_ran_for() {
        let (mut app, _rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.handle_key(key(KeyCode::Left));
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
    fn the_archive_lists_archived_agents_and_restores_them() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        assert_eq!(app.archived().len(), 2);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_archived(), 1);
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "ee"));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);

        app.handle_command("/archive");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    }
}
