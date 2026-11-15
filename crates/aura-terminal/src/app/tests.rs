use super::*;
use crate::components::{Activity, Message, MessageRole, ToolStatus};
use crate::events::{ActivityData, CallUsage, MessageData, ModelOptionView, UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

#[test]
fn test_app_creation() {
    let app = App::new();
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.messages().is_empty());
}

#[test]
fn test_add_message() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::User, "Hello"));
    assert_eq!(app.messages().len(), 1);
}

#[test]
fn test_input_handling() {
    let mut app = App::new();

    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
    assert_eq!(app.input(), "hi");
    assert_eq!(app.cursor_pos(), 2);

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
    assert_eq!(app.input(), "h");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_cursor_movement() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()));

    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
    assert_eq!(app.cursor_pos(), 0);

    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()));
    assert_eq!(app.cursor_pos(), 3);

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
    assert_eq!(app.cursor_pos(), 2);
}

#[test]
fn test_approval_state() {
    let mut app = App::new();
    app.pending_approval = Some(PendingApproval {
        id: "test".to_string(),
        tool: "fs.write".to_string(),
        description: "Write file".to_string(),
    });
    app.state = AppState::AwaitingApproval;

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()));
    assert!(app.pending_approval.is_none());
    assert_eq!(app.state(), AppState::Idle);
}

// ========================================================================
// State Transitions
// ========================================================================

#[test]
fn test_initial_state_is_idle() {
    let app = App::new();
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_state_transition_to_processing() {
    let mut app = App::new();
    app.state = AppState::Processing;
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn test_state_transition_to_awaiting_approval() {
    let mut app = App::new();
    app.pending_approval = Some(PendingApproval {
        id: "t1".to_string(),
        tool: "write_file".to_string(),
        description: "Write file".to_string(),
    });
    app.state = AppState::AwaitingApproval;
    assert_eq!(app.state(), AppState::AwaitingApproval);
    assert!(app.pending_approval.is_some());
}

#[test]
fn test_state_transition_showing_help() {
    let mut app = App::new();
    app.state = AppState::ShowingHelp;
    assert_eq!(app.state(), AppState::ShowingHelp);
}

#[test]
fn test_processing_to_idle() {
    let mut app = App::new();
    app.state = AppState::Processing;
    app.state = AppState::Idle;
    assert_eq!(app.state(), AppState::Idle);
}

// ========================================================================
// Input handling edge cases
// ========================================================================

#[test]
fn test_delete_on_empty_input() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
    assert_eq!(app.input(), "");
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_insert_at_cursor_position() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
    assert_eq!(app.input(), "abc");
}

#[test]
fn test_left_at_start_stays() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_right_at_end_stays() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
    assert_eq!(app.cursor_pos(), 1);
}

// ========================================================================
// Message handling
// ========================================================================

#[test]
fn test_add_many_messages_respects_limit() {
    let mut app = App::new();
    for i in 0..MAX_MESSAGES + 10 {
        app.add_message(Message::new(MessageRole::User, &format!("msg {i}")));
    }
    assert!(app.messages().len() <= MAX_MESSAGES);
}

#[test]
fn test_default_status_is_ready() {
    let app = App::new();
    assert_eq!(app.status(), "Ready");
}

#[test]
fn test_clear_command_emits_clear_event() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    app.handle_command("/clear");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::Clear)));
}

fn submit(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

#[test]
fn test_terminal_prompt_runs_input_as_command() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "$echo hi");

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::RunCommand { command, cols: 80 }) if command == "echo hi"
    ));
    assert_eq!(app.state(), AppState::Processing);
    let run = app.messages()[0].shell_run().expect("shell message");
    assert_eq!(run.command, "echo hi");
    assert_eq!(run.output, None);
    assert_eq!(app.working_label().as_deref(), Some("running echo"));
}

#[test]
fn test_command_output_fills_the_shell_message() {
    let mut app = App::new();
    submit(&mut app, "$false");

    app.process_command(UiCommand::CommandFinished {
        output: "nope\n".to_string(),
        exit_code: Some(1),
    });
    app.process_command(UiCommand::Complete);

    let run = app.messages()[0].shell_run().expect("shell message");
    assert_eq!(run.output.as_deref(), Some("nope\n"));
    assert_eq!(run.exit_code, Some(1));
    assert_eq!(app.working_label(), None);
}

#[test]
fn test_command_width_follows_the_chat() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);
    app.set_chat_width(118);

    submit(&mut app, "$ls");

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::RunCommand { cols: 118, .. })
    ));
}

#[test]
fn test_commands_without_hash_go_to_the_agent() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "ls -la");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage(text)) if text == "ls -la"));
}

