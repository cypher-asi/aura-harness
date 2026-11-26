use super::*;

pub(super) fn agent(id: &str, depth: usize) -> AgentSummary {
    AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        depth,
        ..AgentSummary::default()
    }
}

pub(super) fn app() -> (App, mpsc::Receiver<UiEvent>) {
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
pub(super) fn draw(app: &mut App, height: u16) -> ratatui::buffer::Buffer {
    draw_sized(app, 120, height)
}

/// Draw the app `width` columns wide and `height` rows tall.
pub(super) fn draw_sized(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let theme = crate::Theme::cyber();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Draw the app 120 columns wide and `height` rows tall, a line per row.
pub(super) fn screen(app: &mut App, height: u16) -> Vec<String> {
    screen_sized(app, 120, height)
}

/// Draw the app `width` columns wide and `height` rows tall, a line
/// per row.
pub(super) fn screen_sized(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let buffer = draw_sized(app, width, height);
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

/// The cell of the first row whose text contains `name`.
pub(super) fn cell_of(rows: &[String], name: &str) -> (u16, u16) {
    let row = rows
        .iter()
        .position(|row| row.contains(name))
        .unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
    let line = &rows[row];
    let column = line[..line.find(name).unwrap_or(0)].chars().count();
    (u16::try_from(column).unwrap(), u16::try_from(row).unwrap())
}

pub(super) fn press(app: &mut App, button: MouseButton, (column, row): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(button),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The app with two archived agents, showing the Archive tab with the
/// first opened over the second, and on screen.
pub(super) fn archive_app() -> (App, mpsc::Receiver<UiEvent>) {
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

/// [`app`] as the second of three workspaces of a host.
pub(super) fn workspace_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app();
    app.set_workspaces(vec!["SITE / AA".into(), "NOTES".into(), String::new()], 1);
    (app, rx)
}

pub(super) fn hover(app: &mut App, (column, row): (u16, u16)) -> bool {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

pub(super) fn submenu(app: &App) -> Option<&Submenu> {
    app.context_menu().and_then(|menu| menu.submenu.as_ref())
}

/// [`app`] with only the project of `aa` listed.
pub(super) fn one_project_app() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app();
    app.process_command(UiCommand::SetAgents(vec![agent("aa", 0), agent("bb", 1)]));
    (app, rx)
}

/// A project, listed with every agent archived.
pub(super) fn empty_project(id: &str) -> UiCommand {
    UiCommand::SetProjects(vec![crate::events::ProjectSummary {
        id: id.into(),
        name: id.into(),
        ..Default::default()
    }])
}

/// Columns of a screen too narrow for the submenu on either side of
/// the menu, so it is drawn over it.
pub(super) const NARROW: u16 = 40;

/// Open the submenu of agent BB's menu on a [`NARROW`] screen and draw
/// it. Returns the menu's and the submenu's areas, which overlap.
pub(super) fn open_submenu_over_the_menu(app: &mut App) -> (Rect, Rect) {
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
pub(super) fn item_under(app: &App, (column, row): (u16, u16)) -> Option<MenuItem> {
    let menu = app.context_menu()?;
    menu.items.get(menu.item_at(column, row)?).copied()
}

/// The menu is open on BB with its submenu, and nothing ran.
pub(super) fn assert_still_open(app: &App, rx: &mut mpsc::Receiver<UiEvent>, what: &str) {
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
