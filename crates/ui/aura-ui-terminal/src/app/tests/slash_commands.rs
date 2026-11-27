use super::*;

fn slash_names(app: &App) -> Vec<&'static str> {
    app.slash_matches().iter().map(|c| c.name).collect()
}

#[test]
fn test_slash_opens_menu_with_all_commands() {
    let mut app = App::new();
    assert!(app.slash_matches().is_empty());
    type_chars(&mut app, "/");
    assert_eq!(slash_names(&app).len(), slash_menu::SLASH_COMMANDS.len());
    assert_eq!(app.slash_cursor(), 0);
}

#[test]
fn test_slash_menu_filters_as_you_type() {
    let mut app = App::new();
    type_chars(&mut app, "/log");
    assert_eq!(slash_names(&app), ["log", "login", "logout"]);
    type_chars(&mut app, "o");
    assert_eq!(slash_names(&app), ["logout"]);
    type_chars(&mut app, "x");
    assert!(app.slash_matches().is_empty());
}

#[test]
fn test_slash_menu_lists_prefix_matches_first() {
    let mut app = App::new();
    type_chars(&mut app, "/a");
    let names = slash_names(&app);
    let mut prefixed = names[..3].to_vec();
    prefixed.sort_unstable();
    assert_eq!(prefixed, ["agent", "agents", "archive"], "{names:?}");
    assert!(names[3..].contains(&"clear"), "{names:?}");
}

#[test]
fn test_slash_menu_closes_after_space() {
    let mut app = App::new();
    type_chars(&mut app, "/model ");
    assert!(app.slash_matches().is_empty());
}

#[test]
fn test_slash_menu_arrows_move_and_wrap() {
    let mut app = App::new();
    type_chars(&mut app, "/log");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 1);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 2);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 0);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.slash_cursor(), 2);
    assert_eq!(app.input(), "/log");
}

#[test]
fn test_slash_menu_tab_completes_without_switching_panel() {
    let mut app = App::new();
    type_chars(&mut app, "/ker");
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.input(), "/kernel");
    assert_eq!(app.cursor_pos(), "/kernel".len());
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert!(app.record_panel_visible());
}

#[test]
fn test_slash_menu_enter_runs_selected_command() {
    let mut app = App::new();
    type_chars(&mut app, "/ker");
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.record_panel_visible());
    assert_eq!(app.input(), "");
    assert!(app.slash_matches().is_empty());
}

#[test]
fn test_slash_menu_enter_on_command_with_args_waits_for_them() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    type_chars(&mut app, "/ag");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.input(), "/agent ");
    assert!(app.slash_matches().is_empty());
    assert!(rx.try_recv().is_err());

    type_chars(&mut app, "new");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CreateAgent(None))));
}

#[test]
fn test_slash_menu_esc_dismisses_until_next_edit() {
    let mut app = App::new();
    type_chars(&mut app, "/m");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.slash_matches().is_empty());
    assert_eq!(app.input(), "/m");

    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Records);
    app.handle_key(key(KeyCode::Char('o')));
    assert_eq!(slash_names(&app), ["model", "mode", "remove-project"]);
}

fn agent_pick_names(app: &App) -> Vec<&str> {
    app.agent_picks().iter().map(|d| d.name.as_str()).collect()
}

#[test]
fn test_agent_space_lists_permanent_agents_and_enter_adds_one() {
    let (mut app, mut rx) = app_with_definitions();
    app.focus = PanelFocus::Chat;
    type_chars(&mut app, "/ag");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.input(), "/agent ");
    assert_eq!(agent_pick_names(&app), ["explorer", "reviewer"]);
    assert!(app.slash_matches().is_empty());

    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 1);
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::AddDefinitionToProject(name)) if name == "reviewer"
    ));
    assert_eq!(app.input(), "");
    assert!(app.agent_picks().is_empty());
    assert_eq!(app.agents_tab(), crate::app::AgentsTab::Projects);
}

#[test]
fn test_agent_picker_filters_and_tab_completes() {
    let (mut app, _rx) = app_with_definitions();
    app.focus = PanelFocus::Chat;
    type_chars(&mut app, "/agent E");
    assert_eq!(agent_pick_names(&app), ["explorer", "reviewer"]);
    type_chars(&mut app, "x");
    assert_eq!(agent_pick_names(&app), ["explorer"]);

    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.input(), "/agent explorer");
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert_eq!(agent_pick_names(&app), ["explorer"]);
}

#[test]
fn test_agent_picker_stays_out_of_agent_new_and_esc() {
    let (mut app, mut rx) = app_with_definitions();
    app.focus = PanelFocus::Chat;
    type_chars(&mut app, "/agent new");
    assert!(app.agent_picks().is_empty());
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CreateAgent(None))));

    type_chars(&mut app, "/agent rev");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.agent_picks().is_empty());
    app.handle_key(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
    assert_eq!(app.input(), "");
}

#[test]
fn test_agent_picker_adds_while_busy() {
    let (mut app, mut rx) = app_with_definitions();
    app.focus = PanelFocus::Chat;
    app.chat_state = AppState::Processing;
    type_chars(&mut app, "/agent ");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::AddDefinitionToProject(name)) if name == "explorer"
    ));
    assert_eq!(app.input(), "");
}

#[test]
fn test_recalled_slash_command_keeps_history_navigation() {
    let mut app = App::new();
    type_chars(&mut app, "/clear");
    app.handle_key(key(KeyCode::Enter));
    type_chars(&mut app, "hello");
    app.handle_key(key(KeyCode::Enter));
    app.chat_state = AppState::Idle;

    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "/clear");
    assert!(app.slash_matches().is_empty());
}
