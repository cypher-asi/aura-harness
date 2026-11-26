use super::*;

#[test]
fn test_ctrl_n_picks_a_folder_and_opens_it_as_a_project() {
    let (mut app, mut rx) = app_with_agents();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("website")).unwrap();
    open_folder_picker(&mut app, dir.path());

    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.folder_picker().unwrap().dir(),
        dir.path().join("website")
    );
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Enter));

    assert!(app.folder_picker().is_none());
    let expected = dir.path().join("website");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateProject { folder }) if folder == expected
    ));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_dropping_a_folder_creates_a_project_named_after_it() {
    let (mut app, mut rx) = app_with_agents();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("website");
    std::fs::create_dir(&folder).unwrap();

    assert!(app.drop_path(&folder));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateProject { folder: got }) if got == folder
    ));
}

#[test]
fn test_dropping_a_file_is_refused() {
    let (mut app, mut rx) = app_with_agents();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "").unwrap();

    assert!(!app.drop_path(&file));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_folder_picker_creates_a_folder_and_space_picks_it() {
    let (mut app, mut rx) = app_with_agents();
    let dir = tempfile::tempdir().unwrap();
    open_folder_picker(&mut app, dir.path());

    app.handle_key(key(KeyCode::Char('n')));
    type_chars(&mut app, "new-app");
    app.handle_key(key(KeyCode::Enter));
    assert!(dir.path().join("new-app").is_dir());
    assert_eq!(app.state(), AppState::SelectingFolder);

    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(app.state(), AppState::Idle);
    let expected = dir.path().join("new-app");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateProject { folder }) if folder == expected
    ));
}

#[test]
fn test_esc_closes_the_folder_picker() {
    let (mut app, mut rx) = app_with_agents();
    let dir = tempfile::tempdir().unwrap();
    open_folder_picker(&mut app, dir.path());

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.folder_picker().is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_projects_have_no_name_to_set() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/name Research");
    assert!(rx.try_recv().is_err());
    assert_ne!(app.state(), AppState::ShowingProjectInfo);
}

#[test]
fn test_alt_arrows_switch_agent_from_chat() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    // aa's chat was on screen before, so it is still in memory.
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "aa"));
}

