
use super::*;
use crate::app::AgentsTab;
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

/// Draw the app 120 columns wide and `height` rows tall.
fn draw(app: &mut App, height: u16) -> ratatui::buffer::Buffer {
    draw_sized(app, 120, height)
}

/// Draw the app `width` columns wide and `height` rows tall.
fn draw_sized(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let theme = crate::Theme::cyber();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Draw the app 120 columns wide and `height` rows tall, a line per row.
fn screen(app: &mut App, height: u16) -> Vec<String> {
    screen_sized(app, 120, height)
}

/// Draw the app `width` columns wide and `height` rows tall, a line
/// per row.
fn screen_sized(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let buffer = draw_sized(app, width, height);
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
fn the_item_under_the_pointer_and_the_row_clicked_are_lit() {
    let (mut app, _rx) = app();
    let rows = screen(&mut app, 30);
    let project = cell_of(&rows, "CC");
    press(&mut app, MouseButton::Right, project);
    let rows = screen(&mut app, 30);
    let (add, rename) = (cell_of(&rows, "Add agent"), cell_of(&rows, "Project info"));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: rename.0,
        row: rename.1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.context_menu().map(|menu| menu.selected), Some(1));

    let buffer = draw(&mut app, 30);
    let hover = Some(crate::Theme::cyber().colors.surface_hover);
    let area = app
        .context_menu()
        .and_then(|menu| menu.area)
        .expect("drawn");
    let row_end = area.right() - 2;
    assert_eq!(buffer[rename].style().bg, hover, "the item hovered");
    assert_eq!(
        buffer[(row_end, rename.1)].style().bg,
        hover,
        "across its row"
    );
    assert_ne!(buffer[add].style().bg, hover, "the other items");
    assert_eq!(buffer[project].style().bg, hover, "the project behind");
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

#[test]
fn an_agent_with_spawns_folds_from_its_menu() {
    let (mut app, _rx) = app();
    // The project's header reads "AA" too; the agent is the next row.
    let agent_aa = |rows: &[String]| {
        let (column, row) = cell_of(rows, "AA");
        (column, row + 1)
    };
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, agent_aa(&rows));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.target, MenuTarget::Agent("aa".into()));
    assert!(menu.items.contains(&MenuItem::Fold));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Left, cell_of(&rows, "Collapse"));
    assert!(app.agent_folded(0));
    assert!(!app
        .project_rows()
        .contains(&ProjectListRow::Agent { index: 1 }));

    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, agent_aa(&rows));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Left, cell_of(&rows, "Expand"));
    assert!(!app.agent_folded(0));

    // A leaf has nothing to fold.
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    let menu = app.context_menu().expect("the menu opened");
    assert!(!menu.items.contains(&MenuItem::Fold));
}

/// The app with two archived agents, showing the Archive tab with the
/// first opened over the second, and on screen.
fn archive_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", 0),
        agent("ee", 1),
    ]));
    app.focus = PanelFocus::Swarm;
    app.show_agents_tab(AgentsTab::Archive);
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "dd"));
    assert!(rx.try_recv().is_err());
    // The Archive opens folded; Space opens dd over ee.
    app.handle_key(key(KeyCode::Char(' ')));
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

/// [`app`] as the second of three workspaces of a host.
fn workspace_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app();
    app.set_workspaces(vec!["SITE / AA".into(), "NOTES".into(), String::new()], 1);
    (app, rx)
}