#[test]
fn test_prose_is_sent_as_user_message() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "what is in here?");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage(text)) if text == "what is in here?"));
}

#[test]
fn test_dollar_on_empty_line_opens_terminal() {
    let mut app = App::new();
    app.handle_key(key(KeyCode::Char('$')));
    assert!(app.input_is_command());
    assert!(app.terminal_pane());
    assert_eq!(app.input(), "", "the `$` is not typed");

    app.handle_key(key(KeyCode::Char('$')));
    assert_eq!(app.input(), "$", "a second `$` is typed");
}

#[test]
fn test_dollar_mid_line_is_typed() {
    let mut app = App::new();
    for c in "costs $5".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.input(), "costs $5");
    assert!(!app.input_is_command());
}

#[test]
fn test_hash_no_longer_opens_terminal() {
    let mut app = App::new();
    app.handle_key(key(KeyCode::Char('#')));
    assert_eq!(app.input(), "#");
    assert!(!app.input_is_command());
}

#[test]
fn test_terminal_prompt_stays_open_after_a_command() {
    let mut app = App::new();
    submit(&mut app, "$true");
    app.process_command(UiCommand::CommandFinished {
        output: String::new(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    assert!(app.input_is_command());
}

#[test]
fn test_backspace_on_empty_line_or_esc_leaves_terminal_prompt() {
    let mut app = App::new();
    app.handle_key(key(KeyCode::Char('$')));
    app.handle_key(key(KeyCode::Char('l')));
    app.handle_key(key(KeyCode::Backspace));
    assert!(app.input_is_command(), "deleting text stays at the prompt");
    app.handle_key(key(KeyCode::Backspace));
    assert!(!app.input_is_command());

    app.handle_key(key(KeyCode::Char('$')));
    app.handle_key(key(KeyCode::Char('l')));
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.input_is_command());
    assert!(!app.terminal_pane(), "back to the chat pane");
    assert_eq!(app.input(), "l");
}

#[test]
fn test_slash_at_terminal_prompt_is_a_path_not_a_command() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);
    app.handle_key(key(KeyCode::Char('$')));
    for c in "/bin/ls".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert!(app.slash_matches().is_empty());
    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::RunCommand { command, .. }) if command == "/bin/ls"
    ));
}

#[test]
fn test_terminal_prompt_recalls_only_commands() {
    let mut app = App::new();
    submit(&mut app, "hello agent");
    app.state = AppState::Idle;
    submit(&mut app, "$ls");
    app.state = AppState::Idle;
    submit(&mut app, "pwd");
    app.state = AppState::Idle;

    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "pwd");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "ls");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "ls", "stays at the oldest command");

    app.handle_key(key(KeyCode::Esc));
    app.input.clear();
    app.cursor_pos = 0;
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "hello agent");
}

// ========================================================================
// Panel focus
// ========================================================================

#[test]
fn test_default_focus_is_chat() {
    let app = App::new();
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_panel_focus_equality() {
    assert_eq!(PanelFocus::Chat, PanelFocus::Chat);
    assert_ne!(PanelFocus::Chat, PanelFocus::Swarm);
    assert_ne!(PanelFocus::Chat, PanelFocus::Records);
}

// ========================================================================
// Agents column
// ========================================================================

fn agent(id: &str, name: &str) -> crate::events::AgentSummary {
    crate::events::AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        zns_id: format!("0://terminal/{id}"),
        is_active: false,
        record_count: 0,
        last_active: "-".to_string(),
        ..Default::default()
    }
}

fn app_with_agents() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        agent("bb", "Researcher"),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    (app, rx)
}

#[test]
fn test_agents_column_visible_by_default() {
    assert!(App::new().swarm_panel_visible());
}

#[test]
fn test_enter_in_agents_column_selects_agent() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    // Down off the first agent skips the next project's header to its agent.
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), 4);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn test_enter_on_active_agent_sends_nothing() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

    assert!(rx.try_recv().is_err());
}

#[test]
fn test_agent_switch_refused_while_processing() {
    let (mut app, mut rx) = app_with_agents();
    app.state = AppState::Processing;

    app.handle_command("/agent 2");

    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((_, NotificationType::Warning))
    ));
}

#[test]
fn test_agent_command_selects_by_number_and_name() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.handle_command("/agent researcher");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.handle_command("/agent nobody");
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_agent_new_command_emits_create_agent() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/agent new Code Reviewer");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateAgent(Some(name))) if name == "Code Reviewer"
    ));

    app.handle_command("/agent new");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CreateAgent(None))));
}

