use super::*;

#[test]
fn test_enter_on_definition_instantiates_it() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::InstantiateDefinition(name)) if name == "reviewer"
    ));
}

#[test]
fn test_enter_on_new_row_creates_definition() {
    let (mut app, mut rx) = app_with_definitions();

    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.selected_definition(), 2);
    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(rx.try_recv(), Ok(UiEvent::EditDefinition(None))));
}

#[test]
fn test_definition_shortcuts_edit_and_create() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_key(key(KeyCode::Char('e')));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::EditDefinition(Some(name))) if name == "explorer"
    ));

    app.handle_key(key(KeyCode::Char('n')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::EditDefinition(None))));
    assert!(app.input().is_empty());
}

#[test]
fn test_definition_shortcuts_type_when_input_not_empty() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_key(key(KeyCode::Char('h')));
    app.handle_key(key(KeyCode::Char('e')));

    assert!(rx.try_recv().is_err());
    assert_eq!(app.input(), "he");
}

#[test]
fn test_delete_definition_needs_confirmation() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_key(key(KeyCode::Char('d')));
    assert!(rx.try_recv().is_err());
    app.handle_key(key(KeyCode::Char('x')));
    assert!(rx.try_recv().is_err());

    app.handle_key(key(KeyCode::Char('d')));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::DeleteDefinition(name)) if name == "explorer"
    ));
}

#[test]
fn test_spawn_command_matches_name_case_insensitively() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_command("/spawn Reviewer");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::InstantiateDefinition(name)) if name == "reviewer"
    ));

    app.handle_command("/spawn nobody");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((_, NotificationType::Warning))
    ));
}

#[test]
fn test_spawn_allowed_while_processing() {
    let (mut app, mut rx) = app_with_definitions();
    app.state = AppState::Processing;

    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::InstantiateDefinition(_))
    ));
}

#[test]
fn test_open_editor_request_is_taken_once() {
    let (mut app, mut rx) = app_with_definitions();
    let path = std::path::PathBuf::from("/tmp/reviewer.md");

    app.process_command(UiCommand::OpenEditor(path.clone()));
    assert_eq!(app.take_editor_request(), Some(path.clone()));
    assert_eq!(app.take_editor_request(), None);

    app.editor_closed(path.clone(), &Ok(()));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::DefinitionEdited(p)) if p == path));
}

#[test]
fn test_arrows_toggle_agents_tabs_without_switching_agent() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);

    assert!(rx.try_recv().is_err());
}

#[test]
fn test_arrows_edit_input_instead_of_toggling_tabs() {
    let (mut app, _rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    app.handle_key(key(KeyCode::Char('a')));
    app.handle_key(key(KeyCode::Left));

    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.cursor_pos(), 0);
}

/// An instance of permanent agent `definition`, last active at `ms`.
fn instance(id: &str, definition: &str, ms: u64) -> crate::events::AgentSummary {
    crate::events::AgentSummary {
        definition: Some(definition.to_string()),
        last_active_ms: Some(ms),
        ..agent(id, &id.to_uppercase())
    }
}

#[test]
fn test_the_agents_app_shows_the_latest_instance_and_projects_gets_its_agent_back() {
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        instance("r1", "reviewer", 10),
        instance("r2", "reviewer", 20),
        instance("x1", "explorer", 5),
    ]));
    app.process_command(UiCommand::SetArchivedAgents(vec![instance(
        "x0", "explorer", 50,
    )]));
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        crate::events::AgentDefinitionSummary {
            name: "explorer".to_string(),
            ..Default::default()
        },
        crate::events::AgentDefinitionSummary {
            name: "reviewer".to_string(),
            ..Default::default()
        },
    ]));
    app.focus = PanelFocus::Swarm;

    // Coming to AGENTS shows the highlighted one's latest instance, an
    // archived one included.
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);
    assert_eq!(app.active_agent_id(), "x0");
    assert!(!app.app_empty());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "x0"));

    // Clicking another shows its latest.
    app.open_definition("reviewer");
    assert_eq!(app.active_agent_id(), "r2");

    // PROJECTS gets back the agent it was left on, AGENTS the one it showed.
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "aa");
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.active_agent_id(), "r2");
}

#[test]
fn test_the_agents_app_with_no_instance_shows_nothing_and_refuses_to_send() {
    let (mut app, mut rx) = app_with_definitions();
    assert!(rx.try_recv().is_err(), "nothing to show");
    assert_eq!(app.active_agent_id(), "aa");
    assert!(app.app_empty());
    assert!(!app.sandbox_panel_visible());
    assert_eq!(
        app.app_placeholder().as_deref(),
        Some("Pick a permanent agent")
    );
    app.input.text = "hello".to_string();
    app.input.cursor_pos = app.input.text.len();
    app.submit_input();
    assert_eq!(app.input(), "hello", "the draft is kept");
    assert!(rx.try_recv().is_err(), "nothing is sent");
}

