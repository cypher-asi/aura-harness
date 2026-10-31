use super::*;
use crate::components::{Message, MessageRole};
use crate::events::{ModelOptionView, UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
fn test_prefixed_input_runs_as_command() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "!echo hi");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::RunCommand(cmd)) if cmd == "echo hi"));
    assert_eq!(app.state(), AppState::Processing);
    assert!(app.messages().is_empty());
}

#[cfg(unix)]
#[test]
fn test_executable_on_path_runs_as_command() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "ls -la");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::RunCommand(cmd)) if cmd == "ls -la"));
}

#[test]
fn test_prose_is_sent_as_user_message() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "what is in here?");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage(text)) if text == "what is in here?"));
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

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
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
fn test_load_conversation_replaces_messages() {
    let mut app = App::new();
    app.add_message(Message::new(MessageRole::User, "old"));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::StartStreaming);
    app.scroll_offset = 7;

    app.process_command(UiCommand::LoadConversation(vec![
        crate::events::MessageData {
            role: MessageRole::User,
            content: "hi".to_string(),
            is_streaming: false,
            timestamp_ms: None,
        },
        crate::events::MessageData {
            role: MessageRole::Assistant,
            content: "hello".to_string(),
            is_streaming: false,
            timestamp_ms: None,
        },
    ]));

    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["hi", "hello"]);
    assert!(!app.is_thinking());
    assert!(app.messages().iter().all(|m| !m.is_streaming()));
    assert_eq!(app.scroll_offset(), 0);
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
            },
            ModelOptionView {
                id: "aura-claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
            },
        ],
        current: "local/qwen3:4b".to_string(),
        user: user.map(str::to_string),
    });
    (app, rx)
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
        options: app.models().to_vec(),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
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
        options: app.models().to_vec(),
        current: "local/qwen3:4b".to_string(),
        user: Some("alice".to_string()),
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
fn test_notification_types() {
    assert_eq!(NotificationType::Success, NotificationType::Success);
    assert_ne!(NotificationType::Success, NotificationType::Warning);
    assert_ne!(NotificationType::Warning, NotificationType::Error);
}