#[test]
fn test_project_folds_its_first_agent() {
    use super::ProjectListRow;

    let (mut app, mut rx) = app_with_agents();
    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: true,
            },
            ProjectListRow::Agent { index: 0 },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 1,
                expanded: true,
            },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::NewProject,
        ]
    );

    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(rx.try_recv().is_err());
    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: false,
            },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 1,
                expanded: true,
            },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::NewProject,
        ]
    );

    app.handle_key(key(KeyCode::Enter));
    assert!(app
        .project_rows()
        .iter()
        .any(|row| matches!(row, ProjectListRow::Agent { index: 0 })));
}

// ========================================================================
// Agents panel: roster tab
// ========================================================================

/// Agents panel focused on the roster tab.
fn app_with_definitions() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agents();
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        crate::events::AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code".to_string(),
        },
        crate::events::AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
        },
    ]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), crate::app::AgentsTab::Roster);
    (app, rx)
}

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
fn test_spawn_refused_while_processing() {
    let (mut app, mut rx) = app_with_definitions();
    app.state = AppState::Processing;

    app.handle_key(key(KeyCode::Enter));

    assert!(rx.try_recv().is_err());
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
    assert_eq!(app.agents_tab(), AgentsTab::Roster);

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
    app.set_agent_tabs_area(Rect::new(1, 1, 22, 1));
    let (_, roster_start, _) = AgentsTab::layout()
        .find(|&(tab, _, _)| tab == AgentsTab::Roster)
        .unwrap();

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        1 + roster_start,
        1,
    ));
    assert_eq!(app.agents_tab(), AgentsTab::Roster);
    assert_eq!(app.focus(), PanelFocus::Swarm);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
}

#[test]
fn test_mouse_move_hovers_clickable_items() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, _rx) = app_with_agent_areas();
    app.set_agent_tabs_area(Rect::new(1, 1, 22, 1));

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 4));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(0)));
    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 5));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(1)));

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 6));
    assert_eq!(app.project_rows()[2], ProjectListRow::Gap);
    assert_eq!(app.hover(), None);

    app.handle_mouse(mouse(MouseEventKind::Moved, 2, 1));
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
    app.set_agent_tabs_area(Rect::new(1, 1, 22, 1));
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
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));

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

/// Open the folder browser with Ctrl+N, pointed at `dir` instead of the
/// launch directory.
fn open_folder_picker(app: &mut App, dir: &std::path::Path) {
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(app.state(), AppState::SelectingFolder);
    app.folder_picker = Some(FolderPicker::open(dir.to_path_buf()).unwrap());
}

#[test]
fn test_ctrl_n_picks_a_folder_then_names_the_project() {
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

    assert_eq!(app.state(), AppState::ProjectName);
    assert!(app.folder_picker().is_none());
    assert_eq!(app.input(), "website");
    assert!(rx.try_recv().is_err());

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_chars(&mut app, "Website");
    app.handle_key(key(KeyCode::Enter));
    let expected = dir.path().join("website");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateProject { name, folder }) if name == "Website" && folder == expected
    ));
    assert_eq!(app.state(), AppState::Idle);
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
    assert_eq!(app.state(), AppState::ProjectName);
    assert_eq!(app.input(), "new-app");
    app.handle_key(key(KeyCode::Enter));
    let expected = dir.path().join("new-app");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateProject { folder, .. }) if folder == expected
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
fn test_name_command_renames_the_current_project() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/name Research");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RenameProject(name)) if name == "Research"));

    app.handle_command("/name");
    assert_eq!(app.state(), AppState::ProjectName);
    type_chars(&mut app, "Notes");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RenameProject(name)) if name == "Notes"));
}

#[test]
fn test_alt_arrows_switch_agent_from_chat() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "aa"));
}

fn mouse(kind: crossterm::event::MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn app_with_agent_areas() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agents();
    let swarm = Rect::new(0, 3, 20, 13);
    app.set_panel_areas(crate::app::PanelAreas {
        swarm: Some(swarm),
        agent_rows: None,
        chat: Some(Rect::new(20, 3, 60, 13)),
        ..crate::app::PanelAreas::default()
    });
    app.set_agent_rows_area(Rect::new(1, 4, 18, 10));
    app.set_new_project_area(Rect::new(1, 14, 18, 1));
    (app, rx)
}

#[test]
fn test_click_sandbox_row_selects_and_toggles_dir() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("notes.md"), "").unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.set_panel_areas(crate::app::PanelAreas {
        sandbox: Some(Rect::new(80, 10, 30, 10)),
        ..crate::app::PanelAreas::default()
    });
    app.set_sandbox_rows_area(Rect::new(81, 11, 28, 8), 0);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 11));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    let names: Vec<&str> = app
        .sandbox()
        .rows()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, ["src", "lib.rs", "notes.md"]);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 13));
    assert_eq!(app.sandbox().selected(), 2);
    assert_eq!(app.sandbox().rows().len(), 3);

    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 85, 15));
    assert_eq!(app.sandbox().selected(), 0);
}