#[test]
fn test_clicking_a_projects_plus_adds_an_agent_to_it() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_agent_areas();

    // Rows span columns 1..19; the "+" takes the last three. Fold the
    // second project (line 7) so adding to it opens it again.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 7));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 7));
    assert!(matches!(
        app.project_rows()[3],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));

    app.handle_mouse(mouse(MouseEventKind::Moved, 15, 7));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(3)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 16, 7));
    assert_eq!(app.hover(), Some(HoverTarget::AddToProject(3)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 13, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 17, 7));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::AddAgentToProject(id)) if id == "bb"));
    assert!(matches!(
        app.project_rows()[3],
        ProjectListRow::Project { expanded: true, .. }
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_agent_buttons_sit_left_of_the_time() {
    // ` ✕ 5m `: the glyph after a space, then the time with a space each
    // side.
    let at = |ago, button| agent_button_offset(18, &AgentRowEnd::time(ago), button);
    assert_eq!(at(Some("5m"), AgentButton::Archive), 13);
    assert_eq!(at(Some("now"), AgentButton::Archive), 12);
    // No time: the archive glyph one column in from the right edge.
    assert_eq!(at(None, AgentButton::Archive), 16);
    assert_eq!(
        agent_button_offset(1, &AgentRowEnd::time(Some("5m")), AgentButton::Archive),
        0
    );
}

#[test]
fn test_agent_archive_button_zone_spans_its_space_glyph_and_gap() {
    // Archive takes its space, glyph and the space before the edge
    // (15..=17); the name left of it is not a button.
    let expected = [
        (13, None),
        (14, None),
        (15, Some(AgentButton::Archive)),
        (16, Some(AgentButton::Archive)),
        (17, Some(AgentButton::Archive)),
    ];
    let no_time = AgentRowEnd::time(None);
    for (offset, button) in expected {
        assert_eq!(agent_button_at(18, &no_time, offset), button, "{offset}");
    }
    let five_minutes = AgentRowEnd::time(Some("5m"));
    assert_eq!(agent_button_at(18, &five_minutes, 11), None, "the name");
    for offset in 12..=14 {
        assert_eq!(
            agent_button_at(18, &five_minutes, offset),
            Some(AgentButton::Archive),
            "{offset}"
        );
    }
    assert_eq!(agent_button_at(18, &five_minutes, 15), None, "the time");
}

#[test]
fn test_agent_row_end_is_the_time_alone_even_for_coded_agents() {
    for ago in [Some("5m"), None] {
        for has_buttons in [true, false] {
            let end = AgentRowEnd::agent(ago);
            assert_eq!(end, AgentRowEnd::time(ago));
            assert_eq!(
                end.text(has_buttons),
                AgentRowEnd::time(ago).text(has_buttons)
            );
        }
    }
    assert_eq!(AgentRowEnd::time(Some("5m")).text(false), " 5m ");
    assert_eq!(AgentRowEnd::time(None).text(false), "");
    assert_eq!(AgentRowEnd::time(None).text(true), " ");
}

#[test]
fn test_an_agents_buttons_leave_room_for_its_time() {
    use crossterm::event::MouseEventKind;
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(vec![crate::events::AgentSummary {
        code: Some("REN36".to_string()),
        ..agent("aa", "A")
    }]));
    let mut app = with_agent_areas(app);

    // Rows span columns 1..19 and end in ` ✕ ` (no time, and no code even
    // though the agent has one): the archive glyph at column 1 + 16.
    let end = AgentRowEnd::agent(None);
    assert_eq!(agent_button_offset(18, &end, AgentButton::Archive), 16);
    app.handle_mouse(mouse(MouseEventKind::Moved, 15, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 17, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ArchiveAgent(1)));
}

#[test]
fn test_hovering_an_agents_buttons_targets_them() {
    use crossterm::event::MouseEventKind;
    let (mut app, _rx) = app_with_agent_areas();

    // Rows span columns 1..19. Agent `aa` (row 1, line 5) has no time, so
    // its archive glyph is at column 17 and that button takes columns
    // 16..=18; left of it is the row.
    for column in 13..=15 {
        app.handle_mouse(mouse(MouseEventKind::Moved, column, 5));
        assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)), "{column}");
    }
    for column in 16..=18 {
        app.handle_mouse(mouse(MouseEventKind::Moved, column, 5));
        assert_eq!(app.hover(), Some(HoverTarget::ArchiveAgent(1)), "{column}");
    }
    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));

    // A project's header keeps its "+" and gets no archive button.
    app.handle_mouse(mouse(MouseEventKind::Moved, 17, 4));
    assert_eq!(app.hover(), Some(HoverTarget::AddToProject(0)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 14, 4));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(0)));
}

#[test]
fn test_an_agents_buttons_follow_its_time() {
    use crossterm::event::MouseEventKind;
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    let three_minutes_ago = aura_core_types::time::now_ms() - 3 * 60 * 1000;
    app.process_command(UiCommand::SetAgents(vec![crate::events::AgentSummary {
        last_active_ms: Some(three_minutes_ago),
        ..agent("aa", "A")
    }]));
    let mut app = with_agent_areas(app);

    // ` ✕ 3m `: the archive glyph at column 1 + 13, the time in columns
    // 15..19.
    app.handle_mouse(mouse(MouseEventKind::Moved, 12, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 14, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ArchiveAgent(1)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 17, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));
}