#[test]
fn test_spawning_from_roster_shows_projects() {
    let (mut app, mut rx) = app_with_definitions();

    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::InstantiateDefinition(_))
    ));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
}

#[test]
fn test_clicking_tab_bar_switches_tabs() {
    let (mut app, _rx) = app_with_agents();
    app.areas.swarm = Some(Rect::new(0, 0, 24, 20));
    app.set_agent_tabs_area(Rect::new(0, 0, 24, 3));
    let start = |tab: AgentsTab| {
        AgentsTab::layout(24)
            .cells
            .into_iter()
            .find(|cell| cell.tab == tab)
            .unwrap()
            .start
    };

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        start(AgentsTab::Roster),
        0,
    ));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);
    assert_eq!(app.focus(), PanelFocus::Swarm);

    let divider = start(AgentsTab::Roster) - 1;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), divider, 1));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);

    let projects = start(AgentsTab::Projects);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), projects, 2));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);

    let channels = start(AgentsTab::Channels);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), channels, 1));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
}

#[test]
fn test_mouse_move_hovers_clickable_items() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, _rx) = app_with_agent_areas();
    app.set_agent_tabs_area(Rect::new(0, 0, 24, 3));

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 4));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(0)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 6));
    assert_eq!(app.project_rows()[2], ProjectListRow::Gap);
    assert_eq!(app.hover(), None);

    let projects = AgentsTab::layout(24)
        .cells
        .into_iter()
        .find(|cell| cell.tab == AgentsTab::Projects)
        .unwrap();
    app.handle_mouse(mouse(MouseEventKind::Moved, projects.start, 1));
    assert_eq!(
        app.hover(),
        Some(HoverTarget::AgentTab(AgentsTab::Projects))
    );

    app.handle_mouse(mouse(MouseEventKind::Moved, 0, 0));
    assert_eq!(app.hover(), None);

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 5));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 5));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 8, 5));
    assert_eq!(app.hover(), None);

    app.state = AppState::AwaitingApproval;
    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 5));
    assert_eq!(app.hover(), None);

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "").unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.set_sandbox_rows_area(Rect::new(81, 11, 28, 8), 0);
    app.handle_mouse(mouse(MouseEventKind::Moved, 85, 11));
    assert_eq!(app.hover(), Some(HoverTarget::SandboxRow(0)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 0, 0));
    assert_eq!(app.hover(), None);

    app.process_command(tool_data("t1", "read_file"));
    app.process_command(tool_data("t2", "read_file"));
    let ids: Vec<u64> = app.messages().iter().map(|m| m.version().0).collect();
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    app.set_chat_layout(step_rows(&ids), 0);
    app.handle_mouse(mouse(MouseEventKind::Moved, 10, 2));
    assert_eq!(
        app.hover(),
        Some(HoverTarget::Activity(ToggleTarget::Step(ids[1])))
    );
}

#[test]
fn test_hidden_tab_ignores_clicks_on_its_old_rows() {
    let (mut app, mut rx) = app_with_definitions();
    app.areas.swarm = Some(Rect::new(0, 0, 24, 20));
    app.set_agent_tabs_area(Rect::new(0, 0, 24, 3));
    app.set_agent_rows_area(Rect::new(1, 3, 22, 10));
    app.set_definition_rows_area(Rect::new(1, 3, 22, 10));

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 4));

    assert_eq!(app.selected_definition(), 1);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_pinned_new_agent_row_creates_agent() {
    let (mut app, mut rx) = app_with_definitions();
    app.areas.swarm = Some(Rect::new(0, 0, 24, 20));
    app.set_definition_rows_area(Rect::new(1, 3, 22, 10));
    app.set_new_agent_area(Rect::new(1, 13, 22, 1));

    // Below the two listed agents is empty list space, not "+ New agent".
    app.handle_mouse(mouse(MouseEventKind::Moved, 3, 5));
    assert_eq!(app.hover(), None);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 5));
    assert!(rx.try_recv().is_err());

    app.handle_mouse(mouse(MouseEventKind::Moved, 3, 13));
    assert_eq!(app.hover(), Some(HoverTarget::DefinitionRow(2)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 13));
    assert_eq!(app.selected_definition(), 2);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::EditDefinition(None))));
}

#[test]
fn test_refolding_project_keeps_active_agent_selected() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_agent_areas();
    let active = app.selected_agent();
    assert_eq!(
        app.project_rows()[active],
        ProjectListRow::Agent { index: 0 }
    );

    // The active agent's project is on line 4: close it, then open it.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));

    assert_eq!(app.selected_agent(), active);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_arrow_in_agents_column_switches_immediately() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    app.handle_key(key(KeyCode::Down));

    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn test_enter_on_new_agent_row_creates_agent() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    app.handle_key(key(KeyCode::Down));
    let _ = rx.try_recv();
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_agent(), 5);
    assert!(rx.try_recv().is_err());

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::SelectingFolder);
    assert!(rx.try_recv().is_err());
}