#[test]
fn test_click_agent_row_switches_and_new_row_creates() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_agent_areas();

    // Projects take lines 4 and 7, agents 5 and 8, the gap 6.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 8));
    assert_eq!(app.focus(), PanelFocus::Swarm);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 6));
    assert_eq!(app.selected_agent(), 4);

    // Clicking a project folds it and leaves the highlighted agent alone.
    let bb = ProjectListRow::Agent { index: 1 };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert_eq!(app.project_rows()[app.selected_agent()], bb);
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(0)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    assert_eq!(app.selected_agent(), 4);
    assert_eq!(app.project_rows()[4], bb);
    assert!(rx.try_recv().is_err());

    // "+ New project" is pinned to the bottom, not drawn after the last agent.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 13));
    assert_eq!(app.selected_agent(), 4);
    assert_ne!(app.state(), AppState::SelectingFolder);
    assert!(rx.try_recv().is_err());

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 14));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(5)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 14));
    assert_eq!(app.state(), AppState::SelectingFolder);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_drag_selects_chat_text_and_requests_copy() {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{buffer::Buffer, style::Style};
    let (mut app, _rx) = app_with_agent_areas();
    app.set_chat_text_area(Rect::new(23, 4, 54, 8));
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 14));
    buffer.set_string(23, 4, "[12:00:00] <AURA> hello", Style::default());
    buffer.set_string(23, 5, "             second line", Style::default());

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 34, 4));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 200, 5));
    assert!(app.take_copy_text(&buffer).is_none());
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 200, 5));

    assert_eq!(
        app.take_copy_text(&buffer).as_deref(),
        Some("<AURA> hello\n             second line")
    );
    assert!(app.take_copy_text(&buffer).is_none());
    assert!(app.request_copy());

    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.selection().is_none());
    assert!(!app.request_copy());
}

#[test]
fn test_copy_notice_sits_then_expires() {
    let mut app = App::new();
    app.notify_copied(4, &Ok(()));
    assert_eq!(
        app.copy_notice(),
        Some(("Copied 4 characters", NotificationType::Success))
    );

    app.tick();
    assert!(app.copy_notice().is_some());

    let notice = app.copy_notice.as_mut().unwrap();
    notice.2 = std::time::Instant::now() - std::time::Duration::from_millis(1);
    app.tick();
    assert!(app.copy_notice().is_none());
}

#[test]
fn test_keypress_clears_copy_notice() {
    let mut app = App::new();
    app.notify_copied(1, &Ok(()));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
    assert!(app.copy_notice().is_none());
}

#[test]
fn test_click_without_drag_selects_nothing() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, _rx) = app_with_agent_areas();

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));

    assert!(app.selection().is_none());
    assert!(!app.request_copy());
}

#[test]
fn test_wheel_over_agents_scrolls_without_switching() {
    use crossterm::event::MouseEventKind;
    let (mut app, mut rx) = app_with_agent_areas();
    // Two lines of a five-line list: project, agent, gap, project, agent.
    app.set_agent_rows_area(Rect::new(1, 4, 18, 2));
    let selected = app.selected_agent();

    for _ in 0..5 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    }
    assert_eq!(app.agents_scroll(), 3);
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);
    assert_eq!(app.selected_agent(), selected);
    assert!(rx.try_recv().is_err());
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert_eq!(app.scroll_offset(), 0);

    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 5));
    assert_eq!(app.agents_scroll(), 2);

    // A key brings the highlight back into view. Up from the first agent
    // has no agent to reach, so the highlight stays where it is.
    app.focus = PanelFocus::Swarm;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), selected);
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 1);
}

#[test]
fn test_agents_scroll_keeps_selection_visible() {
    // Rows: project 0, agent 1, gap 2, project 3, agent 4.
    let (mut app, _rx) = app_with_agents();
    app.selected_agent = 1;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 0);

    app.selected_agent = 4;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);

    app.selected_agent = 3;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);

    app.selected_agent = 0;
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 0);
}