#[test]
fn test_clicking_an_agents_archive_button_archives_it() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_agent_areas();
    let selected = app.selected_agent();
    assert_ne!(selected, 4);

    // Agent `bb` is row 4, line 8.
    app.handle_mouse(mouse(MouseEventKind::Moved, 17, 8));
    assert_eq!(app.hover(), Some(HoverTarget::ArchiveAgent(4)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 17, 8));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 17, 8));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    assert!(rx.try_recv().is_err(), "the click neither opens nor drags");
    assert_eq!(app.selected_agent(), selected);
    assert!(app.project_drag_hit().is_none());
}

/// Projects `notes`, with every agent archived, and `site`, holding `a1`.
/// Rows from line 4: notes 4; gap 5; site 6, a1 7.
fn app_with_an_empty_project() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new();
    app.event_tx = Some(tx);
    let project = |id: &str| crate::events::ProjectSummary {
        id: id.to_string(),
        name: id.to_string(),
        ..Default::default()
    };
    app.process_command(UiCommand::SetProjects(vec![
        project("notes"),
        project("site"),
    ]));
    app.process_command(UiCommand::SetAgents(vec![crate::events::AgentSummary {
        project_id: Some("site".to_string()),
        ..agent("a1", "A1")
    }]));
    (with_agent_areas(app), rx)
}

#[test]
fn test_a_project_with_every_agent_archived_keeps_its_header() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_an_empty_project();
    assert_eq!(
        app.project_rows(),
        [
            ProjectListRow::EmptyProject { project: 0 },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 0,
                expanded: true
            },
            ProjectListRow::Agent { index: 0 },
            ProjectListRow::NewProject,
        ]
    );

    // A click on it has nothing to fold; its "+" adds an agent to it.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
    assert!(rx.try_recv().is_err());
    app.handle_mouse(mouse(MouseEventKind::Moved, 17, 4));
    assert_eq!(app.hover(), Some(HoverTarget::AddToProject(0)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 17, 4));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::AddAgentToProject(id)) if id == "notes"));

    // Its menu has what applies to a project with no agents.
    assert!(app.open_context_menu(5, 4));
    let menu = app.context_menu().expect("open");
    assert_eq!(
        menu.target,
        super::super::context_menu::MenuTarget::Project("notes".to_string())
    );
    assert_eq!(
        menu.items,
        [
            super::super::context_menu::MenuItem::AddAgent,
            super::super::context_menu::MenuItem::Info,
            super::super::context_menu::MenuItem::RemoveProject,
        ]
    );

    // Removing it names it and waits for a yes.
    app.request_remove_project("notes".to_string());
    assert!(app
        .notification()
        .is_some_and(|(text, _)| text.contains("Remove project notes")));
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RemoveProject(id)) if id == "notes"));
}

#[test]
fn test_an_empty_project_keeps_its_place_while_others_are_dragged() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app = App::new();
    app.event_tx = Some(tx);
    let project = |id: &str| crate::events::ProjectSummary {
        id: id.to_string(),
        name: id.to_string(),
        ..Default::default()
    };
    let at = |project: &str, id: &str| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        ..agent(id, id)
    };
    app.process_command(UiCommand::SetProjects(vec![
        project("aa"),
        project("ee"),
        project("bb"),
    ]));
    app.process_command(UiCommand::SetAgents(vec![at("aa", "a1"), at("bb", "b1")]));
    let mut app = with_agent_areas(app);
    let headers = |app: &App| -> Vec<String> {
        app.project_rows()
            .into_iter()
            .filter_map(|row| app.header_key(row))
            .collect()
    };
    assert_eq!(headers(&app), ["aa", "ee", "bb"]);

    // Rows from line 4: aa 4, a1 5; gap 6; ee 7; gap 8; bb 9, b1 10.
    // bb dragged onto aa takes the first place; ee stays second.
    left(&mut app, MouseEventKind::Down, 9);
    left(&mut app, MouseEventKind::Drag, 4);
    assert_eq!(headers(&app), ["bb", "ee", "aa"]);
    left(&mut app, MouseEventKind::Up, 4);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::ReorderProjects(order)) if order == ["bb", "aa"]
    ));
}
