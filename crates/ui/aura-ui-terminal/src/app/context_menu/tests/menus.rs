use super::*;

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