#[test]
fn test_follow_selection_moves_only_to_keep_selection_visible() {
    use crate::app::follow_selection;
    // Moving within the view leaves it alone.
    assert_eq!(follow_selection(0, 4, 5, 20), 0);
    // Stepping past the bottom scrolls one row, not half a page.
    assert_eq!(follow_selection(0, 5, 5, 20), 1);
    // Stepping back up inside the view keeps it where it is.
    assert_eq!(follow_selection(6, 7, 5, 20), 6);
    assert_eq!(follow_selection(6, 5, 5, 20), 5);
    // Never scrolls past the end of the list.
    assert_eq!(follow_selection(18, 19, 5, 20), 15);
    assert_eq!(follow_selection(3, 1, 5, 2), 0);
}

#[test]
fn test_chat_scroll_holds_position_as_lines_arrive() {
    let mut app = App::new();
    assert_eq!(app.sync_chat_scroll(100, 20, true), 80);

    app.scroll_up(10);
    assert_eq!(app.sync_chat_scroll(100, 20, false), 70);
    // 15 lines stream in below: the same lines stay on screen.
    assert_eq!(app.sync_chat_scroll(115, 20, false), 70);
    assert_eq!(app.scroll_offset(), 25);
    // A reflow moves every line, so the offset is only clamped.
    assert_eq!(app.sync_chat_scroll(60, 20, true), 15);

    app.scroll_chat_to_bottom();
    assert_eq!(app.sync_chat_scroll(70, 20, false), 50);
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_chat_page_keys_scroll_a_screenful() {
    let mut app = App::new();
    app.set_chat_text_area(Rect::new(0, 0, 60, 22));
    app.sync_chat_scroll(200, 22, true);

    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.scroll_offset(), 20);
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
    app.sync_chat_scroll(200, 22, false);
    assert_eq!(app.scroll_offset(), 178);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_incoming_messages_keep_scroll_but_sending_jumps_to_bottom() {
    let mut app = App::new();
    app.scroll_offset = 7;
    app.add_message(Message::new(MessageRole::System, "tool output"));
    assert_eq!(app.scroll_offset(), 7);

    app.input = "hello".to_string();
    app.submit_input();
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_load_conversation_replaces_messages() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::User, "old"));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::StartStreaming);
    app.scroll_offset = 7;

    app.process_command(UiCommand::LoadConversation(vec![
        MessageData::new(MessageRole::User, "hi"),
        MessageData {
            activity: Some(ActivityData::Thinking {
                text: "hmm".to_string(),
                duration_ms: Some(1200),
            }),
            ..MessageData::new(MessageRole::Assistant, "")
        },
        MessageData {
            activity: Some(ActivityData::Tool {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                args: r#"{"path":"a.rs"}"#.to_string(),
                result: Some("fn main() {}".to_string()),
                is_error: false,
                duration_ms: Some(5),
                policy: None,
            }),
            ..MessageData::new(MessageRole::Assistant, "")
        },
        MessageData::new(MessageRole::Assistant, "hello"),
        MessageData {
            command: Some("ls -la".to_string()),
            ..MessageData::new(MessageRole::User, "ls -la")
        },
    ]));

    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["hi", "", "", "hello", "ls -la"]);
    assert!(matches!(
        app.messages()[1].activity_step(),
        Some(Activity::Thinking(step)) if step.text == "hmm" && !step.running
    ));
    assert!(matches!(
        app.messages()[2].activity_step(),
        Some(Activity::Tool(tool)) if tool.status == ToolStatus::Success
    ));
    let run = app.messages()[4].shell_run().expect("shell message");
    assert_eq!(run.command, "ls -la");
    assert!(!app.messages()[4].is_running_shell());
    assert!(!app.has_live_activity());
    assert!(app.messages().iter().all(|m| !m.is_streaming()));
    assert_eq!(app.scroll_offset(), 0);
}

#[test]
fn test_reply_model_names_only_replies_since_last_user_message() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::Assistant, "earlier reply"));
    app.add_message(Message::new(MessageRole::User, "question"));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::FinishThinking);
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("first part".to_string()));
    app.process_command(UiCommand::FinishStreaming);
    app.process_command(UiCommand::SetReplyModel {
        model: "qwen3:4b".to_string(),
        usage: Some(CallUsage {
            tokens: 900,
            cost_usd: None,
        }),
    });
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("second part".to_string()));
    let opus = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.042),
    };
    app.process_command(UiCommand::SetReplyModel {
        model: "claude-opus-4-7".to_string(),
        usage: Some(opus),
    });

    let models: Vec<(&str, Option<&str>, Option<u64>)> = app
        .messages()
        .iter()
        .map(|m| (m.content(), m.model(), m.usage().map(|u| u.tokens)))
        .collect();
    assert_eq!(
        models,
        [
            ("earlier reply", None, None),
            ("question", None, None),
            ("", None, None),
            ("first part", Some("qwen3:4b"), Some(900)),
            ("second part", Some("claude-opus-4-7"), Some(12_400)),
        ]
    );
    assert_eq!(app.messages().back().unwrap().usage(), Some(opus));
}

