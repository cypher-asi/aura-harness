//! The menu a right click opens on a project or an agent in the projects
//! list, or on an agent in the Archive. Its items are the list's own
//! actions (add an agent, project info, fold, archive, remove, unarchive),
//! gathered where the pointer is.
//!
//! The menu keeps the ids of what it was opened on, not list positions, so
//! the list may change under it. A click on an item or Enter runs it; Esc,
//! any other key or a click elsewhere closes it.

use super::{contains, AgentsTab, App, AppState, PanelFocus, ProjectListRow};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};

/// What the menu was opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuTarget {
    /// A project, by its key (see [`crate::events::AgentSummary::project_key`])
    Project(String),
    /// An agent, by id
    Agent(String),
    /// An agent in the Archive, by id. Kept apart from [`Self::Agent`]
    /// because archived agents are not in the projects list, so its
    /// items must not look the id up there.
    Archived(String),
}

/// An entry of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuItem {
    AddAgent,
    Info,
    Fold,
    Open,
    Archive,
    RemoveProject,
    Restore,
}

/// The open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMenu {
    pub target: MenuTarget,
    pub items: Vec<MenuItem>,
    /// The cell that was clicked; the menu opens below and right of it
    pub anchor: Position,
    /// Highlighted item
    pub selected: usize,
    /// Where the menu was drawn in the last frame
    pub area: Option<Rect>,
}

impl ContextMenu {
    /// The item drawn at a screen cell, inside the menu's border.
    fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.area?;
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        if !contains(Some(inner), column, row) {
            return None;
        }
        let index = usize::from(row - inner.y);
        (index < self.items.len()).then_some(index)
    }

    fn covers(&self, column: u16, row: u16) -> bool {
        contains(self.area, column, row)
    }
}

impl App {
    /// The open context menu, if any.
    pub(crate) const fn context_menu(&self) -> Option<&ContextMenu> {
        self.context_menu.as_ref()
    }