fn hover(app: &mut App, (column, row): (u16, u16)) -> bool {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn submenu(app: &App) -> Option<&Submenu> {
    app.context_menu().and_then(|menu| menu.submenu.as_ref())
}

#[test]
fn without_workspaces_the_menu_offers_no_move() {
    let (mut app, _rx) = app();
    let rows = screen(&mut app, 30);
    for name in ["CC", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let menu = app.context_menu().expect("the menu opened");
        assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
    }
    assert!(app.take_workspace_move().is_none());
}

#[test]
fn projects_and_agents_offer_a_move_before_removing() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 30);
    for name in ["CC", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let items = &app.context_menu().expect("the menu opened").items;
        let remove = items.iter().position(|&i| i == MenuItem::RemoveProject);
        let at = items.iter().position(|&i| i == MenuItem::MoveToWorkspace);
        assert_eq!(at.map(|at| at + 1), remove, "{name}: {items:?}");
    }
    let rows = screen(&mut app, 30);
    let item = rows
        .iter()
        .find(|row| row.contains("Move to workspace"))
        .expect("drawn");
    assert!(item.contains('›'), "{item}");

    // A project with every agent archived, too.
    app.process_command(UiCommand::SetProjects(vec![
        crate::events::ProjectSummary {
            id: "ee".into(),
            name: "empty".into(),
            ..Default::default()
        },
    ]));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "EMPTY"));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.target, MenuTarget::Project("ee".into()));
    assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
}

#[test]
fn the_archive_offers_no_move() {
    let (mut app, _rx) = archive_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "EE"));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.items, vec![MenuItem::Restore]);
}

#[test]
fn the_keys_move_an_agents_project_to_another_workspace() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    // Open, Archive, then Move to workspace.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert!(submenu(&app).is_none());
    app.handle_key(key(KeyCode::Right));
    let entries = &submenu(&app).expect("Right opens it").entries;
    assert_eq!(
        entries,
        &[
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(0)),
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(2)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ],
        "every workspace but this one, then a new one"
    );

    let rows = screen(&mut app, 30);
    for label in ["Workspace 1  SITE / AA", "Workspace 3", "New workspace"] {
        assert!(
            rows.iter().any(|row| row.contains(label)),
            "{label}:\n{}",
            rows.join("\n")
        );
    }
    assert!(!rows.iter().any(|row| row.contains("Workspace 2")));

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "aa".into(),
            to: WorkspaceTarget::Existing(2),
        }),
        "the agent's whole project moves"
    );
    assert!(app.take_workspace_move().is_none(), "once");
    assert!(rx.try_recv().is_err(), "nothing goes to the session");
}

#[test]
fn enter_opens_the_submenu_even_with_one_workspace() {
    let (mut app, mut rx) = app();
    app.set_workspaces(vec![String::new()], 0);
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter));
    let entries = &submenu(&app).expect("Enter opens it").entries;
    assert_eq!(entries, &[SubmenuEntry::Workspace(WorkspaceTarget::New)]);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "cc".into(),
            to: WorkspaceTarget::New,
        })
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn the_pointer_opens_the_submenu_and_picks_a_workspace() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    let rows = screen(&mut app, 30);
    let item = cell_of(&rows, "Move to workspace");
    assert!(hover(&mut app, item), "redraws");
    assert!(submenu(&app).is_some(), "hovering opens it");

    let rows = screen(&mut app, 30);
    let first = cell_of(&rows, "Workspace 1");
    assert_eq!(first.1, item.1, "its first entry is on the item's row");
    let new = cell_of(&rows, "New workspace");
    assert!(hover(&mut app, new), "the hover moves, so it redraws");
    assert_eq!(submenu(&app).map(|s| s.selected), Some(2));
    let buffer = draw(&mut app, 30);
    let lit = Some(crate::Theme::cyber().colors.surface_hover);
    assert_eq!(buffer[new].style().bg, lit);
    assert_ne!(buffer[first].style().bg, lit);

    // Another item closes it, and the item opens it again.
    let rows = screen(&mut app, 30);
    hover(&mut app, cell_of(&rows, "Project info"));
    assert!(submenu(&app).is_none());
    press(&mut app, MouseButton::Left, item);
    assert!(app.context_menu().is_some(), "a click on it runs nothing");
    assert!(submenu(&app).is_some(), "a click opens it");

    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Left, cell_of(&rows, "New workspace"));
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "cc".into(),
            to: WorkspaceTarget::New,
        })
    );
    assert!(rx.try_recv().is_err(), "nothing goes to the session");
}