// ========================================================================
// Activity blocks
// ========================================================================

fn tool_data(id: &str, name: &str) -> UiCommand {
    UiCommand::ShowTool(crate::events::ToolData {
        id: id.to_string(),
        name: name.to_string(),
        args: String::new(),
    })
}

fn activities(app: &App) -> Vec<&Activity> {
    app.messages()
        .iter()
        .filter_map(Message::activity_step)
        .collect()
}

#[test]
fn test_thinking_streams_into_one_block() {
    let mut app = App::new();
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Let me ".to_string()));
    app.process_command(UiCommand::AppendThinking("look.".to_string()));
    assert!(app.has_live_activity());

    app.process_command(UiCommand::FinishThinking);
    let [Activity::Thinking(step)] = activities(&app)[..] else {
        panic!("expected one thinking block");
    };
    assert_eq!(step.text, "Let me look.");
    assert!(!step.running);
    assert!(step.duration_ms.is_some());
}

#[test]
fn test_tool_block_updates_in_place() {
    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    app.process_command(tool_data("t2", "run_command"));
    app.process_command(UiCommand::UpdateToolArgs {
        id: "t1".to_string(),
        args: r#"{"path":"a.rs"}"#.to_string(),
    });
    app.process_command(UiCommand::ToolRetrying {
        id: "t2".to_string(),
        attempt: 2,
        max_attempts: 3,
    });
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "fn main() {}".to_string(),
        success: true,
    });

    let [Activity::Tool(first), Activity::Tool(second)] = activities(&app)[..] else {
        panic!("expected two tool blocks");
    };
    assert_eq!(first.args, r#"{"path":"a.rs"}"#);
    assert_eq!(first.status, ToolStatus::Success);
    assert_eq!(first.result.as_deref(), Some("fn main() {}"));
    assert_eq!(second.status, ToolStatus::Running);
    assert_eq!(second.retry, Some((2, 3)));
    assert_eq!(app.messages().len(), 2);
}

#[test]
fn test_complete_ends_running_activity() {
    let mut app = App::new();
    app.process_command(UiCommand::StartThinking);
    app.process_command(tool_data("t1", "run_command"));
    app.process_command(UiCommand::Complete);

    assert!(!app.has_live_activity());
    let [_, Activity::Tool(tool)] = activities(&app)[..] else {
        panic!("expected thinking then a tool");
    };
    assert_eq!(tool.status, ToolStatus::Error);
}

#[test]
fn test_ctrl_o_toggles_every_block_and_new_ones_follow() {
    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    assert!(!app.messages()[0].is_expanded());

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.messages()[0].is_expanded());
    app.process_command(UiCommand::StartThinking);
    assert!(app.messages()[1].is_expanded());

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.messages().iter().all(|m| !m.is_expanded()));
    assert!(app.input().is_empty());
}

#[test]
fn test_clicking_a_block_header_toggles_only_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    app.process_command(tool_data("t2", "read_file"));
    let ids: Vec<u64> = app.messages().iter().map(|m| m.version().0).collect();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    app.set_chat_layout(step_rows(&ids), 0);

    let click = |app: &mut App, kind| {
        app.handle_mouse(MouseEvent {
            kind,
            column: 10,
            row: 2,
            modifiers: KeyModifiers::empty(),
        });
    };
    click(&mut app, MouseEventKind::Down(MouseButton::Left));
    click(&mut app, MouseEventKind::Up(MouseButton::Left));

    assert!(!app.messages()[0].is_expanded());
    assert!(app.messages()[1].is_expanded());
}

#[test]
fn test_clicking_a_group_summary_flips_what_it_shows() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    let click = |app: &mut App| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: 10,
                row: 1,
                modifiers: KeyModifiers::empty(),
            });
        }
    };
    let group = |open| ToggleRow {
        line: 0,
        target: ToggleTarget::Group(7),
        open,
    };

    app.set_chat_layout(vec![group(false)], 0);
    click(&mut app);
    assert_eq!(app.group_open.get(&7), Some(&true));

    app.set_chat_layout(vec![group(true)], 0);
    click(&mut app);
    assert_eq!(app.group_open.get(&7), Some(&false));

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.group_open.is_empty());
}

/// One collapsed step row per message id, on consecutive lines.
fn step_rows(ids: &[u64]) -> Vec<ToggleRow> {
    ids.iter()
        .enumerate()
        .map(|(line, &id)| ToggleRow {
            line,
            target: ToggleTarget::Step(id),
            open: false,
        })
        .collect()
}