    /// Record where the context menu was drawn this frame.
    pub(crate) fn set_context_menu_area(&mut self, area: Rect) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.area = Some(area);
        }
    }

    /// What an item reads, for the target it was opened on.
    pub(crate) fn menu_label(&self, menu: &ContextMenu, item: MenuItem) -> &'static str {
        match item {
            MenuItem::AddAgent => "Add agent",
            MenuItem::Info => "Project info",
            MenuItem::Fold => {
                let folded = matches!(&menu.target, MenuTarget::Project(id) if self.collapsed_projects.contains(id));
                if folded {
                    "Expand"
                } else {
                    "Collapse"
                }
            }
            MenuItem::Open => "Open",
            MenuItem::Archive => "Archive",
            MenuItem::RemoveProject => "Remove project…",
            MenuItem::Restore => "Unarchive",
        }
    }

    /// A right click on the agents panel: highlight the project or agent
    /// under the pointer and open its menu there. Returns whether a menu
    /// opened.
    pub(super) fn open_context_menu(&mut self, column: u16, row: u16) -> bool {
        self.context_menu = None;
        match self.agents_tab {
            AgentsTab::Projects => {}
            AgentsTab::Archive => return self.open_archive_menu(column, row),
            AgentsTab::Roster => return false,
        }
        let Some(index) = self.project_row_at(column, row) else {
            return false;
        };
        let (target, items) = match self.project_rows().get(index).copied() {
            Some(ProjectListRow::Project { root, .. }) => (
                MenuTarget::Project(self.agents[root].project_key().to_string()),
                vec![
                    MenuItem::AddAgent,
                    MenuItem::Info,
                    MenuItem::Fold,
                    MenuItem::RemoveProject,
                ],
            ),
            Some(ProjectListRow::Agent { index }) => (
                MenuTarget::Agent(self.agents[index].id.clone()),
                vec![MenuItem::Open, MenuItem::Archive, MenuItem::RemoveProject],
            ),
            _ => return false,
        };
        self.focus = PanelFocus::Swarm;
        self.selected_agent = index;
        self.agents_scroll_detached = false;
        self.context_menu = Some(ContextMenu {
            target,
            items,
            anchor: Position::new(column, row),
            selected: 0,
            area: None,
        });
        true
    }

    /// A right click on an Archive row: highlight it and offer to bring
    /// it back.
    fn open_archive_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.archived_row_at(column, row) else {
            return false;
        };
        let Some(id) = self.archived.get(index).map(|a| a.id.clone()) else {
            return false;
        };
        self.focus = PanelFocus::Swarm;
        self.select_archived_row(index);
        self.context_menu = Some(ContextMenu {
            target: MenuTarget::Archived(id),
            items: vec![MenuItem::Restore],
            anchor: Position::new(column, row),
            selected: 0,
            area: None,
        });
        true
    }

    /// A pointer event while the menu is open. Returns whether the menu
    /// used it; a press outside closes the menu and is used up too.
    pub(super) fn context_menu_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        match kind {
            MouseEventKind::Moved => {
                if let Some(index) = menu.item_at(column, row) {
                    menu.selected = index;
                }
                menu.covers(column, row)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = menu.item_at(column, row) {
                    menu.selected = index;
                    self.run_context_menu_item();
                } else if !menu.covers(column, row) {
                    self.context_menu = None;
                }
                true
            }
            MouseEventKind::Down(_) => {
                if menu.covers(column, row) {
                    return true;
                }
                self.context_menu = None;
                false
            }
            MouseEventKind::Up(_) | MouseEventKind::Drag(_) => menu.covers(column, row),
            _ => {
                self.context_menu = None;
                false
            }
        }
    }

    /// Keys while the menu is open. Returns whether the key was used; any
    /// other key closes the menu and goes on to where it would have.
    pub(super) fn handle_context_menu_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(self.state, AppState::Idle | AppState::Processing) {
            self.context_menu = None;
            return false;
        }
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        let last = menu.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => menu.selected = menu.selected.checked_sub(1).unwrap_or(last),
            KeyCode::Down => {
                menu.selected = if menu.selected >= last {
                    0
                } else {
                    menu.selected + 1
                }
            }
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = last,
            KeyCode::Enter => self.run_context_menu_item(),
            KeyCode::Esc => self.context_menu = None,
            _ => {
                self.context_menu = None;
                return false;
            }
        }
        true
    }

    /// Close the menu and run its highlighted item on its target.
    fn run_context_menu_item(&mut self) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let Some(&item) = menu.items.get(menu.selected) else {
            return;
        };
        match menu.target {
            MenuTarget::Archived(id) => {
                if item == MenuItem::Restore {
                    self.restore_agent(id);
                }
            }
            MenuTarget::Project(key) => {
                let Some(root) = self.project_root_of_key(&key) else {
                    return;
                };
                match item {
                    MenuItem::AddAgent => self.add_agent_to_project(root),
                    MenuItem::Fold => self.toggle_project(root),
                    MenuItem::Info => self.show_project_info(&key),
                    MenuItem::RemoveProject => self.request_remove_project(key),
                    MenuItem::Open | MenuItem::Archive | MenuItem::Restore => {}
                }
            }
            MenuTarget::Agent(id) => {
                if !self.agents.iter().any(|agent| agent.id == id) {
                    return;
                }
                match item {
                    MenuItem::Open => self.request_agent_switch(id),
                    MenuItem::Archive => self.archive_agent(id),
                    MenuItem::RemoveProject => self.request_remove_project_of(&id),
                    MenuItem::Restore => self.restore_agent(id),
                    MenuItem::AddAgent | MenuItem::Fold | MenuItem::Info => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, UiCommand, UiEvent};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
        (app, rx)
    }

    /// Draw the app `height` rows tall, a line per row.
    fn screen(app: &mut App, height: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(120, height)).unwrap();
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

    fn press(app: &mut App, button: MouseButton, (column, row): (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(button),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn right_click_in_the_input_box_asks_for_a_paste() {
        let mut app = App::new();
        app.set_input_area(Rect::new(10, 20, 40, 3));
        press(&mut app, MouseButton::Right, (12, 21));
        assert!(app.take_paste_request());
        press(&mut app, MouseButton::Right, (12, 5));
        assert!(!app.take_paste_request(), "outside the box");
    }

    #[test]
    fn super_v_asks_for_a_paste() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER));
        assert!(app.take_paste_request());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn right_clicking_a_project_lists_its_actions() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        for label in ["Add agent", "Project info", "Collapse", "Remove project…"] {
            assert!(
                rows.iter().any(|row| row.contains(label)),
                "{label}:\n{}",
                rows.join("\n")
            );
        }
        assert_eq!(app.focus(), PanelFocus::Swarm);
    }

    #[test]
    fn a_project_item_runs_on_the_project_clicked() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Add agent"));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::AddAgentToProject(id)) if id == "cc"));
    }

    #[test]
    fn right_clicking_an_agent_archives_it_from_the_keys() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Agent("bb".into()));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    }

    /// The app with two archived agents, showing the Archive tab.
    fn archive_app() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        // Left switches the agents panel's tab only while it has focus.
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        (app, rx)
    }

    #[test]
    fn right_clicking_an_archived_agent_unarchives_it() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "EE"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Archived("ee".into()));
        assert_eq!(menu.items, vec![MenuItem::Restore]);
        assert_eq!(app.selected_archived(), 1, "the row is highlighted");
        assert!(rx.try_recv().is_err(), "opening the menu runs nothing");

        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Unarchive"));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "ee"));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
    }

    #[test]
    fn the_archive_menu_unarchives_from_the_keys() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "DD"));
        assert!(app.context_menu().is_some());
        app.handle_key(key(KeyCode::Enter));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "dd"));
    }

    #[test]
    fn right_clicking_empty_archive_space_opens_no_menu() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        let (column, row) = cell_of(&rows, "EE");
        // Several rows below the last archived agent, still in the panel.
        press(&mut app, MouseButton::Right, (column, row + 4));
        assert!(app.context_menu().is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn project_info_shows_the_folder_and_closes_on_any_key() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetProjects(vec![
            crate::events::ProjectSummary {
                id: "aa".into(),
                name: "site".into(),
                folder: "/work/site".into(),
                folder_exists: true,
                ..Default::default()
            },
            crate::events::ProjectSummary {
                id: "cc".into(),
                name: "notes".into(),
                folder: "/work/notes".into(),
                folder_exists: false,
                created: "2026-11-19 14:00".into(),
                listed_agents: 1,
                archived_agents: 2,
            },
        ]));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Project info"));
        assert!(rx.try_recv().is_err(), "nothing goes to the session");
        assert_eq!(app.state(), AppState::ShowingProjectInfo);
        assert_eq!(app.project_info().map(|p| p.id.as_str()), Some("cc"));

        let rows = screen(&mut app, 30);
        for text in [
            "NOTES",
            "/work/notes",
            "The folder is missing",
            "2026-11-19 14:00",
        ] {
            assert!(
                rows.iter().any(|row| row.contains(text)),
                "{text}:\n{}",
                rows.join("\n")
            );
        }
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.state(), AppState::Idle);
        assert!(app.project_info().is_none());

        // `/project` shows the active agent's project.
        app.handle_command("/project");
        assert_eq!(app.project_info().map(|p| p.id.as_str()), Some("aa"));
    }

    #[test]
    fn esc_or_a_click_elsewhere_closes_the_menu() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        let project = cell_of(&rows, "CC");
        press(&mut app, MouseButton::Right, project);
        assert!(app.context_menu().is_some());
        app.handle_key(key(KeyCode::Esc));
        assert!(app.context_menu().is_none());

        press(&mut app, MouseButton::Right, project);
        screen(&mut app, 30);
        press(&mut app, MouseButton::Left, (110, 1));
        assert!(app.context_menu().is_none());
        assert!(rx.try_recv().is_err(), "closing runs nothing");
    }

    #[test]
    fn the_menu_stays_on_screen_near_the_bottom() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 12);
        let at = cell_of(&rows, "BB");
        press(&mut app, MouseButton::Right, at);
        let rows = screen(&mut app, 12);
        let area = app.context_menu().and_then(|m| m.area).expect("drawn");
        assert!(area.bottom() <= 12, "{area:?}");
        assert!(
            area.bottom() <= at.1,
            "opens upward over the list: {area:?}"
        );
        assert!(rows.iter().any(|row| row.contains("Remove project…")));
    }
}