#[test]
fn left_or_esc_closes_only_the_submenu() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    for close in [KeyCode::Left, KeyCode::Esc] {
        app.handle_key(key(KeyCode::Right));
        assert!(submenu(&app).is_some());
        app.handle_key(key(close));
        assert!(submenu(&app).is_none(), "{close:?}");
        let menu = app.context_menu().expect("the menu stays");
        assert_eq!(menu.items[menu.selected], MenuItem::MoveToWorkspace);
    }

    // A click outside both closes everything.
    app.handle_key(key(KeyCode::Right));
    screen(&mut app, 30);
    press(&mut app, MouseButton::Left, (110, 1));
    assert!(app.context_menu().is_none());
    assert!(app.take_workspace_move().is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn the_submenu_stays_on_screen_near_the_edges() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 12);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Right));
    screen(&mut app, 12);
    let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
    let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
    assert!(sub.bottom() <= 12, "{sub:?}");
    assert_eq!(sub.x, menu.right(), "right of the menu");

    // Opened near the right edge, it goes left of the menu.
    if let Some(open) = app.context_menu.as_mut() {
        open.anchor = Position::new(118, 2);
    }
    screen(&mut app, 30);
    let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
    let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
    assert!(menu.right() <= 120, "{menu:?}");
    assert_eq!(sub.right(), menu.x, "left of the menu: {sub:?} {menu:?}");
    assert!(sub.bottom() <= 30);
}

#[test]
fn new_workspaces_refresh_an_open_submenu() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::End));
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let open = submenu(&app).expect("still open");
    assert_eq!(
        open.entries,
        [
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ]
    );
    assert_eq!(open.selected, 1, "kept in range");

    // No workspaces any more: nothing to move to.
    app.set_workspaces(Vec::new(), 0);
    let menu = app.context_menu().expect("the menu stays");
    assert!(menu.submenu.is_none());
    assert!(!menu.items.contains(&MenuItem::MoveToWorkspace));
}

/// [`app`] with only the project of `aa` listed.
fn one_project_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app();
    app.process_command(UiCommand::SetAgents(vec![agent("aa", 0), agent("bb", 1)]));
    (app, rx)
}

/// A project, listed with every agent archived.
fn empty_project(id: &str) -> UiCommand {
    UiCommand::SetProjects(vec![crate::events::ProjectSummary {
        id: id.into(),
        name: id.into(),
        ..Default::default()
    }])
}

#[test]
fn the_only_project_listed_is_offered_no_new_workspace() {
    let (mut app, _rx) = one_project_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let open_submenu = |app: &mut App| {
        let rows = screen(app, 30);
        press(app, MouseButton::Right, cell_of(&rows, "BB"));
        // Open, Archive, Move to workspace, Remove project…
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Right));
        submenu(app).expect("Right opens it").entries.clone()
    };
    assert_eq!(
        open_submenu(&mut app),
        [SubmenuEntry::Workspace(WorkspaceTarget::Existing(1))]
    );
    let rows = screen(&mut app, 30);
    assert!(rows.iter().any(|row| row.contains("Workspace 2")));
    assert!(!rows.iter().any(|row| row.contains("New workspace")));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));

    // A project with every agent archived is listed too.
    app.process_command(empty_project("ee"));
    assert_eq!(
        open_submenu(&mut app),
        [
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ]
    );
}

#[test]
fn one_project_and_one_workspace_offer_no_move() {
    let (mut app, _rx) = one_project_app();
    app.set_workspaces(vec![String::new()], 0);
    let rows = screen(&mut app, 30);
    // The project's header, then its agents.
    for name in ["AA", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let menu = app.context_menu().expect("the menu opened");
        assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
        assert!(menu.items.contains(&MenuItem::RemoveProject), "{name}");
    }
    app.handle_key(key(KeyCode::Esc));

    // A second project gives the first one somewhere to go.
    app.process_command(empty_project("ee"));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    let menu = app.context_menu().expect("the menu opened");
    assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
}