// ========================================================================
// Notification types
// ========================================================================

// ========================================================================
// Model selector
// ========================================================================

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn app_with_models(user: Option<&str>) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetModels {
        options: vec![
            ModelOptionView {
                id: "local/qwen3:4b".to_string(),
                label: "qwen3:4b".to_string(),
                local: true,
                installed: true,
                group: "Local (no login)".to_string(),
                provider: None,
                available: true,
            },
            ModelOptionView {
                id: "aura-claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Cloud (aura-router)".to_string(),
                provider: Some("zos".to_string()),
                available: user.is_some(),
            },
            ModelOptionView {
                id: "claude/claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Claude plan (direct)".to_string(),
                provider: Some("claude".to_string()),
                available: false,
            },
        ],
        current: "local/qwen3:4b".to_string(),
        user: user.map(str::to_string),
        plans: Vec::new(),
    });
    (app, rx)
}

fn available(models: &[ModelOptionView]) -> Vec<ModelOptionView> {
    models
        .iter()
        .cloned()
        .map(|m| ModelOptionView {
            available: true,
            ..m
        })
        .collect()
}

#[test]
fn test_unlinked_plan_pick_starts_link_then_selects() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model claude/claude-sonnet-4-6");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::LinkSubscription { provider, paste: false }) if provider == "claude"
    ));

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
        plans: Vec::new(),
    });
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "claude/claude-sonnet-4-6"
    ));
}

#[test]
fn test_failed_link_drops_pending_model() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model claude/claude-sonnet-4-6");
    let _ = rx.try_recv();
    app.process_command(UiCommand::PromptLinkCode {
        provider: "Claude".to_string(),
    });
    assert_eq!(app.state(), AppState::LinkCode);
    app.process_command(UiCommand::EndLink);
    assert_eq!(app.state(), AppState::Idle);

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
        plans: Vec::new(),
    });
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_login_and_logout_take_a_provider() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/login chatgpt paste");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::LinkSubscription { provider, paste: true }) if provider == "chatgpt"
    ));
    app.handle_command("/logout anthropic");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Unlink(p)) if p == "claude"));
    app.handle_command("/logout");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Logout)));
    app.handle_command("/login");
    assert_eq!(app.state(), AppState::LoginEmail);
}

#[test]
fn test_model_command_opens_selector_on_current() {
    let (mut app, _rx) = app_with_models(None);
    app.handle_command("/model");
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(app.model_cursor(), 0);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_selector_enter_emits_select_model() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model");
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::Idle);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "aura-claude-sonnet-4-6"
    ));
}

#[test]
fn test_guest_cloud_pick_starts_login_then_selects() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model");
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::LoginEmail);
    assert!(rx.try_recv().is_err());

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
        plans: Vec::new(),
    });
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "aura-claude-sonnet-4-6"
    ));
}

#[test]
fn test_cancelled_login_drops_pending_model() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model Claude Sonnet 4.6");
    assert_eq!(app.state(), AppState::LoginEmail);
    app.handle_key(key(KeyCode::Esc));

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
        plans: Vec::new(),
    });
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_model_command_passes_unlisted_id_through() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model local/qwen3:8b");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "local/qwen3:8b"
    ));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_panel_commands_toggle_visibility() {
    let mut app = App::new();
    assert!(app.record_panel_visible());
    assert!(app.sandbox_panel_visible());
    assert!(app.swarm_panel_visible());

    app.handle_command("/kernel");
    app.handle_command("/files");
    app.handle_command("/agents");
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert!(!app.swarm_panel_visible());

    app.handle_command("/record");
    app.handle_command("/files");
    app.handle_command("/agents");
    assert!(app.record_panel_visible());
    assert!(app.sandbox_panel_visible());
    assert!(app.swarm_panel_visible());
}

#[test]
fn test_agents_command_refreshes_when_opened() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.handle_command("/agents");
    assert!(rx.try_recv().is_err());
    app.handle_command("/agents");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RefreshAgents)));
}

#[test]
fn test_closing_focused_sandbox_returns_focus_to_chat() {
    let mut app = App::new();
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    app.handle_command("/files");
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_tab_cycles_visible_panels() {
    let mut app = App::new();
    let mut order = Vec::new();
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Tab));
        order.push(app.focus());
    }
    assert_eq!(
        order,
        [
            PanelFocus::Records,
            PanelFocus::Sandbox,
            PanelFocus::Swarm,
            PanelFocus::Chat
        ]
    );

    app.handle_command("/record");
    app.handle_command("/agents");
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_sandbox_keys_navigate_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("notes.md"), "").unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    app.handle_key(key(KeyCode::Enter));
    let names: Vec<&str> = app
        .sandbox()
        .rows()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, ["src", "lib.rs", "notes.md"]);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.sandbox().selected(), 0);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.sandbox().rows().len(), 2);
}

#[test]
fn test_arrows_move_input_cursor_when_panel_focused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    app.handle_key(key(KeyCode::Char('a')));
    app.handle_key(key(KeyCode::Char('b')));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.cursor_pos(), 1);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.cursor_pos(), 2);
    assert_eq!(app.sandbox().rows().len(), 1);

    for focus in [PanelFocus::Records, PanelFocus::Swarm] {
        app.focus = focus;
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.cursor_pos(), 1);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cursor_pos(), 2);
    }
}

#[test]
fn test_sandbox_refreshes_after_tool_completes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    assert!(app.sandbox().rows().is_empty());

    std::fs::write(dir.path().join("out.txt"), "").unwrap();
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: String::new(),
        success: true,
    });
    assert_eq!(app.sandbox().rows().len(), 1);
}

#[test]
fn test_notification_types() {
    assert_eq!(NotificationType::Success, NotificationType::Success);
    assert_ne!(NotificationType::Success, NotificationType::Warning);
    assert_ne!(NotificationType::Warning, NotificationType::Error);
}

// ========================================================================
// Slash menu
// ========================================================================

fn type_chars(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

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
    type_chars(&mut app, "/lo");
    assert_eq!(slash_names(&app), ["login", "logout"]);
    type_chars(&mut app, "go");
    assert_eq!(slash_names(&app), ["logout"]);
    type_chars(&mut app, "x");
    assert!(app.slash_matches().is_empty());
}

#[test]
fn test_slash_menu_lists_prefix_matches_first() {
    let mut app = App::new();
    type_chars(&mut app, "/a");
    let names = slash_names(&app);
    assert_eq!(names[..2], ["agent", "agents"]);
    assert!(names[2..].contains(&"clear"), "{names:?}");
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
    type_chars(&mut app, "/lo");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 1);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.slash_cursor(), 0);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.slash_cursor(), 1);
    assert_eq!(app.input(), "/lo");
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
    assert_eq!(slash_names(&app), ["model"]);
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
fn test_agent_picker_keeps_input_while_busy() {
    let (mut app, mut rx) = app_with_definitions();
    app.focus = PanelFocus::Chat;
    app.state = AppState::Processing;
    type_chars(&mut app, "/agent ");
    app.handle_key(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
    assert_eq!(app.input(), "/agent ");
}

#[test]
fn test_recalled_slash_command_keeps_history_navigation() {
    let mut app = App::new();
    type_chars(&mut app, "/clear");
    app.handle_key(key(KeyCode::Enter));
    type_chars(&mut app, "hello");
    app.handle_key(key(KeyCode::Enter));
    app.state = AppState::Idle;

    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "/clear");
    assert!(app.slash_matches().is_empty());
}

fn new_record(app: &mut App, seq: u64) {
    app.process_command(UiCommand::NewRecord(crate::events::RecordSummary {
        seq,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "Prompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: crate::events::RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    }));
}

fn selected_seq(app: &App) -> u64 {
    app.selected_record_data().unwrap().seq
}

#[test]
fn test_records_list_oldest_first_and_follow_newest() {
    let mut app = App::new();
    for seq in 0..3 {
        new_record(&mut app, seq);
    }
    let order: Vec<u64> = app.records().iter().map(|r| r.seq).collect();
    assert_eq!(order, [0, 1, 2]);
    assert_eq!(selected_seq(&app), 2);

    new_record(&mut app, 3);
    assert_eq!(selected_seq(&app), 3);
}

#[test]
fn test_records_selection_moved_up_stays_put() {
    let mut app = App::new();
    for seq in 0..3 {
        new_record(&mut app, seq);
    }
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Up));
    new_record(&mut app, 3);
    assert_eq!(selected_seq(&app), 1);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    new_record(&mut app, 4);
    assert_eq!(selected_seq(&app), 4);
}

#[test]
fn test_records_cap_drops_oldest_and_keeps_selection() {
    let mut app = App::new();
    let cap = u64::try_from(MAX_RECORDS).unwrap();
    for seq in 0..cap {
        new_record(&mut app, seq);
    }
    app.selected_record = 10;
    new_record(&mut app, cap);
    assert_eq!(app.records().len(), MAX_RECORDS);
    assert_eq!(app.records().front().unwrap().seq, 1);
    assert_eq!(app.records().back().unwrap().seq, cap);
    assert_eq!(selected_seq(&app), 10);
}