/// Columns of a screen too narrow for the submenu on either side of
/// the menu, so it is drawn over it.
const NARROW: u16 = 40;

/// Open the submenu of agent BB's menu on a [`NARROW`] screen and draw
/// it. Returns the menu's and the submenu's areas, which overlap.
fn open_submenu_over_the_menu(app: &mut App) -> (Rect, Rect) {
    let rows = screen_sized(app, NARROW, 30);
    // BB's name is cut off this narrow; it is the row under its
    // parent AA, which has the fold mark.
    let (column, parent) = cell_of(&rows, "▾");
    press(app, MouseButton::Right, (column, parent + 1));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.target, MenuTarget::Agent("bb".into()));
    // Open, Archive, Move to workspace, Remove project…
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Right));
    let rows = screen_sized(app, NARROW, 30);
    let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
    let sub = submenu(app).and_then(|s| s.area).expect("drawn");
    assert!(
        sub.intersects(menu),
        "over the menu: {sub:?} {menu:?}\n{}",
        rows.join("\n")
    );
    (menu, sub)
}

/// The main item drawn under a cell the submenu covers.
fn item_under(app: &App, (column, row): (u16, u16)) -> Option<MenuItem> {
    let menu = app.context_menu()?;
    menu.items.get(menu.item_at(column, row)?).copied()
}

/// The menu is open on BB with its submenu, and nothing ran.
fn assert_still_open(app: &App, rx: &mut mpsc::Receiver<UiEvent>, what: &str) {
    let menu = app.context_menu().expect("the menu stays");
    assert_eq!(
        menu.items[menu.selected],
        MenuItem::MoveToWorkspace,
        "{what}"
    );
    assert!(menu.submenu.is_some(), "{what}: the submenu stays");
    assert!(rx.try_recv().is_err(), "{what}: nothing runs");
    assert_eq!(app.state(), AppState::Idle, "{what}");
    assert!(app.workspace_move.is_none(), "{what}");
}

#[test]
fn the_submenus_top_border_over_the_menu_runs_nothing() {
    let (mut app, mut rx) = workspace_app();
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    let border = (sub.x + 1, sub.y);
    assert_eq!(
        item_under(&app, border),
        Some(MenuItem::Archive),
        "the border hides the item above"
    );

    press(&mut app, MouseButton::Left, border);
    assert_still_open(&app, &mut rx, "click");
    hover(&mut app, border);
    assert_still_open(&app, &mut rx, "hover");
}

#[test]
fn the_submenus_bottom_border_over_the_menu_runs_nothing() {
    // One workspace to go to: the bottom border is on the next item.
    let (mut app, mut rx) = one_project_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    let border = (sub.x + 1, sub.bottom() - 1);
    assert_eq!(item_under(&app, border), Some(MenuItem::RemoveProject));

    hover(&mut app, border);
    assert_still_open(&app, &mut rx, "hover");
    press(&mut app, MouseButton::Left, border);
    assert_still_open(&app, &mut rx, "click");
}

#[test]
fn an_entry_over_the_menu_still_picks_its_workspace() {
    let (mut app, _rx) = workspace_app();
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    // The second entry sits on "Remove project…".
    let entry = (sub.x + 1, sub.y + 2);
    assert_eq!(item_under(&app, entry), Some(MenuItem::RemoveProject));
    hover(&mut app, entry);
    assert_eq!(submenu(&app).map(|s| s.selected), Some(1));
    press(&mut app, MouseButton::Left, entry);
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "aa".into(),
            to: WorkspaceTarget::Existing(2),
        })
    );
    assert_eq!(app.state(), AppState::Idle, "no removal asked");
}
