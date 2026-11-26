use super::*;
use crate::components::{Activity, Message, MessageRole, ToolStatus, ToolStep};
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
    app.pending_approvals.push_back(PendingApproval {
        id: "test".to_string(),
        tool: "fs.write".to_string(),
        description: "Write file".to_string(),
        always: String::new(),
    });
    app.state = AppState::AwaitingApproval;

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()));
    assert!(app.pending_approvals.is_empty());
    assert_eq!(
        app.state(),
        AppState::Processing,
        "the turn goes on with the denial"
    );
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
    app.pending_approvals.push_back(PendingApproval {
        id: "t1".to_string(),
        tool: "write_file".to_string(),
        description: "Write file".to_string(),
        always: String::new(),
    });
    app.state = AppState::AwaitingApproval;
    assert_eq!(app.state(), AppState::AwaitingApproval);
    assert!(app.pending_approval().is_some());
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

#[test]
fn test_rewind_command_parses_turn_and_scope() {
    use crate::events::RewindScope;

    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);

    app.handle_command("/rewind");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Rewind {
            turn: None,
            scope: RewindScope::Both,
            ..
        })
    ));
    app.handle_command("/rewind 3");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Rewind {
            turn: Some(3),
            scope: RewindScope::Both,
            ..
        })
    ));
    app.handle_command("/rewind 2 code");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Rewind {
            turn: Some(2),
            scope: RewindScope::Code,
            ..
        })
    ));

    for bad in [
        "/rewind 0",
        "/rewind x",
        "/rewind 2 files later",
        "/rewind 2 nope",
    ] {
        app.handle_command(bad);
        assert!(rx.try_recv().is_err(), "{bad} should not be sent");
    }
}

#[test]
fn test_diff_command_picks_turn_or_session() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);

    app.handle_command("/diff");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Diff { session: false, .. })
    ));
    app.handle_command("/diff session");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Diff { session: true, .. })
    ));
    app.handle_command("/diff everything");
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_compare_passes_its_arguments_as_typed() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "abc".to_string();
    app.handle_command("/compare fix the \"flaky\" test --repeats 2");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Compare { ref agent_id, ref args })
            if agent_id == "abc" && args == "fix the \"flaky\" test --repeats 2"
    ));
    app.handle_command("/compare");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::Compare { ref args, .. }) if args.is_empty()
    ));
}

#[test]
fn test_gate_asks_for_the_gate_view() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.handle_command("/gate");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ShowGate)));
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
        Ok(UiEvent::RunCommand { command, cols: 80, .. }) if command == "echo hi"
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
fn a_typed_command_shows_its_output_as_it_runs_until_it_finishes() {
    let mut app = App::new();
    submit(&mut app, "$cargo build");
    app.process_command(UiCommand::CommandOutput {
        chunk: "Compiling a\n".to_string(),
    });
    app.process_command(UiCommand::CommandOutput {
        chunk: "Compiling b\n".to_string(),
    });
    let run = app.messages()[0].shell_run().expect("shell message");
    assert_eq!(run.live.tail(5), ["Compiling a", "Compiling b"]);

    app.process_command(UiCommand::CommandFinished {
        output: "Compiling a\nCompiling b\nFinished\n".to_string(),
        exit_code: None,
    });
    let run = app.messages()[0].shell_run().expect("shell message");
    assert!(run.live.is_empty(), "the full output replaces it");
    app.process_command(UiCommand::CommandOutput {
        chunk: "late\n".to_string(),
    });
    let run = app.messages()[0].shell_run().expect("shell message");
    assert!(run.live.is_empty(), "a finished command takes no more");
}

/// The tool step of `app`'s last message.
fn last_tool(app: &App) -> &ToolStep {
    match app.messages().back().and_then(Message::activity_step) {
        Some(Activity::Tool(tool)) => tool,
        other => panic!("expected a tool step, got {other:?}"),
    }
}

fn run_command_call(app: &mut App, id: &str) {
    app.process_command(UiCommand::ShowTool(crate::events::ToolData {
        id: id.to_string(),
        name: "run_command".to_string(),
        args: r#"{"shell_script":"cargo test"}"#.to_string(),
    }));
}

#[test]
fn a_running_call_keeps_its_output_until_its_result_arrives() {
    let mut app = App::new();
    submit(&mut app, "test it");
    run_command_call(&mut app, "t1");
    app.process_command(UiCommand::ToolOutput {
        id: "t1".to_string(),
        chunk: "running 3 tests\n".to_string(),
    });
    assert_eq!(last_tool(&app).live.tail(5), ["running 3 tests"]);
    assert!(last_tool(&app).live.last_at().is_some());

    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "ok".to_string(),
        success: true,
    });
    assert!(last_tool(&app).live.is_empty());
    app.process_command(UiCommand::ToolOutput {
        id: "t1".to_string(),
        chunk: "late\n".to_string(),
    });
    assert!(
        last_tool(&app).live.is_empty(),
        "a finished call takes no more"
    );
}

#[test]
fn the_working_line_counts_the_running_call_not_the_turn() {
    let mut app = App::new();
    submit(&mut app, "test it");
    app.processing_since = Instant::now().checked_sub(Duration::from_secs(600));
    assert!(app.working_times().0.unwrap() >= Duration::from_secs(600));

    run_command_call(&mut app, "t1");
    let (elapsed, quiet) = app.working_times();
    assert!(elapsed.unwrap() < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(quiet, None, "it only just started");

    app.messages.back_mut().unwrap().update_activity(|step| {
        if let Activity::Tool(tool) = step {
            tool.started = Instant::now().checked_sub(Duration::from_secs(90));
        }
    });
    let (elapsed, quiet) = app.working_times();
    assert!(elapsed.unwrap() >= Duration::from_secs(90));
    assert!(
        quiet.unwrap() >= Duration::from_secs(90),
        "nothing printed yet"
    );

    app.process_command(UiCommand::ToolOutput {
        id: "t1".to_string(),
        chunk: "test a ... ok\n".to_string(),
    });
    assert_eq!(app.working_times().1, None, "it just printed");
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

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "ls -la"
    ));
}

#[test]
fn test_prose_is_sent_as_user_message() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new().with_event_sender(tx);

    submit(&mut app, "what is in here?");

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, shown: true, .. }) if text == "what is in here?"
    ));
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
fn test_agent_switch_allowed_while_processing() {
    let (mut app, mut rx) = app_with_agents();
    app.state = AppState::Processing;

    app.handle_command("/agent 2");

    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

/// Stream `text` into agent `id`'s chat as a reply in progress.
fn stream_to(app: &mut App, id: &str, text: &str) {
    for command in [
        UiCommand::TurnStarted {
            prompt: Some(format!("ask {id}")),
        },
        UiCommand::StartStreaming,
        UiCommand::AppendText(text.to_string()),
    ] {
        app.process_command(UiCommand::ForAgent {
            agent_id: id.to_string(),
            command: Box::new(command),
        });
    }
}

#[test]
fn test_steered_message_splits_the_streaming_reply() {
    let (mut app, _rx) = app_with_agents();
    for command in [
        UiCommand::TurnStarted {
            prompt: Some("start".to_string()),
        },
        UiCommand::StartStreaming,
        UiCommand::AppendText("first half".to_string()),
        UiCommand::UserSteered("also do x".to_string()),
        UiCommand::StartStreaming,
        UiCommand::AppendText("second half".to_string()),
        UiCommand::FinishStreaming,
    ] {
        app.process_command(command);
    }

    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(
        contents,
        ["start", "first half", "also do x", "second half"]
    );
}

#[test]
fn test_background_agent_streams_into_its_own_chat() {
    let (mut app, _rx) = app_with_agents();
    submit(&mut app, "hello aa");

    stream_to(&mut app, "bb", "working on it");

    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["hello aa"]);
    assert!(app.agent_busy("bb"));

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["ask bb", "working on it"]);
    assert_eq!(app.state(), AppState::Processing);
    assert!(app.agent_busy("aa"));

    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["hello aa"]);
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn test_switching_back_shows_a_turn_that_finished_meanwhile() {
    let (mut app, _rx) = app_with_agents();
    stream_to(&mut app, "aa", "first ");
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(app.state(), AppState::Idle);

    for command in [
        UiCommand::AppendText("draft".to_string()),
        UiCommand::FinishStreaming,
        UiCommand::Complete,
    ] {
        app.process_command(UiCommand::ForAgent {
            agent_id: "aa".to_string(),
            command: Box::new(command),
        });
    }
    assert!(app.messages().is_empty());
    assert!(!app.agent_busy("aa"));
    assert!(matches!(
        app.notification(),
        Some((text, NotificationType::Success)) if text == "Terminal Agent finished"
    ));

    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["ask aa", "first draft"]);
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn a_boot_console_is_updated_in_place_in_its_agents_chat() {
    use crate::events::{BootStageState, BootUpdate};
    let (mut app, _rx) = app_with_agents();
    let boot = |update: BootUpdate| UiCommand::ForAgent {
        agent_id: "bb".to_string(),
        command: Box::new(UiCommand::Boot {
            id: "boot-1".to_string(),
            update,
        }),
    };
    app.process_command(boot(BootUpdate::Begin {
        title: "Moving to the cloud".into(),
    }));
    app.process_command(boot(BootUpdate::Stage {
        key: "create".into(),
        label: "Creating machine".into(),
        state: BootStageState::Running,
    }));
    assert!(app.messages().is_empty(), "the console is in bb's chat");

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(app.messages().len(), 1);
    assert!(app.chat_animating(), "a running console animates the chat");
    app.process_command(boot(BootUpdate::Stage {
        key: "create".into(),
        label: "Creating machine".into(),
        state: BootStageState::Done { ms: 900 },
    }));
    app.process_command(boot(BootUpdate::End {
        ok: true,
        summary: "Now running in the cloud.".into(),
    }));
    assert_eq!(app.messages().len(), 1, "updates change the one console");
    let console = app.messages()[0].boot_console().expect("console");
    assert_eq!(console.steps[0].state, BootStageState::Done { ms: 900 });
    assert!(!console.is_running());
    assert!(!app.chat_animating());
}

#[test]
fn a_boot_console_in_a_loaded_history_comes_back_and_goes_on() {
    use crate::events::{BootHistory, BootStageState, BootUpdate};
    let mut app = App::new();
    let stage = |state| BootUpdate::Stage {
        key: "create".into(),
        label: "Creating machine".into(),
        state,
    };
    let console = |id: &str, updates: Vec<BootUpdate>, elapsed_ms| MessageData {
        timestamp_ms: Some(1_700_000_000_000),
        boot: Some(BootHistory {
            id: id.into(),
            updates,
            elapsed_ms,
        }),
        ..MessageData::new(MessageRole::System, "")
    };
    app.process_command(UiCommand::LoadConversation(vec![
        console(
            "boot-old",
            vec![
                BootUpdate::Begin {
                    title: "Moving to the cloud".into(),
                },
                stage(BootStageState::Running),
                BootUpdate::End {
                    ok: false,
                    summary: "claim failed".into(),
                },
            ],
            Some(4_200),
        ),
        console("boot-live", vec![stage(BootStageState::Running)], None),
    ]));
    let ended = app.messages()[0].boot_console().expect("console");
    assert_eq!(ended.title, "Moving to the cloud");
    let outcome = ended.outcome.as_ref().expect("ended");
    assert_eq!((outcome.ok, outcome.elapsed_ms), (false, 4_200));
    assert!(app.messages()[1].is_live_boot());

    app.process_command(UiCommand::Boot {
        id: "boot-live".into(),
        update: stage(BootStageState::Done { ms: 300 }),
    });
    assert_eq!(
        app.messages().len(),
        2,
        "the update went to the console shown"
    );
    let live = app.messages()[1].boot_console().expect("console");
    assert_eq!(live.steps[0].state, BootStageState::Done { ms: 300 });
}

#[test]
fn only_what_changes_the_screen_asks_for_a_frame() {
    let (mut app, _rx) = app_with_agents();
    assert!(app.take_redraw(), "nothing is drawn yet");
    assert!(!app.take_redraw());

    stream_to(&mut app, "bb", "working on it");
    assert!(!app.take_redraw(), "a chat off screen is not on screen");
    stream_to(&mut app, "aa", "on screen");
    assert!(app.take_redraw());

    let step = |agent_id: &str| UiCommand::ForAgent {
        agent_id: agent_id.to_string(),
        command: Box::new(UiCommand::GraphUpsert {
            run_id: format!("run-{agent_id}"),
            nodes: Vec::new(),
        }),
    };
    app.process_command(step("bb"));
    assert!(!app.take_redraw(), "the steps feed is not on screen");
    app.side_tab = SidePanelTab::Trace;
    app.process_command(step("bb"));
    assert!(app.take_redraw(), "the steps feed is on screen");

    for command in [UiCommand::FinishStreaming, UiCommand::Complete] {
        app.process_command(UiCommand::ForAgent {
            agent_id: "bb".to_string(),
            command: Box::new(command),
        });
    }
    assert!(app.take_redraw(), "an agent off screen finishing says so");
    assert!(app.notification().is_some());
}

#[test]
fn a_tick_asks_for_a_frame_only_while_something_moves() {
    let (mut app, _rx) = app_with_agents();
    app.take_redraw();
    app.tick();
    assert!(!app.take_redraw(), "nothing moves");

    let mut working = agent("bb", "Researcher");
    working.running = true;
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        working,
    ]));
    app.take_redraw();
    app.tick();
    assert!(app.take_redraw(), "a working agent's spinner turns");
    let frame = app.animation_frame();
    app.tick();
    app.tick();
    assert!(!app.take_redraw(), "spinners alone turn every few ticks");
    assert_eq!(
        app.animation_frame(),
        frame,
        "and step one frame when they do"
    );
    app.tick();
    assert!(app.take_redraw());
    assert_eq!(app.animation_frame(), frame + 1);
}

#[test]
fn test_each_agent_has_its_own_trace_feed() {
    let (mut app, _rx) = app_with_agents();
    let step = |agent_id: &str, id: &str| UiCommand::ForAgent {
        agent_id: agent_id.to_string(),
        command: Box::new(UiCommand::GraphUpsert {
            run_id: format!("run-{agent_id}"),
            nodes: vec![crate::events::TraceNode {
                id: id.to_string(),
                kind: "tool_call".to_string(),
                ..Default::default()
            }],
        }),
    };
    let feed = |app: &App| -> Vec<String> {
        app.trace_feed()
            .entries()
            .map(|e| e.node.id.clone())
            .collect()
    };
    app.process_command(step("aa", "a1"));
    app.process_command(step("bb", "b1"));
    app.process_command(step("bb", "b2"));

    assert_eq!(feed(&app), ["a1"]);
    assert!(!app.agent_busy("bb"));
    assert!(app.notification().is_none());
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(feed(&app), ["b1", "b2"]);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(feed(&app), ["a1"]);
}

#[test]
fn enter_on_an_empty_prompt_sends_the_queued_message_now() {
    let (mut app, mut rx) = app_with_agents();
    submit(&mut app, "first");
    assert!(rx.try_recv().is_ok());
    submit(&mut app, "second");
    assert!(rx.try_recv().is_ok());
    // Another key ends the "just queued" window.
    app.handle_key(KeyEvent::from(KeyCode::Left));

    // Nothing is queued yet as far as the status line knows.
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(rx.try_recv().is_err());

    app.process_command(UiCommand::SetQueue(vec![crate::events::QueuedItemView {
        id: 1,
        text: "second".to_string(),
        editable: true,
    }]));
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SendQueuedNow(id)) if id == "aa"
    ));
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn enter_again_right_after_queueing_sends_the_message_now() {
    let (mut app, mut rx) = app_with_agents();
    submit(&mut app, "first");
    assert!(rx.try_recv().is_ok());
    submit(&mut app, "second");
    assert!(rx.try_recv().is_ok());

    // Before the event loop's `SetQueue` arrives, and with another panel
    // holding the keys.
    submit(&mut app, "third");
    assert!(rx.try_recv().is_ok());
    app.focus = super::PanelFocus::Swarm;
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SendQueuedNow(id)) if id == "aa"
    ));
    assert_eq!(app.state(), AppState::Processing);

    // Only the first Enter after queueing does it.
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
}

#[test]
fn enter_after_a_message_to_an_idle_agent_sends_nothing() {
    let (mut app, mut rx) = app_with_agents();
    submit(&mut app, "first");
    assert!(rx.try_recv().is_ok());
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_message_to_busy_agent_is_queued_not_shown() {
    let (mut app, mut rx) = app_with_agents();
    submit(&mut app, "first");
    assert!(rx.try_recv().is_ok());

    submit(&mut app, "second");

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { agent_id, text, shown: false, .. })
            if agent_id == "aa" && text == "second"
    ));
    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["first"]);
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn test_records_of_background_agents_are_not_listed() {
    let (mut app, _rx) = app_with_agents();
    let record = |agent_id: &str| crate::events::RecordSummary {
        seq: 1,
        timestamp: String::new(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
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
        agent_id: agent_id.to_string(),
        ts_ms: 0,
        policy: None,
    };

    app.process_command(UiCommand::NewRecord(record("bb")));
    app.process_command(UiCommand::NewRecord(record("aa")));

    assert_eq!(app.records().len(), 1);
}

fn record_of(agent_id: &str, seq: u64) -> crate::events::RecordSummary {
    crate::events::RecordSummary {
        seq,
        timestamp: String::new(),
        full_hash: format!("hash{seq}"),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
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
        agent_id: agent_id.to_string(),
        ts_ms: 0,
        policy: None,
    }
}

#[test]
fn test_an_agent_comes_back_with_its_records_and_files_where_it_left_them() {
    let (mut app, _rx) = app_with_agents();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(dir_a.path().join("src")).unwrap();
    std::fs::write(dir_a.path().join("src/main.rs"), "").unwrap();
    std::fs::write(dir_b.path().join("notes.md"), "").unwrap();
    let records_a: Vec<_> = (1..=5).map(|seq| record_of("aa", seq)).collect();
    app.process_command(UiCommand::SetRecords(records_a.clone()));
    app.process_command(UiCommand::SetSandboxRoot(dir_a.path().to_path_buf()));
    app.selected_record = 1;
    app.records_scroll = 1;
    app.sandbox.toggle_selected();
    app.sandbox.select(1);
    app.sandbox_scroll = 1;
    let file = dir_a.path().join("src/main.rs");
    assert_eq!(app.sandbox.rows()[1].path, file);

    // The session sends each agent's records and workspace as it comes
    // on screen.
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.process_command(UiCommand::SetRecords(vec![record_of("bb", 1)]));
    app.process_command(UiCommand::SetSandboxRoot(dir_b.path().to_path_buf()));
    assert_eq!(app.records().len(), 1);
    assert_eq!(app.sandbox.root(), Some(dir_b.path()));
    assert_eq!((app.records_scroll, app.sandbox_scroll), (0, 0));

    std::fs::write(dir_a.path().join("src/lib.rs"), "").unwrap();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(
        app.records().len(),
        5,
        "back from memory before the session answers"
    );
    app.process_command(UiCommand::SetRecords(records_a));
    app.process_command(UiCommand::SetSandboxRoot(dir_a.path().to_path_buf()));
    assert_eq!((app.selected_record, app.records_scroll), (1, 1));
    assert_eq!(app.sandbox.rows()[app.sandbox.selected()].path, file);
    assert_eq!(app.sandbox_scroll, 1);

    // The tree is read again on the next tick, keeping its folds.
    app.tick();
    let names: Vec<&str> = app.sandbox.rows().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["src", "lib.rs", "main.rs"]);
    assert_eq!(app.sandbox.rows()[app.sandbox.selected()].path, file);
}

#[test]
fn test_new_records_keep_the_highlight_on_its_record() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(UiCommand::SetRecords(
        (1..=3).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.selected_record, 2, "on the newest");
    app.process_command(UiCommand::SetRecords(
        (1..=4).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.selected_record, 3, "following the newest");
    app.selected_record = 1;
    app.process_command(UiCommand::SetRecords(
        (1..=5).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.selected_record, 1, "left on its record");

    // Records sent for an agent the screen has left since change nothing.
    app.process_command(UiCommand::SetRecords(vec![record_of("bb", 1)]));
    assert_eq!(app.records().len(), 5);
}

#[test]
fn test_agent_command_selects_by_number_and_name() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert_eq!(app.active_agent_id(), "bb", "switched on screen at once");

    // Already on screen: nothing to ask for.
    app.handle_command("/agent researcher");
    assert!(rx.try_recv().is_err());

    // The first agent's chat is held in memory, so the session is only
    // told it is in front.
    app.handle_command("/agent 1");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "aa"));

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

#[test]
fn test_agents_of_one_project_stay_in_it_unindented() {
    use super::ProjectListRow;

    let (mut app, _rx) = app_with_agents();
    let in_project = |project: &str, id: &str, name: &str| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        ..agent(id, name)
    };
    let spawned = crate::events::AgentSummary {
        depth: 1,
        ..in_project("site", "dd", "explorer #1")
    };
    app.process_command(UiCommand::SetAgents(vec![
        in_project("site", "aa", "Agent"),
        in_project("site", "bb", "Agent 2"),
        spawned,
        in_project("site", "cc", "reviewer #1"),
        in_project("notes", "ee", "Other"),
    ]));

    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: true,
            },
            ProjectListRow::Agent { index: 0 },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::Agent { index: 2 },
            ProjectListRow::Agent { index: 3 },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 4,
                expanded: true,
            },
            ProjectListRow::Agent { index: 4 },
            ProjectListRow::NewProject,
        ]
    );
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
            ..Default::default()
        },
        crate::events::AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
            ..Default::default()
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
    app.input = "hello".to_string();
    app.cursor_pos = app.input.len();
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

/// Open the folder browser with Ctrl+N, pointed at `dir` instead of the
/// launch directory.
fn open_folder_picker(app: &mut App, dir: &std::path::Path) {
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(app.state(), AppState::SelectingFolder);
    app.folder_picker = Some(FolderPicker::open(dir.to_path_buf()).unwrap());
}

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

fn mouse(kind: crossterm::event::MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
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
    let three_minutes_ago = crate::renderer::now_ms() - 3 * 60 * 1000;
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
        super::context_menu::MenuTarget::Project("notes".to_string())
    );
    assert_eq!(
        menu.items,
        [
            super::context_menu::MenuItem::AddAgent,
            super::context_menu::MenuItem::Info,
            super::context_menu::MenuItem::RemoveProject,
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

fn app_with_agent_areas() -> (App, mpsc::Receiver<UiEvent>) {
    let (app, rx) = app_with_agents();
    (with_agent_areas(app), rx)
}

fn with_agent_areas(mut app: App) -> App {
    let swarm = Rect::new(0, 3, 20, 13);
    app.set_panel_areas(crate::app::PanelAreas {
        swarm: Some(swarm),
        agent_rows: None,
        chat: Some(Rect::new(20, 3, 60, 13)),
        ..crate::app::PanelAreas::default()
    });
    app.set_agent_rows_area(Rect::new(1, 4, 18, 10));
    app.set_new_project_area(Rect::new(1, 14, 18, 1));
    app
}

/// Projects `aa` (with child `aa1`), `bb` and `cc`. Rows from line 4:
/// aa 4, 5, 6; gap 7; bb 8, 9; gap 10; cc 11, 12.
fn app_with_three_projects() -> App {
    let mut app = App::new();
    let child = crate::events::AgentSummary {
        depth: 1,
        ..agent("aa1", "Child")
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "A"),
        child,
        agent("bb", "B"),
        agent("cc", "C"),
    ]));
    with_agent_areas(app)
}

fn agent_ids(app: &App) -> Vec<&str> {
    app.agents().iter().map(|a| a.id.as_str()).collect()
}

fn left(app: &mut App, kind: fn(MouseButton) -> MouseEventKind, row: u16) {
    app.handle_mouse(mouse(kind(MouseButton::Left), 5, row));
}

#[test]
fn test_dragging_a_project_reorders_the_list_live() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);

    // Onto bb: aa, with its child, takes bb's place at once.
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert!(app.project_dragging());
    assert_eq!(app.project_order(), ["bb", "aa", "cc"]);

    // Onto cc, and back up to the top, still before the button is up.
    left(&mut app, MouseEventKind::Drag, 12);
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    left(&mut app, MouseEventKind::Drag, 4);
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);

    left(&mut app, MouseEventKind::Drag, 11);
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    left(&mut app, MouseEventKind::Up, 11);
    assert!(!app.project_dragging());
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    // Dropping a project does not fold it.
    assert!(matches!(
        app.project_rows()[app.project_rows().len() - 4],
        ProjectListRow::Project { expanded: true, .. }
    ));
}

#[test]
fn test_dropping_a_dragged_project_sends_the_new_order() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app = app_with_three_projects();
    app.event_tx = Some(tx);
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Drag, 8);
    assert!(rx.try_recv().is_err(), "nothing is sent mid-drag");
    left(&mut app, MouseEventKind::Up, 8);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::ReorderProjects(order)) if order == ["bb", "aa", "cc"]
    ));

    // A click that folds sends no order.
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Up, 4);
    assert!(rx.try_recv().is_err());
}

/// Project `site` with `a1` (which spawned `a1c`), `a2` and `a3`, then
/// project `notes` with `n1`. Rows from line 4: site 4, a1 5, a1c 6, a2 7,
/// a3 8; gap 9; notes 10, n1 11.
fn app_with_agents_to_drag() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new();
    app.event_tx = Some(tx);
    let at = |project: &str, id: &str, depth: usize| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        depth,
        ..agent(id, &id.to_uppercase())
    };
    app.process_command(UiCommand::SetAgents(vec![
        at("site", "a1", 0),
        at("site", "a1c", 1),
        at("site", "a2", 0),
        at("site", "a3", 0),
        at("notes", "n1", 0),
    ]));
    (with_agent_areas(app), rx)
}

#[test]
fn test_dragging_an_agent_reorders_its_project_and_sends_the_order() {
    let (mut app, mut rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 5);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "a1"));

    // Onto a3: a1 and the agent it spawned move below it.
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["a2", "a3", "a1", "a1c", "n1"]);
    assert!(app.project_dragging());
    // It never leaves its project.
    left(&mut app, MouseEventKind::Drag, 11);
    assert_eq!(agent_ids(&app), ["a2", "a3", "a1", "a1c", "n1"]);

    left(&mut app, MouseEventKind::Up, 11);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::ReorderAgents { project_id, order })
            if project_id == "site" && order == ["a2", "a3", "a1"]
    ));
}

#[test]
fn test_a_spawned_agent_is_not_dragged_on_its_own() {
    let (mut app, mut rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 6);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["a1", "a1c", "a2", "a3", "n1"]);
    left(&mut app, MouseEventKind::Up, 8);
    let sent: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        !sent
            .iter()
            .any(|event| matches!(event, UiEvent::ReorderAgents { .. })),
        "{sent:?}"
    );
}

#[test]
fn test_a_project_drag_reports_the_rows_under_the_pointer() {
    use crate::app::project_drag::ProjectDragHit;

    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 8);
    assert_eq!(app.project_drag_hit(), None, "a press is not a drag");

    // Onto aa's child: bb would not land under the pointer, so it stays,
    // and aa is the hit area. List rows: aa 0..3, gap, bb 4..6.
    let redraw = app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 6));
    assert!(redraw, "a new hit area is drawn even without a move");
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);
    assert_eq!(
        app.project_drag_hit(),
        Some(ProjectDragHit {
            dragged: 4..6,
            target: Some(0..3),
            edge: Some(0),
        })
    );

    // Onto aa's header: bb takes its place and is its own hit area, and
    // goes between itself and aa, which it passed.
    left(&mut app, MouseEventKind::Drag, 4);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert_eq!(
        app.project_drag_hit(),
        Some(ProjectDragHit {
            dragged: 0..2,
            target: Some(0..2),
            edge: Some(2),
        })
    );

    left(&mut app, MouseEventKind::Up, 4);
    assert_eq!(app.project_drag_hit(), None);
}

#[test]
fn test_an_agent_drag_has_no_hit_area_outside_its_project() {
    let (mut app, _rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 5);
    left(&mut app, MouseEventKind::Drag, 11);
    let hit = app.project_drag_hit().expect("dragging");
    // a1 and the agent it spawned, still in place.
    assert_eq!(hit.dragged, 1..3);
    assert_eq!(hit.target, None);

    left(&mut app, MouseEventKind::Drag, 8);
    let hit = app.project_drag_hit().expect("dragging");
    assert_eq!(hit.target, Some(hit.dragged));
}

#[test]
fn test_a_redraw_is_asked_for_each_move_of_a_dragged_project() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);
    let moved = app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 8));
    assert!(moved);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
}

#[test]
fn test_a_click_without_movement_folds_and_does_not_reorder() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project { expanded: true, .. }
    ));
    left(&mut app, MouseEventKind::Up, 4);
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);
    assert!(!app.project_dragging());
}

#[test]
fn test_a_dragged_project_does_not_swap_back_under_a_still_pointer() {
    let mut app = app_with_three_projects();
    // cc is one row tall once folded; dragging it up over the taller aa
    // must not flip between the two on the way.
    left(&mut app, MouseEventKind::Down, 11);
    left(&mut app, MouseEventKind::Drag, 8);
    let once = agent_ids(&app).join(",");
    left(&mut app, MouseEventKind::Drag, 9);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app).join(","), once);
    left(&mut app, MouseEventKind::Up, 8);
    let mut order = app.project_order();
    order.sort();
    assert_eq!(order, ["aa", "bb", "cc"]);
}

#[test]
fn test_dragging_keeps_the_highlight_on_its_agent() {
    let mut app = app_with_three_projects();
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    let highlighted = |app: &App| match app.project_rows()[app.selected_agent()] {
        ProjectListRow::Agent { index } => app.agents()[index].id.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(highlighted(&app), "bb");
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert_eq!(highlighted(&app), "bb");
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
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
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
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
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
        app.notification(),
        Some(&("Copied 4 characters".to_string(), NotificationType::Success))
    );

    app.tick();
    assert!(app.notification().is_some());

    let (_, until) = app.notification_shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);
    app.tick();
    assert!(app.notification().is_none());
}

#[test]
fn test_keypress_clears_copy_notice() {
    let mut app = App::new();
    app.notify_copied(1, &Ok(()));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
    assert!(app.notification().is_none());
}

#[test]
fn test_notification_expires_and_a_new_one_starts_its_own_timer() {
    let mut app = App::new();
    app.notification = Some(("first".to_string(), NotificationType::Warning));
    app.tick();
    let (_, until) = app.notification_shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);

    app.notification = Some(("second".to_string(), NotificationType::Error));
    app.tick();
    assert_eq!(app.notification().unwrap().0, "second");
    let (_, until) = app.notification_shown.as_ref().unwrap();
    assert!(*until > std::time::Instant::now() + std::time::Duration::from_secs(5));

    let (_, until) = app.notification_shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);
    app.tick();
    assert!(app.notification().is_none());
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

    // Three rows a notch, as in the chat.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 5));
    assert_eq!(app.agents_scroll(), 0);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    assert_eq!(app.agents_scroll(), 3);

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
fn test_reply_model_sets_the_turn_so_far_on_replies_since_last_user_message() {
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
            unpriced_calls: 0,
        }),
        sources: Vec::new(),
    });
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("second part".to_string()));
    let opus = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.042),
        unpriced_calls: 0,
    };
    app.process_command(UiCommand::SetReplyModel {
        model: "qwen3:4b · claude-opus-4-7".to_string(),
        usage: Some(opus),
        sources: Vec::new(),
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
            (
                "first part",
                Some("qwen3:4b · claude-opus-4-7"),
                Some(12_400)
            ),
            (
                "second part",
                Some("qwen3:4b · claude-opus-4-7"),
                Some(12_400)
            ),
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

#[test]
fn test_clicking_the_todo_checklist_expands_and_collapses_it() {
    use crate::events::{TodoState, TodoView};
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
    let checklist = |open| ToggleRow {
        line: 0,
        target: ToggleTarget::TodoChecklist,
        open,
    };

    assert!(!app.todos_expanded());
    app.set_chat_layout(vec![checklist(false)], 0);
    click(&mut app);
    assert!(app.todos_expanded());

    app.set_chat_layout(vec![checklist(true)], 0);
    click(&mut app);
    assert!(!app.todos_expanded());

    // An emptied list starts the next one short.
    click(&mut app);
    assert!(app.todos_expanded());
    app.process_command(UiCommand::SetTodos(vec![TodoView {
        content: "next".to_string(),
        state: TodoState::Pending,
        assignee: None,
        task: None,
    }]));
    assert!(app.todos_expanded());
    app.process_command(UiCommand::SetTodos(Vec::new()));
    assert!(!app.todos_expanded());
}

#[test]
fn test_clicking_a_long_tool_result_opens_it_in_the_file_view() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    for (id, output) in [("t1", "a\n".repeat(30)), ("t2", "short".to_string())] {
        app.process_command(UiCommand::ShowTool(crate::events::ToolData {
            id: id.to_string(),
            name: "run_command".to_string(),
            args: r#"{"program":"cargo","args":["test"]}"#.to_string(),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: output,
            success: true,
        });
    }
    let ids: Vec<u64> = app.messages().iter().map(|m| m.version().0).collect();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    app.set_chat_layout(step_rows(&ids), 0);
    let click = |app: &mut App, row| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: 10,
                row,
                modifiers: KeyModifiers::empty(),
            });
        }
    };

    click(&mut app, 2);
    assert!(app.detail_view().is_none(), "short output expands inline");
    assert!(app.messages()[1].is_expanded());

    click(&mut app, 1);
    let Some(DetailView::File(view)) = app.detail_view() else {
        panic!("long output opens in the file view");
    };
    assert_eq!(view.extension(), "log");
    assert!(!app.messages()[0].is_expanded());
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
                context_window: Some(32_768),
            },
            ModelOptionView {
                id: "aura-claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Cloud (aura-router)".to_string(),
                provider: Some("zos".to_string()),
                available: user.is_some(),
                context_window: Some(1_000_000),
            },
            ModelOptionView {
                id: "claude/claude-sonnet-4-6".to_string(),
                label: "Claude Sonnet 4.6".to_string(),
                local: false,
                installed: true,
                group: "Claude plan (direct)".to_string(),
                provider: Some("claude".to_string()),
                available: false,
                context_window: Some(1_000_000),
            },
        ],
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: user.map(str::to_string),
        plans: Vec::new(),
        thinking: None,
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
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
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
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
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
    assert_eq!(app.model_picker().cursor(), 0);

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
fn test_selector_typing_filters_and_esc_clears_before_closing() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model");
    for c in "sonnet".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.model_picker().filter(), "sonnet");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(app.model_picker().filter(), "");

    for c in "zzz".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.state(),
        AppState::SelectingModel,
        "no match keeps it open"
    );
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Backspace));
    }
    for c in "sonnet".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
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
    // Signed-out cloud models live on the Sign-in tab.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::LoginEmail);
    assert!(rx.try_recv().is_err());

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
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
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
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
    assert!(app.sandbox().rows().is_empty(), "read on the next tick");
    app.tick();
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
    app.state = AppState::Processing;
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
    app.state = AppState::Idle;

    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "/clear");
    assert!(app.slash_matches().is_empty());
}

// ========================================================================
// Approvals, modes and alerts
// ========================================================================

fn approval_request(id: &str, always: &str) -> UiCommand {
    UiCommand::RequestApproval {
        id: id.to_string(),
        tool: "run_command".to_string(),
        description: "cargo test".to_string(),
        always: always.to_string(),
    }
}

fn for_agent(agent_id: &str, command: UiCommand) -> UiCommand {
    UiCommand::ForAgent {
        agent_id: agent_id.to_string(),
        command: Box::new(command),
    }
}

#[test]
fn approval_keys_answer_once_always_or_deny() {
    for (key_char, expect) in [('y', "approve"), ('a', "always"), ('n', "deny")] {
        let (tx, mut rx) = mpsc::channel(10);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(approval_request("req-1", "`cargo test` commands"));
        assert_eq!(app.state(), AppState::AwaitingApproval);

        app.handle_key(key(KeyCode::Char(key_char)));

        let event = rx.try_recv().unwrap();
        let matched = match expect {
            "approve" => matches!(event, UiEvent::Approve(ref id) if id == "req-1"),
            "always" => matches!(event, UiEvent::ApproveAlways(ref id) if id == "req-1"),
            _ => matches!(event, UiEvent::Deny(ref id) if id == "req-1"),
        };
        assert!(matched, "{key_char}: {event:?}");
        assert_eq!(app.state(), AppState::Processing);
        assert!(app.pending_approval().is_none());
    }
}

#[test]
fn always_is_not_offered_when_the_call_cannot_be_remembered() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(approval_request("req-1", ""));
    app.handle_key(key(KeyCode::Char('a')));
    assert!(rx.try_recv().is_err());
    assert_eq!(app.state(), AppState::AwaitingApproval);
}

#[test]
fn a_closed_approval_drops_its_prompt() {
    let mut app = App::new();
    app.process_command(approval_request("req-1", ""));
    app.process_command(UiCommand::CloseApproval("other".to_string()));
    assert!(app.pending_approval().is_some());
    app.process_command(UiCommand::CloseApproval("req-1".to_string()));
    assert!(app.pending_approval().is_none());
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn approvals_asked_together_are_answered_in_turn() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    for id in ["req-1", "req-2", "req-3"] {
        app.process_command(approval_request(id, ""));
    }
    assert_eq!(app.pending_approval().unwrap().id, "req-1");
    assert_eq!(app.pending_approval_count(), 3);

    app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(app.state(), AppState::AwaitingApproval);
    assert_eq!(app.pending_approval().unwrap().id, "req-2");
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.pending_approval().unwrap().id, "req-3");
    app.handle_key(key(KeyCode::Char('y')));
    assert!(app.pending_approval().is_none());
    assert_eq!(app.state(), AppState::Processing);

    let answered: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|event| match event {
            UiEvent::Approve(id) => format!("y:{id}"),
            UiEvent::Deny(id) => format!("n:{id}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(answered, ["y:req-1", "n:req-2", "y:req-3"]);
}

#[test]
fn a_call_waiting_for_approval_is_marked_until_answered() {
    let mut app = App::new();
    app.process_command(tool_data("toolu_1", "edit_file"));
    app.process_command(tool_data("toolu_2", "edit_file"));
    app.process_command(approval_request("aa:toolu_1", ""));
    app.process_command(approval_request("aa:toolu_2", ""));
    let waiting = |app: &App| -> Vec<bool> {
        activities(app)
            .iter()
            .map(|a| matches!(a, Activity::Tool(t) if t.awaiting_approval))
            .collect()
    };
    assert_eq!(waiting(&app), [true, true]);

    app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(waiting(&app), [false, true]);
    app.process_command(UiCommand::CloseApproval("aa:toolu_2".to_string()));
    assert_eq!(waiting(&app), [false, false]);
}

#[test]
fn closing_a_queued_approval_keeps_the_others_in_order() {
    let mut app = App::new();
    for id in ["req-1", "req-2", "req-3"] {
        app.process_command(approval_request(id, ""));
    }
    app.process_command(approval_request("req-2", ""));
    assert_eq!(
        app.pending_approval_count(),
        3,
        "a repeated request is not queued twice"
    );

    app.process_command(UiCommand::CloseApproval("req-2".to_string()));
    assert_eq!(app.pending_approval().unwrap().id, "req-1");
    app.process_command(UiCommand::CloseApproval("req-1".to_string()));
    assert_eq!(app.pending_approval().unwrap().id, "req-3");
    assert_eq!(app.state(), AppState::AwaitingApproval);
    app.process_command(UiCommand::CloseApproval("req-3".to_string()));
    assert_eq!(app.state(), AppState::Processing);
}

#[test]
fn queued_approvals_follow_their_agent_across_chat_switches() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(for_agent("bb", UiCommand::TurnStarted { prompt: None }));
    app.process_command(for_agent("bb", approval_request("req-1", "")));
    app.process_command(for_agent("bb", approval_request("req-2", "")));
    assert!(app.pending_approval().is_none());

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(app.pending_approval_count(), 2);
    assert_eq!(app.pending_approval().unwrap().id, "req-1");
}

#[test]
fn cancel_while_awaiting_approval_cancels_the_turn() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(approval_request("req-1", ""));
    assert!(app.is_busy());
    app.cancel();
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Cancel(id)) if id == "aa"));
    assert!(app.pending_approval().is_none());
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn ctrl_t_stops_the_command_and_keeps_the_turn() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::TurnStarted { prompt: None });
    assert!(app.is_busy());
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::StopCommand(id)) if id == "aa"));
    assert!(app.is_busy());
    assert!(app.input.is_empty());
}

#[test]
fn approval_for_an_agent_off_screen_waits_in_its_chat() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(for_agent("bb", UiCommand::TurnStarted { prompt: None }));
    app.process_command(for_agent("bb", approval_request("req-b", "")));

    assert!(
        app.pending_approval().is_none(),
        "the shown chat is not asked"
    );
    assert_eq!(app.state(), AppState::Idle);
    let (note, _) = app.notification().unwrap();
    assert!(note.contains("needs your approval"), "{note}");
    assert_eq!(app.take_alerts().len(), 1);

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(app.state(), AppState::AwaitingApproval);
    assert_eq!(app.pending_approval().unwrap().id, "req-b");

    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert!(app.pending_approval().is_none());
    assert!(app.agent_busy("bb"));
}

#[test]
fn shift_tab_and_mode_command_ask_the_event_loop() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CycleApproval(id)) if id == "aa"));

    app.handle_command("/mode read-only");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode { agent_id, mode: crate::PermissionMode::ReadOnly }) if agent_id == "aa"
    ));

    app.handle_command("/mode chaos");
    assert!(rx.try_recv().is_err());
    assert!(app.notification().unwrap().0.contains("unknown mode"));

    app.handle_command("/mode unrestricted");
    assert!(rx.try_recv().is_err());
    assert!(app.notification().unwrap().0.contains("/approval"));
}

#[test]
fn thinking_command_and_alt_t_ask_the_event_loop() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    let thinking_is = |app: &mut App, level: Option<&str>| {
        app.process_command(UiCommand::SetModels {
            options: Vec::new(),
            current: "local/qwen3:4b".to_string(),
            route: None,
            user: None,
            plans: Vec::new(),
            thinking: level.map(str::to_string),
        });
    };

    app.handle_command("/thinking HIGH");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SetThinking(Some(l))) if l == "high"));

    thinking_is(&mut app, Some("high"));
    assert_eq!(app.thinking(), Some("high"));
    app.handle_command("/thinking high");
    assert!(rx.try_recv().is_err());
    assert!(app.notification().unwrap().0.contains("already high"));

    app.handle_command("/thinking auto");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SetThinking(None))));

    app.handle_command("/thinking lots");
    assert!(rx.try_recv().is_err());
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("Unknown thinking level"));

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SetThinking(Some(l))) if l == "xhigh"));
    thinking_is(&mut app, Some("max"));
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SetThinking(None))));
    thinking_is(&mut app, None);
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SetThinking(Some(l))) if l == "off"));
    assert!(app.input().is_empty(), "Alt+T types nothing");
}

#[test]
fn approval_command_asks_the_event_loop() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    app.handle_command("/approval strict");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetApproval { agent_id, level: crate::ApprovalLevel::Strict }) if agent_id == "aa"
    ));

    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Yolo));
    assert_eq!(app.approval(), Some(crate::ApprovalLevel::Yolo));
    app.handle_command("/approval yolo");
    assert!(rx.try_recv().is_err());
    assert!(app.notification().unwrap().0.contains("Already"));

    app.handle_command("/approval chaos");
    assert!(rx.try_recv().is_err());
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("unknown approval level"));
}

#[test]
fn approval_command_without_a_level_lists_the_levels() {
    let mut app = App::new();
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Strict));
    app.handle_command("/approval");
    let report = app.messages().back().unwrap().content().to_string();
    assert!(report.starts_with("Approval: strict"), "{report}");
    assert!(report.contains("yolo"), "{report}");
}

#[test]
fn mode_command_without_a_name_lists_the_modes() {
    let mut app = App::new();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    app.handle_command("/mode");
    let report = app.messages().back().unwrap().content().to_string();
    assert!(report.starts_with("Mode: plan"), "{report}");
    assert!(report.contains("read-only"), "{report}");
}

#[test]
fn new_session_keeps_the_mode() {
    let mut app = App::new();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Debug));
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Yolo));
    app.handle_command("/new");
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Debug));
    assert_eq!(app.hud().approval, Some(crate::ApprovalLevel::Yolo));
}

#[test]
fn a_long_turn_alerts_when_it_finishes_and_a_short_one_does_not() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(for_agent("aa", UiCommand::TurnStarted { prompt: None }));
    app.processing_since = Some(Instant::now());
    app.process_command(for_agent("aa", UiCommand::Complete));
    assert!(app.take_alerts().is_empty());

    app.process_command(for_agent("aa", UiCommand::TurnStarted { prompt: None }));
    app.processing_since = Instant::now().checked_sub(Duration::from_secs(30));
    app.process_command(for_agent("aa", UiCommand::Complete));
    let alerts = app.take_alerts();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].contains("finished after 30s"), "{alerts:?}");
}

// ========================================================================
// @ mentions
// ========================================================================

fn app_with_workspace() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("README.md"), "").unwrap();
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    (dir, app, rx)
}

fn type_keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
}

fn mention_paths(app: &App) -> Vec<String> {
    app.mention_matches()
        .iter()
        .map(|c| c.path.clone())
        .collect()
}

#[test]
fn test_at_opens_the_file_picker_and_enter_inserts_the_path() {
    let (_dir, mut app, mut rx) = app_with_workspace();
    type_keys(&mut app, "explain @");
    assert_eq!(
        mention_paths(&app),
        ["src/", "README.md", "src/lib.rs", "src/main.rs"],
        "skips target/, shallowest first"
    );

    type_keys(&mut app, "mai");
    assert_eq!(mention_paths(&app), ["src/main.rs"]);
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.input(), "explain @src/main.rs ");
    assert!(app.mention_matches().is_empty(), "inserting closes it");

    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "explain @src/main.rs "
    ));
}

#[test]
fn test_tab_on_a_folder_stays_in_the_picker() {
    let (_dir, mut app, _rx) = app_with_workspace();
    type_keys(&mut app, "@sr");
    assert_eq!(mention_paths(&app)[0], "src/");
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input(), "@src/");
    assert_eq!(mention_paths(&app), ["src/", "src/lib.rs", "src/main.rs"]);

    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Down));
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input(), "@src/main.rs ");
    assert_eq!(app.cursor_pos(), app.input().len());
}

#[test]
fn test_esc_closes_the_picker_for_that_word() {
    let (_dir, mut app, _rx) = app_with_workspace();
    type_keys(&mut app, "@src");
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.mention_matches().is_empty());
    type_keys(&mut app, "/");
    assert!(app.mention_matches().is_empty(), "same word stays closed");

    type_keys(&mut app, " @READ");
    assert_eq!(mention_paths(&app), ["README.md"], "a new word opens it");
}

#[test]
fn test_an_approval_hides_the_picker_and_takes_y_a_n() {
    let (_dir, mut app, mut rx) = app_with_workspace();
    type_keys(&mut app, "@");
    assert!(!app.mention_matches().is_empty());

    app.process_command(approval_request("req-1", "`cargo test` commands"));
    assert!(app.mention_matches().is_empty(), "the modal covers it");
    app.handle_key(key(KeyCode::Char('a')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ApproveAlways(id)) if id == "req-1"));
    assert_eq!(app.input(), "@", "the answer is not typed");
    assert!(!app.mention_matches().is_empty(), "the picker comes back");
}

#[test]
fn test_email_addresses_do_not_open_the_picker() {
    let (_dir, mut app, _rx) = app_with_workspace();
    type_keys(&mut app, "mail me@src");
    assert!(app.mention_matches().is_empty());
}

// ========================================================================
// Pasted images
// ========================================================================

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

fn paste_image(app: &mut App) {
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(PNG.to_vec())));
}

fn sent_images(rx: &mut mpsc::Receiver<UiEvent>) -> Option<(String, usize)> {
    match rx.try_recv() {
        Ok(UiEvent::UserMessage { text, images, .. }) => Some((text, images.len())),
        _ => None,
    }
}

#[test]
fn test_ctrl_v_asks_the_terminal_for_the_clipboard() {
    let mut app = App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(app.take_paste_request());
    assert!(!app.take_paste_request(), "once");
    assert!(app.input().is_empty(), "Ctrl+V types nothing");
}

#[test]
fn test_pasted_image_shows_a_chip_and_is_sent_with_the_message() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    type_keys(&mut app, "what is");
    paste_image(&mut app);
    assert_eq!(app.input(), "what is [image 1] ");
    assert_eq!(app.attached_images(), 1);
    type_keys(&mut app, "this?");
    app.handle_key(KeyEvent::from(KeyCode::Enter));

    assert_eq!(
        sent_images(&mut rx),
        Some(("what is [image 1] this?".to_string(), 1))
    );
    let shown: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(shown, ["what is [image 1] this?"]);
    assert_eq!(app.attached_images(), 0, "the next message starts empty");
}

#[test]
fn test_deleting_the_chip_drops_the_image() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    paste_image(&mut app);
    paste_image(&mut app);
    assert_eq!(app.input(), "[image 1] [image 2] ");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    paste_image(&mut app);
    assert_eq!(
        app.input(),
        "[image 1] ",
        "numbering restarts once chips are gone"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "no picture");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(sent_images(&mut rx), Some(("no picture".to_string(), 0)));
}

#[test]
fn test_images_that_are_too_big_or_too_many_are_refused() {
    let mut app = App::new();
    let mut huge = PNG.to_vec();
    huge.resize(images::MAX_IMAGE_BYTES + 1, 0);
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(huge)));
    assert!(app.input().is_empty());
    let (problem, _) = app.notification().unwrap();
    assert!(problem.contains("the limit is 3.75 MiB"), "{problem}");

    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Image(b"GIF8".to_vec())));
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("PNG, JPEG, GIF and WebP"));

    for _ in 0..images::MAX_IMAGES {
        paste_image(&mut app);
    }
    paste_image(&mut app);
    assert_eq!(app.attached_images(), images::MAX_IMAGES);
    assert!(app.notification().unwrap().0.contains("at most 5 images"));
}

#[test]
fn test_images_for_a_busy_agent_are_queued() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.state = AppState::Processing;
    paste_image(&mut app);
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(app
        .notification()
        .unwrap()
        .0
        .starts_with("Queued: the agent gets the images"));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { shown: false, images, .. }) if images.len() == 1
    ));
}

#[test]
fn test_pasting_an_image_path_attaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, PNG).unwrap();
    let mut app = App::new();
    app.handle_paste(&format!("'{}'", path.display()));
    assert_eq!(app.input(), "[image 1] ");
    assert_eq!(app.attached_images(), 1);
}

#[test]
fn test_pasted_text_keeps_its_newlines_instead_of_sending() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.handle_paste("line one\r\nline two");
    assert_eq!(app.input(), "line one\nline two");
    assert!(rx.try_recv().is_err(), "nothing was sent");

    app.state = AppState::LinkCode;
    app.input.clear();
    app.cursor_pos = 0;
    app.handle_paste("  code-123\n");
    assert_eq!(app.input(), "code-123");
}

#[test]
fn test_clipboard_errors_are_reported() {
    let mut app = App::new();
    app.paste_clipboard(Err(std::io::Error::other("no tool")));
    assert_eq!(
        app.notification().unwrap().0,
        "Cannot read the clipboard: no tool"
    );
    app.paste_clipboard(Ok(crate::clipboard::Clipboard::Text("hi".into())));
    assert_eq!(app.input(), "hi");
}

// ========================================================================
// Ctrl+G editor
// ========================================================================

fn ctrl_g(app: &mut App) -> std::path::PathBuf {
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    app.take_editor_request()
        .expect("Ctrl+G asks for the editor")
}

#[test]
fn test_ctrl_g_edits_the_input_in_the_editor() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    type_keys(&mut app, "draft");
    let path = ctrl_g(&mut app);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "draft");

    std::fs::write(&path, "line one\nline two\n\n").unwrap();
    app.editor_closed(path.clone(), &Ok(()));

    assert_eq!(app.input(), "line one\nline two");
    assert_eq!(app.cursor_pos(), app.input().len());
    assert!(!path.exists(), "the temp file is removed");
    assert!(rx.try_recv().is_err(), "not a permanent-agent edit");

    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "line one\nline two"
    ));
}

#[test]
fn test_a_failed_editor_keeps_the_input() {
    let mut app = App::new();
    type_keys(&mut app, "keep me");
    let path = ctrl_g(&mut app);
    std::fs::write(&path, "half-written").unwrap();
    app.editor_closed(path.clone(), &Err(std::io::Error::other("exit status: 1")));

    assert_eq!(app.input(), "keep me");
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("the editor failed: exit status: 1"));
    assert!(!path.exists());
}

#[test]
fn test_ctrl_g_with_empty_input_starts_an_empty_file() {
    let mut app = App::new();
    let path = ctrl_g(&mut app);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    std::fs::write(&path, "@src/main.rs explain").unwrap();
    app.editor_closed(path, &Ok(()));
    assert_eq!(app.input(), "@src/main.rs explain");
}

fn new_record(app: &mut App, seq: u64) {
    app.process_command(UiCommand::NewRecord(record_summary(seq)));
}

use super::detail_tests::record as record_summary;

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

fn set_records(app: &mut App, seqs: impl IntoIterator<Item = u64>) {
    let records = seqs.into_iter().map(record_summary).collect();
    app.process_command(UiCommand::SetRecords(records));
}

#[test]
fn test_set_records_replaces_existing_records_and_follows_newest() {
    let mut app = App::new();
    for seq in 0..5 {
        new_record(&mut app, seq);
    }
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Up));
    assert_eq!(selected_seq(&app), 3);

    set_records(&mut app, 100..103);
    let order: Vec<u64> = app.records().iter().map(|r| r.seq).collect();
    assert_eq!(order, [100, 101, 102]);
    assert_eq!(selected_seq(&app), 102);

    new_record(&mut app, 103);
    assert_eq!(selected_seq(&app), 103);
}

#[test]
fn test_set_records_empty_clears_records() {
    let mut app = App::new();
    new_record(&mut app, 1);
    set_records(&mut app, []);
    assert!(app.records().is_empty());
    assert!(app.selected_record_data().is_none());
}

#[test]
fn test_set_records_over_cap_keeps_newest() {
    let mut app = App::new();
    let cap = u64::try_from(MAX_RECORDS).unwrap();
    set_records(&mut app, 0..cap + 5);
    assert_eq!(app.records().len(), MAX_RECORDS);
    assert_eq!(app.records().front().unwrap().seq, 5);
    assert_eq!(app.records().back().unwrap().seq, cap + 4);
    assert_eq!(selected_seq(&app), cap + 4);
}

#[test]
fn test_records_cap_is_well_above_a_hundred() {
    let mut app = App::new();
    for seq in 0..150 {
        new_record(&mut app, seq);
    }
    assert_eq!(app.records().len(), 150);
}

// ========================================================================
// Session modes and plans
// ========================================================================

fn app_with_events() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    (App::new().with_event_sender(tx), rx)
}

fn sample_plan() -> crate::events::PlanView {
    use crate::events::{PlanView, TodoState, TodoView};
    PlanView {
        path: ".aura/plans/auth_0a1b2c3d.plan.md".to_string(),
        name: "Auth".to_string(),
        todos: vec![TodoView {
            content: "Add login".to_string(),
            state: TodoState::Pending,
            assignee: None,
            task: None,
        }],
    }
}

#[test]
fn test_mode_commands_request_a_switch() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(agent_id)) if agent_id == "aa"));
    assert_eq!(app.hud().mode, None, "badge waits for the kernel");

    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Plan));
    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(agent_id)) if agent_id == "aa"));
    assert!(app.notification().is_none(), "plan mode toggles off");

    app.handle_command("/mode ask");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::ReadOnly,
            ..
        })
    ));

    app.handle_command("/mode nonsense");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((_, NotificationType::Warning))
    ));
}

#[test]
fn test_switching_to_the_current_mode_sends_nothing() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Agent));
    app.handle_command("/mode agent");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((text, NotificationType::Warning)) if text.contains("Already in agent mode")
    ));
}

#[test]
fn test_plan_works_mid_turn_but_build_waits() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    app.state = AppState::Processing;

    app.handle_command("/build");
    assert!(rx.try_recv().is_err());
    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
}

#[test]
fn test_plan_with_a_message_sends_it_in_plan_mode() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));

    // Even a build phrase after `/plan` is a request for a plan.
    submit(&mut app, "/plan implement auth");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode { agent_id, mode: crate::PermissionMode::Plan }) if agent_id == "aa"
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { agent_id, text, shown: true, .. })
            if agent_id == "aa" && text == "implement auth"
    ));
    assert_eq!(app.state(), AppState::Processing);
    assert_eq!(app.messages().back().unwrap().content(), "implement auth");

    // Already in plan mode, it stays there rather than toggling out.
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    app.state = AppState::Idle;
    submit(&mut app, "/PLAN  add\nlogout ");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::Plan,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "add\nlogout"
    ));
    assert!(app.notification().is_none());
}

#[test]
fn test_plan_message_parsing() {
    use super::commands::{plan_ranges, strip_plan};
    let ranges = |text: &str| -> Vec<(usize, usize)> {
        plan_ranges(text).iter().map(|r| (r.start, r.end)).collect()
    };
    assert_eq!(ranges("/plan"), [(0, 5)]);
    assert_eq!(ranges("/Plan do it"), [(0, 5)]);
    assert_eq!(ranges("fix it /plan"), [(7, 12)]);
    assert_eq!(ranges("a /plan b\n/PLAN"), [(2, 7), (10, 15)]);
    for plain in [
        "/planner",
        "/pla",
        "src/plan",
        "x /plan/y",
        "/plé",
        "/build a /plan",
    ] {
        assert!(plan_ranges(plain).is_empty(), "{plain}");
    }
    assert_eq!(strip_plan("/plan").as_deref(), Some(""));
    assert_eq!(strip_plan("  /plan   ").as_deref(), Some(""));
    assert_eq!(strip_plan("/plan fix it ").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix /plan it").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix it /plan").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix it"), None);
    assert_eq!(strip_plan("/planfix"), None);
}

#[test]
fn test_plan_anywhere_in_a_message_sends_it_in_plan_mode() {
    let (mut app, mut rx) = app_with_events();
    submit(&mut app, "add a logout button /plan");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::Plan,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "add a logout button"
    ));
}

#[test]
fn test_bare_plan_still_toggles() {
    let (mut app, mut rx) = app_with_events();
    submit(&mut app, "/plan ");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
    assert!(rx.try_recv().is_err());
    submit(&mut app, " /plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_build_needs_a_plan() {
    let (mut app, mut rx) = app_with_events();
    app.handle_command("/build");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((text, NotificationType::Warning)) if text.contains("No plan")
    ));

    app.handle_command("/build .aura/plans/x_00000000.plan.md");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: Some(path), .. }) if path == ".aura/plans/x_00000000.plan.md"
    ));

    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    assert_eq!(app.active_plan().map(|p| p.name.as_str()), Some("Auth"));
    app.handle_command("/build");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: None, .. })
    ));
}

#[test]
fn saying_implement_in_plan_mode_builds_the_plan() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));

    // No plan yet: the agent gets the message.
    submit(&mut app, "implement");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "implement"
    ));
    app.state = AppState::Idle;

    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    submit(&mut app, "  Implement it! ");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: None, .. })
    ));

    // Longer messages are feedback on the plan.
    submit(&mut app, "implement it with a trait instead");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage { .. })));
    app.state = AppState::Idle;

    // Outside plan mode "implement" is an ordinary message.
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Agent));
    submit(&mut app, "implement");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage { .. })));
}

#[test]
fn a_plan_for_an_agent_off_screen_leaves_the_active_plan_alone() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("a1".to_string()));
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));

    app.process_command(for_agent("a2", UiCommand::SetPlan(None)));
    assert_eq!(app.active_plan().map(|p| p.name.as_str()), Some("Auth"));

    app.process_command(for_agent("a1", UiCommand::SetPlan(None)));
    assert!(app.active_plan().is_none());
}

/// Whether the Agents panel marks agent `id` as in plan mode.
fn listed_in_plan_mode(app: &App, id: &str) -> bool {
    app.agents()
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
        .plan_mode
}

#[test]
fn set_mode_plan_marks_an_off_screen_agent_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();
    assert!(!listed_in_plan_mode(&app, "aa") && !listed_in_plan_mode(&app, "bb"));

    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "bb"));
    assert!(!listed_in_plan_mode(&app, "aa"), "the active agent is not");
    assert_eq!(app.hud().mode, None, "the chat on screen keeps its mode");

    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Agent),
    ));
    assert!(!listed_in_plan_mode(&app, "bb"));
}

#[test]
fn set_mode_plan_marks_the_on_screen_agent_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();

    app.process_command(for_agent(
        "aa",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "aa"));
    assert!(!listed_in_plan_mode(&app, "bb"));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Plan));

    app.process_command(for_agent(
        "aa",
        UiCommand::SetMode(crate::PermissionMode::Agent),
    ));
    assert!(!listed_in_plan_mode(&app, "aa"));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Agent));
}

#[test]
fn any_mode_but_plan_clears_the_plan_mode_mark() {
    use crate::PermissionMode;
    let (mut app, _rx) = app_with_agents();
    for mode in [PermissionMode::ReadOnly, PermissionMode::Debug] {
        app.process_command(for_agent("bb", UiCommand::SetMode(PermissionMode::Plan)));
        assert!(listed_in_plan_mode(&app, "bb"));
        app.process_command(for_agent("bb", UiCommand::SetMode(mode)));
        assert!(!listed_in_plan_mode(&app, "bb"), "{mode:?}");
    }
}

#[test]
fn set_mode_plan_for_an_unlisted_agent_marks_no_one_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(for_agent(
        "zz",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(app.agents().iter().all(|a| !a.plan_mode));
}

#[test]
fn a_new_agent_list_replaces_the_plan_mode_mark() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "bb"));

    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        agent("bb", "Researcher"),
    ]));
    assert!(!listed_in_plan_mode(&app, "bb"), "the list says it is not");

    app.process_command(UiCommand::SetAgents(vec![
        crate::events::AgentSummary {
            plan_mode: true,
            ..agent("aa", "Terminal Agent")
        },
        agent("bb", "Researcher"),
    ]));
    assert!(listed_in_plan_mode(&app, "aa"), "the list says it is");
    assert!(!listed_in_plan_mode(&app, "bb"));
}

// ========================================================================
// Folding agents over their spawns
// ========================================================================

/// Project `aa` with spawns `bb` (which spawned `cc`) and `dd`, then
/// project `ee`. Rows from line 4: header 4, aa 5, bb 6, cc 7, dd 8.
fn app_with_spawn_tree() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agent_areas();
    let spawned = |id: &str, depth: usize| crate::events::AgentSummary {
        depth,
        ..agent(id, &id.to_uppercase())
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("bb", 1),
        spawned("cc", 2),
        spawned("dd", 1),
        agent("ee", "EE"),
    ]));
    (app, rx)
}

/// Indices of the agents with a row.
fn shown_agents(app: &App) -> Vec<usize> {
    app.project_rows()
        .iter()
        .filter_map(|row| match row {
            ProjectListRow::Agent { index } => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn folding_an_agent_hides_what_it_spawned_and_nested_folds_survive() {
    let (mut app, _rx) = app_with_spawn_tree();
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    assert!(app.agent_has_spawns(0) && app.agent_has_spawns(1));
    assert!(!app.agent_has_spawns(2) && !app.agent_has_spawns(3) && !app.agent_has_spawns(4));

    app.toggle_agent(1);
    assert!(app.agent_folded(1));
    assert_eq!(shown_agents(&app), [0, 1, 3, 4]);
    app.toggle_agent(0);
    assert_eq!(shown_agents(&app), [0, 4]);
    app.toggle_agent(0);
    assert_eq!(shown_agents(&app), [0, 1, 3, 4], "bb stays folded inside");
    app.toggle_agent(1);
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);

    // A leaf has nothing to fold.
    app.toggle_agent(2);
    assert!(!app.agent_folded(2));
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
}

#[test]
fn folding_an_agent_parks_a_hidden_highlight_on_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    let cc = app.row_of_agent(2).expect("cc shown");
    app.selected_agent = cc;
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 },
        "opening keeps the same row"
    );

    // A highlight below the fold follows its row up and back down.
    app.selected_agent = app.row_of_agent(3).expect("dd shown");
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 3 }
    );
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 3 }
    );
}

#[test]
fn space_folds_an_agent_with_spawns_and_a_leaf_folds_its_project() {
    let (mut app, mut rx) = app_with_spawn_tree();
    app.focus = PanelFocus::Swarm;
    app.selected_agent = app.row_of_agent(1).expect("bb shown");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(app.agent_folded(1));
    assert_eq!(shown_agents(&app), [0, 1, 3, 4], "the project stays open");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(!app.agent_folded(1));

    app.selected_agent = app.row_of_agent(3).expect("dd shown");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert!(rx.try_recv().is_err(), "folding switches no agent");
}

#[test]
fn clicking_an_agents_marker_folds_it_and_its_name_opens_it() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_spawn_tree();
    let click = |app: &mut App, column, row| {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
    };

    // bb's marker sits after one space and one level of guides.
    click(&mut app, 6, 6);
    assert!(app.agent_folded(1));
    assert!(rx.try_recv().is_err(), "the marker switches no agent");
    click(&mut app, 8, 6);
    assert!(!app.agent_folded(1));

    // Past the marker the row switches to the agent as before.
    click(&mut app, 10, 6);
    assert!(!app.agent_folded(1));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    // A leaf's icon column is no button.
    click(&mut app, 10, 7);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "cc"));

    // A top-level agent's marker folds it rather than starting a drag.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 5));
    assert!(app.agent_folded(0));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 12));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 12));
    assert!(!app.project_dragging());
    assert_eq!(agent_ids(&app), ["aa", "bb", "cc", "dd", "ee"]);
    assert!(rx.try_recv().is_err());
}

#[test]
fn switching_to_a_hidden_subagent_opens_the_agents_above_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.toggle_agent(1);
    app.toggle_agent(0);
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 2 }
    );

    // Stepping onto one from the keys opens it too.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.toggle_agent(0);
    app.switch_agent_by(1);
    assert!(!app.agent_folded(0));
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn a_folded_agents_hidden_active_spawn_parks_the_highlight_on_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    app.toggle_agent(1);
    app.select_visible_active();
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn set_agents_drops_folds_of_agents_gone_or_without_spawns() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.toggle_agent(1);
    app.toggle_agent(0);
    assert_eq!(app.collapsed_agents.len(), 2);

    let spawned = |id: &str| crate::events::AgentSummary {
        depth: 1,
        ..agent(id, id)
    };
    // cc is gone, so bb has nothing left to fold.
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("bb"),
        spawned("dd"),
        agent("ee", "EE"),
    ]));
    assert_eq!(
        app.collapsed_agents,
        HashSet::from(["aa".to_string()]),
        "aa still folds over bb and dd"
    );

    app.process_command(UiCommand::SetAgents(vec![agent("ee", "EE")]));
    assert!(app.collapsed_agents.is_empty());
}

#[test]
fn clicking_a_fold_marker_keeps_a_wheel_scrolled_list_put() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, _rx) = app_with_agent_areas();
    let spawned = |id: &str, depth: usize| crate::events::AgentSummary {
        depth,
        ..agent(id, id)
    };
    // Rows: header 0, aa 1, a1 2, a2 3, a3 4, bb 5, b1 6.
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("a1", 1),
        spawned("a2", 1),
        spawned("a3", 1),
        spawned("bb", 1),
        spawned("b1", 2),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.set_agent_rows_area(Rect::new(1, 4, 18, 3));
    app.sync_agents_scroll(3);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    app.sync_agents_scroll(3);
    assert_eq!(app.agents_scroll(), 3, "a2, a3 and bb on screen");

    // bb is on the third row; its marker after one level of guides.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 6, 6));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 6, 6));
    assert!(app.agent_folded(4));
    app.sync_agents_scroll(3);
    assert_eq!(app.agents_scroll(), 3);
}

#[test]
fn shift_space_folds_the_project_from_any_agent() {
    let (mut app, mut rx) = app_with_spawn_tree();
    app.focus = PanelFocus::Swarm;
    let shift_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT);
    for agent in [0, 1] {
        app.selected_agent = app.row_of_agent(agent).expect("shown");
        app.handle_key(shift_space);
        assert!(!app.agent_folded(agent), "the agent itself stays open");
        assert!(
            matches!(
                app.project_rows()[0],
                ProjectListRow::Project {
                    expanded: false,
                    ..
                }
            ),
            "agent {agent} folds its project"
        );
        // The highlight parks on the header, so Shift+Space opens it.
        app.handle_key(shift_space);
        assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    }
    assert!(rx.try_recv().is_err(), "folding switches no agent");
}

#[test]
fn a_busy_agents_ring_is_its_fold_button() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_spawn_tree();
    let mut agents = app.agents().to_vec();
    agents[1].running = true;
    app.process_command(UiCommand::SetAgents(agents));
    let click = |app: &mut App, column| {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, 6));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, 6));
    };
    // bb's ring takes offsets 5 and 6 of the rows, starting at column 1,
    // and the spare column after it offset 7; no arrow is drawn in either
    // state, but all three fold and open it.
    click(&mut app, 6);
    assert!(app.agent_folded(1), "the ring folds an open agent");
    click(&mut app, 6);
    assert!(!app.agent_folded(1), "and opens a folded one");
    click(&mut app, 8);
    assert!(app.agent_folded(1));
    click(&mut app, 8);
    assert!(!app.agent_folded(1));
    assert!(rx.try_recv().is_err(), "the ring switches no agent");
    // Past the icon column the row still switches to the agent.
    click(&mut app, 10);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn opening_a_fold_brings_a_parked_highlight_back_to_the_active_spawn() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    app.selected_agent = app.row_of_agent(2).expect("cc shown");
    app.toggle_agent(0);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 0 },
        "parked on aa"
    );
    app.toggle_agent(0);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 2 },
        "back on cc"
    );

    // With the active agent outside the fold, the highlight stays put.
    app.process_command(UiCommand::SetActiveAgent("ee".to_string()));
    app.selected_agent = app.row_of_agent(1).expect("bb shown");
    app.toggle_agent(1);
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn the_folded_agent_over_the_active_one_stands_for_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    assert_eq!(app.shown_active_agent(), Some(2));
    app.toggle_agent(1);
    assert_eq!(app.shown_active_agent(), Some(1));
    app.toggle_agent(0);
    assert_eq!(app.shown_active_agent(), Some(0), "the nearest one shown");
    app.toggle_selected_project();
    assert_eq!(app.shown_active_agent(), None, "under a folded project");
}

/// An app whose chat holds more than fits, drawn once on a 90 × 30 screen.
fn overflowing_chat() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = App::new();
    for i in 0..60 {
        app.add_message(Message::new(MessageRole::Assistant, &format!("reply {i}")));
    }
    let terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    (app, terminal)
}

fn draw(app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>) {
    let theme = crate::Theme::by_name("cyber");
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
}

/// The screen cell where `text` is drawn, if it is.
fn cell_of(
    terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
    text: &str,
) -> Option<(u16, u16)> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height).find_map(|y| {
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        let at = row.find(text)?;
        let x = u16::try_from(row[..at].chars().count()).ok()?;
        Some((x, y))
    })
}

fn click_at(app: &mut App, (column, row): (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEventKind};
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
}

#[test]
fn clicking_the_earlier_lines_row_of_a_boot_console_loads_them() {
    use crate::components::{
        BootConsole, LIVE_BOOT_LOG_LINES, MAX_BOOT_LOG_LINES, MORE_PAGE_LINES,
    };
    use crate::events::{BootLogLine, BootUpdate};

    let mut console = BootConsole::new("boot-1", "Moving to the cloud");
    console.apply(BootUpdate::Log(
        (0..MAX_BOOT_LOG_LINES + 100)
            .map(|i| BootLogLine {
                stderr: false,
                text: format!("build line {i}"),
            })
            .collect(),
    ));
    let mut app = App::new();
    app.add_message(Message::boot(console));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let hidden = MAX_BOOT_LOG_LINES + 100 - LIVE_BOOT_LOG_LINES;
    let row = cell_of(&terminal, &format!("… {hidden} earlier lines"))
        .expect("the earlier lines row is on screen");

    click_at(&mut app, row);
    let shown = |app: &App| app.messages()[0].boot_console().unwrap().log_shown();
    assert_eq!(shown(&app), LIVE_BOOT_LOG_LINES + MORE_PAGE_LINES);

    // Past what memory holds, the rest comes back from the spill file.
    for _ in 0..3 {
        app.messages
            .back_mut()
            .unwrap()
            .show_more(crate::components::MoreSection::BootLog);
    }
    let console = app.messages()[0].boot_console().unwrap();
    assert_eq!(console.hidden(), 0);
    assert_eq!(
        console.earlier.front().map(|l| l.text.as_str()),
        Some("build line 0")
    );
}

#[test]
fn clicking_a_finished_boot_consoles_fold_opens_and_closes_it() {
    use crate::components::BootConsole;
    use crate::events::{BootLogLine, BootStageState, BootUpdate};

    let mut console = BootConsole::new("boot-1", "Moving to the cloud");
    console.apply(BootUpdate::Stage {
        key: "create".into(),
        label: "Creating machine".into(),
        state: BootStageState::Done { ms: 1200 },
    });
    console.apply(BootUpdate::Log(vec![BootLogLine {
        stderr: false,
        text: "booted line".into(),
    }]));
    console.apply(BootUpdate::End {
        ok: true,
        summary: "Now running in the cloud.".into(),
    });
    let mut app = App::new();
    app.add_message(Message::boot(console));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "booted line").is_none(), "folded");

    let hint = cell_of(&terminal, "click to show the boot log").expect("the hint is on screen");
    click_at(&mut app, hint);
    assert!(app.messages()[0].is_expanded(), "the hint opens it");
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "booted line").is_some());
    assert!(cell_of(&terminal, "Creating machine").is_some());

    let summary = cell_of(&terminal, "Now running in the cloud.").expect("summary on screen");
    click_at(&mut app, summary);
    assert!(!app.messages()[0].is_expanded(), "the outcome closes it");
    draw(&mut app, &mut terminal);

    let header = cell_of(&terminal, "Moving to the cloud").expect("header on screen");
    click_at(&mut app, header);
    assert!(app.messages()[0].is_expanded(), "the header opens it");
}

#[test]
fn clicking_the_more_lines_row_of_a_tool_result_shows_the_rest() {
    let output: Vec<String> = (0..250).map(|i| format!("result line {i}")).collect();
    let mut app = App::new();
    app.add_message(Message::activity(
        Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            args: r#"{"path":"big.txt"}"#.to_string(),
            status: ToolStatus::Success,
            result: Some(output.join("\n")),
            retry: None,
            started: None,
            duration_ms: Some(10),
            diff: None,
            policy: None,
            awaiting_approval: false,
            live: crate::components::LiveOutput::default(),
        }),
        true,
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "result line 249").is_none());
    let row = cell_of(&terminal, "… 50 more lines").expect("the more lines row is on screen");

    click_at(&mut app, row);
    draw(&mut app, &mut terminal);
    assert!(cell_of(&terminal, "result line 249").is_some());
    assert!(cell_of(&terminal, "more lines").is_none());
}

#[test]
fn test_dragging_the_chat_scrollbar_scrolls_without_selecting() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert_eq!(bar.target, scrollbar::ScrollTarget::Chat);
    let chat = app.areas.chat.expect("the chat is drawn");
    assert_eq!(bar.track.x, chat.right() - 1, "on the right border");
    assert_eq!(app.scroll_offset(), 0, "at the bottom");

    // The thumb sits at the bottom; take it there and drag it to the top.
    let (column, bottom) = (bar.track.x, bar.track.bottom() - 1);
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        column,
        bottom,
    ));
    assert!(app.selection().is_none(), "no text selection starts");
    assert!(app.selecting.is_none());
    assert_eq!(app.scroll_offset(), 0, "pressing the thumb leaves it");

    let moved = app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column,
        bar.track.y,
    ));
    assert!(moved, "the chat scrolled, so there is a frame to draw");
    assert_eq!(app.scroll_offset(), bar.total - bar.visible, "at the top");
    draw(&mut app, &mut terminal);
    assert_eq!(app.areas.scrollbar.map(|bar| bar.first), Some(0));

    // Halfway down the track is halfway through the conversation.
    let middle = bar.track.y + bar.track.height / 2;
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column,
        middle,
    ));
    let first = (bar.total - bar.visible) - app.scroll_offset();
    let half = bar.total / 2;
    assert!(
        first.abs_diff(half) <= bar.total / 4,
        "{first} of {}",
        bar.total
    );
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, middle));
    assert!(app.selection().is_none());

    // Released, a drag over the chat is the selection's again.
    let offset = app.scroll_offset();
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column,
        bottom,
    ));
    assert_eq!(app.scroll_offset(), offset);

    // A press on the track away from the thumb jumps it there.
    draw(&mut app, &mut terminal);
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        column,
        bottom,
    ));
    assert_eq!(app.scroll_offset(), 0, "back at the bottom");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, bottom));
    assert!(app.selection().is_none());
}

#[test]
fn test_the_blank_columns_left_of_the_chat_scrollbar_grab_it_too() {
    use crossterm::event::MouseButton::Left;
    use crossterm::event::MouseEventKind::{Down, Drag, Up};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    let rows = app.areas.chat_rows.expect("the chat's rows");
    assert!(
        bar.track.x - rows.right() > 2,
        "more blank columns than it reaches: {rows:?} beside {bar:?}"
    );
    let (top, bottom) = (bar.track.y, bar.track.bottom() - 1);
    let farthest = bar.track.x - 2;
    let press = |app: &mut App, kind, column, row| app.handle_mouse(mouse(kind, column, row));

    // Two columns in, a press at the top jumps the thumb there and a
    // drag takes it back down.
    press(&mut app, Down(Left), farthest, top);
    assert!(app.scrollbar_held());
    assert!(app.selection().is_none(), "no text selection starts");
    assert_eq!(app.scroll_offset(), bar.last, "at the top");
    press(&mut app, Drag(Left), farthest, bottom);
    assert_eq!(app.scroll_offset(), 0, "back at the bottom");
    press(&mut app, Up(Left), farthest, bottom);
    assert!(!app.scrollbar_held());

    // A column further in is the chat's, blank or not.
    draw(&mut app, &mut terminal);
    let inside = farthest - 1;
    press(&mut app, Down(Left), inside, top);
    assert!(!app.scrollbar_held());
    assert_eq!(app.scroll_offset(), 0, "the chat stayed put");
    press(&mut app, Up(Left), inside, top);
}

#[test]
fn test_a_chat_that_fits_has_no_scrollbar_to_grab() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = App::new();
    app.add_message(Message::new(MessageRole::Assistant, "short"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(app.areas.scrollbar.is_none());
    let chat = app.areas.chat.expect("the chat is drawn");
    let rows = app.areas.chat_rows.expect("the chat's rows");
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        chat.right() - 1,
        rows.y + 1,
    ));
    assert!(!app.scrollbar_held());
}

#[test]
fn test_the_thumb_at_the_bottom_of_a_long_chat_shows_its_end_and_follows_it() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = App::new();
    for i in 0..1000 {
        app.add_message(Message::new(MessageRole::Assistant, &format!("reply {i}")));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    // Long enough that the track's last row stands for more rows than
    // fit on screen.
    let track = usize::from(bar.track.height);
    assert!(bar.total > bar.visible * track, "{bar:?}");
    let (column, top, bottom) = (bar.track.x, bar.track.y, bar.track.bottom() - 1);
    let press = |app: &mut App, kind, row| app.handle_mouse(mouse(kind, column, row));

    // Taken at the bottom, dragged to the top and back down.
    press(&mut app, MouseEventKind::Down(MouseButton::Left), bottom);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), top);
    assert_eq!(app.scroll_offset(), bar.last, "at the top");
    draw(&mut app, &mut terminal);
    press(&mut app, MouseEventKind::Drag(MouseButton::Left), bottom);
    assert_eq!(app.scroll_offset(), 0, "at the very bottom");
    press(&mut app, MouseEventKind::Up(MouseButton::Left), bottom);
    app.add_message(Message::new(MessageRole::Assistant, "the latest"));
    draw(&mut app, &mut terminal);
    assert_eq!(app.scroll_offset(), 0, "following what comes in");

    // A few rows up the thumb is still drawn on the last row. Pressed
    // there it stays put; moved along that row it goes to the end.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, column - 5, top + 2));
    draw(&mut app, &mut terminal);
    let offset = app.scroll_offset();
    assert!(offset > 0);
    press(&mut app, MouseEventKind::Down(MouseButton::Left), bottom);
    assert_eq!(app.scroll_offset(), offset, "pressing the thumb leaves it");
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column - 1,
        bottom,
    ));
    assert_eq!(app.scroll_offset(), 0);
    press(&mut app, MouseEventKind::Up(MouseButton::Left), bottom);
}

#[test]
fn test_a_menu_over_the_chat_keeps_the_rows_of_the_scrollbar_it_covers() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut terminal) = overflowing_chat();
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert_eq!(bar.open, bar.track.height, "nothing over it");

    // The slash menu opens over the bottom of the chat, its right border
    // on the scrollbar's column.
    for c in "/lo".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the chat overflows");
    assert!(bar.open > 0 && bar.open < bar.track.height, "{bar:?}");
    let covered = bar.track.y + bar.open;
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.track.x,
        covered,
    ));
    assert!(!app.scrollbar_held(), "the menu's border");
    assert_eq!(app.scroll_offset(), 0);
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        bar.track.x,
        covered,
    ));

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.track.x,
        bar.track.y,
    ));
    assert!(app.scrollbar_held(), "above the menu");
    assert_eq!(app.scroll_offset(), bar.last, "jumped to the top");
}

/// An app showing a 400-line file over the chat, drawn once on a 90 × 30
/// screen, and the directory the file is in.
fn long_file_over_the_chat() -> (
    tempfile::TempDir,
    App,
    ratatui::Terminal<ratatui::backend::TestBackend>,
) {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (1..=400).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.path().join("long.txt"), text).unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    (dir, app, terminal)
}

#[test]
fn test_dragging_the_scrollbar_of_a_file_over_the_chat_scrolls_the_file() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (_dir, mut app, mut terminal) = long_file_over_the_chat();
    let bar = app.areas.scrollbar.expect("the file overflows");
    assert_eq!(bar.target, scrollbar::ScrollTarget::Detail);
    let (column, top, bottom) = (bar.track.x, bar.track.y, bar.track.bottom() - 1);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, top));
    assert_eq!(app.detail_scroll(), 0, "pressing the thumb leaves it");
    app.handle_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        column,
        bottom,
    ));
    assert_eq!(app.detail_scroll(), bar.last, "at the end of the file");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, bottom));
    assert!(app.selection().is_none());
    draw(&mut app, &mut terminal);
    let bar = app.areas.scrollbar.expect("the file overflows");
    assert_eq!(bar.first, bar.last, "the cursor came along");

    // A press on the track away from the thumb jumps it there.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, top));
    assert_eq!(app.detail_scroll(), 0, "back at the top");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, top));
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
}

#[test]
fn test_the_scrollbar_of_a_file_reaches_no_further_than_its_text() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (_dir, mut app, mut terminal) = long_file_over_the_chat();
    let bar = app.areas.scrollbar.expect("the file overflows");
    let rows = app.areas.chat_rows.expect("the file's rows");
    assert_eq!(bar.track.x - rows.right(), 1, "a column of padding");
    let (padding, text) = (bar.track.x - 1, bar.track.x - 2);
    let (top, bottom) = (bar.track.y, bar.track.bottom() - 1);

    // The padding grabs it: pressed at the bottom, the thumb jumps there.
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        padding,
        bottom,
    ));
    assert!(app.scrollbar_held());
    assert_eq!(app.detail_scroll(), bar.last, "at the end of the file");
    app.handle_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        padding,
        bottom,
    ));

    // The text's last column is the file's.
    draw(&mut app, &mut terminal);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), text, top));
    assert!(!app.scrollbar_held());
    assert_eq!(app.detail_scroll(), bar.last, "where it was");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), text, top));
}

// ========================================================================
// Multi-line input and the stop button
// ========================================================================

/// The keys that break the line instead of sending it.
const NEWLINE_KEYS: [(KeyCode, KeyModifiers); 3] = [
    (KeyCode::Enter, KeyModifiers::SHIFT),
    (KeyCode::Enter, KeyModifiers::ALT),
    (KeyCode::Char('j'), KeyModifiers::CONTROL),
];

fn shift_enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
}

#[test]
fn newline_keys_break_the_line_at_the_cursor_without_sending() {
    for (code, modifiers) in NEWLINE_KEYS {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        type_chars(&mut app, "ab");
        app.handle_key(key(KeyCode::Left));

        app.handle_key(KeyEvent::new(code, modifiers));
        assert_eq!(app.input(), "a\nb", "{code:?} {modifiers:?}");
        assert_eq!(app.cursor_pos(), 2, "just past the break");

        app.handle_key(key(KeyCode::End));
        app.handle_key(KeyEvent::new(code, modifiers));
        assert_eq!(app.input(), "a\nb\n", "{code:?} {modifiers:?}");
        assert_eq!(app.cursor_pos(), 4);

        assert!(
            rx.try_recv().is_err(),
            "nothing sent: {code:?} {modifiers:?}"
        );
        assert!(app.messages().is_empty());
        assert_eq!(app.state(), AppState::Idle);
    }
}

#[test]
fn plain_enter_still_sends_a_multi_line_message() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    type_chars(&mut app, "one");
    app.handle_key(shift_enter());
    type_chars(&mut app, "two");

    app.handle_key(key(KeyCode::Enter));

    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "one\ntwo"
    ));
    assert_eq!(app.input(), "");
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn newline_keys_on_an_empty_line_break_it_from_any_panel() {
    for (code, modifiers) in NEWLINE_KEYS {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        assert_eq!(app.focus, PanelFocus::Chat);
        app.handle_key(KeyEvent::new(code, modifiers));
        assert_eq!(app.input(), "\n", "{code:?} {modifiers:?}");
        assert_eq!(app.cursor_pos(), 1);
        assert!(rx.try_recv().is_err());
    }

    // Typing goes to the prompt from any panel, and so does a line break,
    // even before anything is typed.
    for focus in [PanelFocus::Records, PanelFocus::Swarm, PanelFocus::Sandbox] {
        let mut app = App::new();
        app.focus = focus;
        app.handle_key(shift_enter());
        assert_eq!(app.input(), "\n", "{focus:?}");
        type_chars(&mut app, "x");
        app.handle_key(shift_enter());
        assert_eq!(app.input(), "\nx\n", "{focus:?}");
    }
}

#[test]
fn newline_keys_do_not_break_login_prompts() {
    let mut app = App::new();
    app.state = AppState::LoginEmail;
    type_chars(&mut app, "me@x.io");

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    assert_eq!(app.input(), "me@x.io", "Ctrl+J is not typed");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    assert_eq!(app.state(), AppState::LoginPassword, "Enter submits");
    assert_eq!(app.input(), "");
}

#[test]
fn up_and_down_move_between_lines_at_the_same_column() {
    let mut app = App::new();
    // "abc\nx\nhello": lines start at bytes 0, 4 and 6.
    type_chars(&mut app, "abc");
    app.handle_key(shift_enter());
    type_chars(&mut app, "x");
    app.handle_key(shift_enter());
    type_chars(&mut app, "hello");
    assert_eq!(app.input(), "abc\nx\nhello");
    assert_eq!(app.cursor_pos(), 11, "at the end of \"hello\", column 5");

    let mut stops = Vec::new();
    for code in [KeyCode::Up, KeyCode::Up, KeyCode::Down, KeyCode::Down] {
        app.handle_key(key(code));
        stops.push(app.cursor_pos());
    }
    assert_eq!(app.input(), "abc\nx\nhello", "the text stays");
    // Up clamps to the end of "x"; the column is then 1 for the rest.
    assert_eq!(stops, [5, 1, 5, 7]);

    // Up on the first line with no history leaves it all as it was.
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!((app.input(), app.cursor_pos()), ("abc\nx\nhello", 1));
}

#[test]
fn up_and_down_recall_history_from_the_first_and_last_lines() {
    let mut app = App::new();
    submit(&mut app, "older");
    app.state = AppState::Idle;
    type_chars(&mut app, "one");
    app.handle_key(shift_enter());
    type_chars(&mut app, "two");
    app.handle_key(key(KeyCode::Enter));
    app.state = AppState::Idle;
    assert_eq!(app.input(), "");

    // Recalled, the two-line message has the cursor at its end; Up walks
    // to its first line before going further back.
    app.handle_key(key(KeyCode::Up));
    assert_eq!((app.input(), app.cursor_pos()), ("one\ntwo", 7));
    app.handle_key(key(KeyCode::Up));
    assert_eq!((app.input(), app.cursor_pos()), ("one\ntwo", 3));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "older");

    app.handle_key(key(KeyCode::Down));
    assert_eq!((app.input(), app.cursor_pos()), ("one\ntwo", 7));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.input(), "", "past the newest entry");
}

#[test]
fn down_on_the_last_line_of_a_typed_draft_keeps_it() {
    let mut app = App::new();
    submit(&mut app, "older");
    app.state = AppState::Idle;
    type_chars(&mut app, "one");
    app.handle_key(shift_enter());
    type_chars(&mut app, "two");
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.cursor_pos(), 6);

    // Not browsing history: Down goes to the end and stays there.
    for _ in 0..2 {
        app.handle_key(key(KeyCode::Down));
        assert_eq!((app.input(), app.cursor_pos()), ("one\ntwo", 7));
    }
    // History is still a key away.
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "older");
}

#[test]
fn enter_on_a_blank_prompt_sends_nothing() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.handle_key(shift_enter());
    type_chars(&mut app, " \t");
    app.handle_key(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err(), "nothing sent");
    assert_eq!(app.state(), AppState::Idle);
    assert_eq!(app.input(), "\n \t", "the blank line stays to edit");
}

#[test]
fn enter_on_a_blank_prompt_sends_the_queued_message_now() {
    let (mut app, mut rx) = app_with_agents();
    submit(&mut app, "first");
    submit(&mut app, "second");
    while rx.try_recv().is_ok() {}
    app.process_command(UiCommand::SetQueue(vec![crate::events::QueuedItemView {
        id: 1,
        text: "second".to_string(),
        editable: true,
    }]));
    app.handle_key(shift_enter());

    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SendQueuedNow(id)) if id == "aa"
    ));
    assert!(rx.try_recv().is_err(), "the blank line is not sent");
}

#[test]
fn a_slash_command_name_ends_at_any_whitespace() {
    let (mut app, _rx) = app_with_models(None);
    type_chars(&mut app, "/model");
    app.handle_key(shift_enter());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::SelectingModel, "\"/model\\n\"");

    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model\tlocal/qwen3:8b\n");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "local/qwen3:8b"
    ));
}

#[test]
fn newline_keys_on_an_empty_line_are_left_to_the_other_chat_tabs() {
    for tab in [ChatTab::Graph, ChatTab::Time, ChatTab::Bench, ChatTab::Log] {
        let mut app = App::new();
        app.show_chat_tab(tab);
        for (code, modifiers) in NEWLINE_KEYS {
            app.handle_key(KeyEvent::new(code, modifiers));
            assert_eq!(app.input(), "", "{tab:?} {code:?} {modifiers:?}");
        }
    }

    // The TERMINAL prompt is a prompt like CHAT.
    let mut app = App::new();
    app.show_chat_tab(ChatTab::Terminal);
    app.handle_key(shift_enter());
    assert_eq!(app.input(), "\n");
}

/// An app whose agent `aa` is mid-turn, drawn once so the stop button's
/// cells are recorded, and the event channel's receiving end.
fn busy_app_with_stop_button() -> (
    App,
    mpsc::Receiver<UiEvent>,
    ratatui::Terminal<ratatui::backend::TestBackend>,
    Rect,
) {
    let (tx, rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::TurnStarted { prompt: None });
    assert!(app.is_busy());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let stop = app
        .input_stop_area()
        .expect("a busy agent shows the button");
    (app, rx, terminal, stop)
}

#[test]
fn clicking_the_stop_button_cancels_the_turn() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut rx, mut terminal, stop) = busy_app_with_stop_button();

    app.handle_mouse(mouse(MouseEventKind::Moved, stop.x + 2, stop.y));
    assert_eq!(app.hover(), Some(HoverTarget::StopTurn));

    let last = stop.right() - 1;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), last, stop.y));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Cancel(id)) if id == "aa"));
    assert_eq!(app.state(), AppState::Idle);
    assert_eq!(app.hover(), None);
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), last, stop.y));

    // Idle, the button is gone and its cells cancel nothing.
    draw(&mut app, &mut terminal);
    assert_eq!(app.input_stop_area(), None);
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        stop.x,
        stop.y,
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn clicks_beside_the_stop_button_do_not_cancel() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut rx, _terminal, stop) = busy_app_with_stop_button();
    for (column, row) in [(stop.x - 1, stop.y), (stop.x, stop.y - 1)] {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
    }
    // Only a left press is a click on it.
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Right),
        stop.x,
        stop.y,
    ));
    app.context_menu = None;
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), stop.x, stop.y));

    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok()).any(|e| matches!(e, UiEvent::Cancel(_))),
        "no cancel"
    );
    assert!(app.is_busy());
}

#[test]
fn the_stop_button_cancels_a_turn_waiting_for_approval() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut rx, mut terminal, _) = busy_app_with_stop_button();
    app.process_command(approval_request("req-1", ""));
    assert_eq!(app.state(), AppState::AwaitingApproval);
    draw(&mut app, &mut terminal);
    let stop = app.input_stop_area().expect("still busy");

    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        stop.x,
        stop.y,
    ));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Cancel(id)) if id == "aa"));
    assert!(app.pending_approval().is_none());
    assert_eq!(app.state(), AppState::Idle);
}

/// The text of the screen `app` draws, a line per row.
fn drawn_text(app: &mut App) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).expect("backend");
    terminal
        .draw(|frame| crate::render(frame, app, &crate::Theme::default()))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn load_chat_of(app: &mut App, id: &str, text: &str) {
    app.process_command(UiCommand::ForAgent {
        agent_id: id.to_string(),
        command: Box::new(UiCommand::LoadConversation(vec![MessageData::new(
            MessageRole::Assistant,
            text,
        )])),
    });
}

#[test]
fn switching_to_an_agent_not_in_memory_is_immediate_and_fades_in_once_loaded() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert_eq!(app.active_agent_id(), "bb", "before the session answers");
    assert!(app.chat_loading());
    assert!(app.chat_animating());
    assert!(app.chat_fading().is_none());

    load_chat_of(&mut app, "bb", "bb's history");
    assert!(!app.chat_loading());
    assert!(app.chat_fading().is_some(), "the history fades in");
    assert!(app.messages().iter().any(|m| m.content() == "bb's history"));
}

#[test]
fn switching_back_to_a_chat_in_memory_shows_it_at_once_without_a_fade() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    load_chat_of(&mut app, "bb", "bb's history");
    app.process_command(UiCommand::SetActiveAgent("bb".into()));

    app.handle_command("/agent 1");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "aa"));
    assert_eq!(app.active_agent_id(), "aa");
    assert!(!app.chat_loading());
    assert!(app.chat_fading().is_none());

    app.process_command(UiCommand::SetActiveAgent("aa".into()));
    app.handle_command("/agent 2");
    assert!(
        matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "bb"),
        "bb's history is held now"
    );
    assert!(!app.chat_loading());
    assert!(app.messages().iter().any(|m| m.content() == "bb's history"));
}

#[test]
fn a_late_answer_to_an_earlier_switch_is_ignored() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    app.handle_command("/agent 1");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "aa"));

    app.process_command(UiCommand::SetActiveAgent("bb".into()));
    assert_eq!(app.active_agent_id(), "aa", "the answer for bb came late");
    app.process_command(UiCommand::SetActiveAgent("aa".into()));
    assert_eq!(app.active_agent_id(), "aa");

    // With nothing awaited, the session's switches apply again.
    app.process_command(UiCommand::SetActiveAgent("bb".into()));
    assert_eq!(app.active_agent_id(), "bb");
}

#[test]
fn a_failed_switch_goes_back_to_the_agent_the_session_kept() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    assert_eq!(app.active_agent_id(), "bb");

    app.process_command(UiCommand::SwitchFailed("bb".into()));
    assert_eq!(app.active_agent_id(), "aa");
    assert!(!app.chat_loading());

    // bb's history was never loaded, so it is asked for again.
    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn chat_ready_ends_the_wait_for_a_live_chat() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    app.process_command(UiCommand::SetActiveAgent("bb".into()));
    assert!(app.chat_loading());

    app.process_command(UiCommand::ForAgent {
        agent_id: "bb".into(),
        command: Box::new(UiCommand::ChatReady),
    });
    assert!(!app.chat_loading());
    assert!(app.chat_fading().is_some());
}

#[test]
fn an_unlisted_agent_waits_for_the_session() {
    let (mut app, mut rx) = app_with_agents();
    app.request_agent_switch("elsewhere".into());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "elsewhere"));
    assert_eq!(app.active_agent_id(), "aa");
    assert!(!app.chat_loading());
}

#[test]
fn a_loading_chat_draws_no_welcome_and_says_it_loads_after_a_moment() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();

    let text = drawn_text(&mut app);
    assert!(!text.contains("Type a message"), "{text}");
    assert!(!text.contains("Loading chat"), "a quick load says nothing");

    app.chat_loading_since = Instant::now().checked_sub(super::run_log::LOADING_DELAY);
    let text = drawn_text(&mut app);
    assert!(text.contains("Loading chat"), "{text}");

    load_chat_of(&mut app, "bb", "bb's history");
    let text = drawn_text(&mut app);
    assert!(text.contains("bb's history"), "{text}");
}

#[test]
fn test_agents_tabs_put_channels_first_and_open_on_projects() {
    let labels = AgentsTab::ALL.map(AgentsTab::label);
    assert_eq!(labels, ["CHANNELS", "PROJECTS", "AGENTS", "ARCHIVE"]);
    assert_eq!(App::new().agents_tab(), AgentsTab::Projects);
}

/// The channel `#zode` (`c1`), coordinated by `coordinator`.
fn zode(coordinator: &str) -> crate::events::ChannelSummary {
    crate::events::ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        title: "Zode release".to_string(),
        coordinator_id: coordinator.to_string(),
        members: vec![agent(coordinator, "Coordinator")],
        ..crate::events::ChannelSummary::default()
    }
}

/// The app with `#zode`, coordinated by `aa` (on screen), listed, and
/// come to CHANNELS, which opened it.
fn app_with_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetChannels(vec![zode("aa")]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.channel_open(), "coming to CHANNELS opens it");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    (app, rx)
}

#[test]
fn test_coming_to_channels_opens_one_and_leaving_the_tab_closes_it() {
    let (mut app, mut rx) = app_with_channel();

    app.handle_key(key(KeyCode::Enter));
    assert!(app.channel_open());
    assert!(rx.try_recv().is_err(), "it is open already");
    assert_eq!(app.agents_tab(), AgentsTab::Channels, "the list stays");

    // Esc and Backspace leave the channel open: the tab is how to leave.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Backspace));
    assert!(app.channel_open());

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(!app.channel_open());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(
        rx.try_recv().is_err(),
        "the coordinator was on screen before"
    );

    // Coming back opens the channel it was left on.
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.channel_open());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
}

/// The app with `#zode` listed, coordinated by `bb`, so `aa`, on screen,
/// is not a member; the agents panel focused on PROJECTS, Left away from
/// CHANNELS.
fn app_with_others_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agents();
    app.process_command(UiCommand::SetChannels(vec![zode("bb")]));
    app.focus = PanelFocus::Swarm;
    (app, rx)
}

#[test]
fn test_leaving_channels_gives_back_the_agent_there_was_before() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(_))));
    // The session confirms the coordinator on screen.
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert!(app.channel_open());

    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(
        matches!(
            rx.try_recv(),
            Ok(UiEvent::SelectAgent(id) | UiEvent::FocusAgent(id)) if id == "aa"
        ),
        "aa, on screen before the channel, comes back"
    );
}

#[test]
fn test_opening_a_channel_shows_its_coordinator_at_once_not_the_agent_before() {
    let (mut app, mut rx) = app_with_others_channel();
    load_chat_of(&mut app, "aa", "aa's history");
    assert!(app.messages().iter().any(|m| m.content() == "aa's history"));

    app.handle_key(key(KeyCode::Left));
    // Asked for before the channel, so the session has it active by then
    // and does not load it twice.
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert_eq!(app.active_agent_id(), "bb", "before the session answers");
    assert!(app.chat_loading(), "waits blank for its history");
    let text = drawn_text(&mut app);
    assert!(!text.contains("aa's history"), "{text}");

    // An answer the session gave for aa before it got the request is old.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(app.active_agent_id(), "bb");
    assert!(app.channel_open());

    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    load_chat_of(&mut app, "bb", "coordinator history");
    assert!(!app.chat_loading());
    assert!(drawn_text(&mut app).contains("coordinator history"));
}

#[test]
fn test_reopening_a_channel_shows_the_coordinators_held_chat_at_once() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    load_chat_of(&mut app, "bb", "coordinator history");
    app.handle_key(key(KeyCode::Right));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    while rx.try_recv().is_ok() {}

    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "bb"));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert!(!app.chat_loading(), "the held chat needs no reload");
    assert!(drawn_text(&mut app).contains("coordinator history"));
}

#[test]
fn test_a_non_member_on_screen_takes_the_user_back_to_projects() {
    let (mut app, mut rx) = app_with_others_channel();
    app.handle_key(key(KeyCode::Left));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(_))));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(_))));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert!(app.channel_open());

    // Once the coordinator came, another agent put on screen is the user
    // going to it.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert!(!app.channel_open());
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloseChannel)));
    assert!(rx.try_recv().is_err(), "nothing is asked for in its place");
}

#[test]
fn test_an_open_channel_fills_the_right_column_with_members_and_info() {
    let (mut app, _rx) = app_with_others_channel();
    // A layout of the user's own: Record shown, the side panel hidden
    // and not maximized, on TRACE.
    app.record_panel_visible = true;
    app.side_expanded = false;
    app.sandbox_panel_visible = false;
    app.side_tab = SidePanelTab::Trace;

    app.handle_key(key(KeyCode::Left));
    assert!(app.channel_open());
    assert!(!app.record_panel_visible(), "no Kernel panel in a channel");
    assert!(app.sandbox_panel_visible(), "shown though the user hid it");
    assert!(app.side_maximized());
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    assert_eq!(
        SidePanelTab::tabs(true),
        [SidePanelTab::ChannelMembers, SidePanelTab::ChannelInfo]
    );

    // Shift+arrows go between the channel's two tabs only.
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);
    assert!(app.handle_side_tab_key(shift(KeyCode::Right)));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelInfo);
    assert!(app.handle_side_tab_key(shift(KeyCode::Right)));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    assert!(app.handle_side_tab_key(shift(KeyCode::Left)));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelInfo);

    // The agent panels stay away, and the user's flags are untouched.
    for command in ["/kernel", "/files", "/inbox", "/members", "/policies"] {
        app.handle_command(command);
        assert!(!app.record_panel_visible(), "{command}");
        assert!(app.sandbox_panel_visible(), "{command}");
    }
    app.toggle_side_expanded();
    app.show_side_tab(SidePanelTab::Trace);
    assert_eq!(app.side_tab(), SidePanelTab::ChannelInfo);
    assert!(
        (
            app.record_panel_visible,
            app.sandbox_panel_visible,
            app.side_expanded
        ) == (true, false, false),
        "the user's flags are untouched"
    );

    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert!(!app.channel_open());
    assert!(app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert!(!app.side_expanded());
    assert!(!app.side_maximized());
    assert_eq!(app.side_tab(), SidePanelTab::Trace);
}

#[test]
fn test_a_channel_dropped_from_the_list_gives_back_the_side_tab() {
    let (mut app, _rx) = app_with_others_channel();
    app.side_tab = SidePanelTab::Tasks;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);

    app.process_command(UiCommand::SetChannels(Vec::new()));
    assert!(!app.channel_open());
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(
        !app.sandbox_panel_visible(),
        "nothing to show beside the list"
    );
}

#[test]
fn test_channel_side_tabs_are_listed_and_reached_only_in_a_channel() {
    let channel_tabs = [SidePanelTab::ChannelMembers, SidePanelTab::ChannelInfo];
    for tab in channel_tabs {
        assert!(!SidePanelTab::tabs(false).contains(&tab));
    }
    for &tab in SidePanelTab::tabs(false) {
        assert!(!tab.next(false).is_channel_tab(), "{tab:?}");
        assert!(!tab.prev(false).is_channel_tab(), "{tab:?}");
    }
    assert_eq!(
        SidePanelTab::ChannelInfo.next(true),
        SidePanelTab::ChannelMembers
    );
    assert_eq!(
        SidePanelTab::Sandbox.next(true),
        SidePanelTab::ChannelMembers
    );

    let mut app = App::new();
    app.show_side_tab(SidePanelTab::Trace);
    app.show_side_tab(SidePanelTab::ChannelMembers);
    app.show_side_tab(SidePanelTab::ChannelInfo);
    assert_eq!(app.side_tab(), SidePanelTab::Trace, "no channel is open");
    let mut shown = Vec::new();
    for _ in 0..SidePanelTab::tabs(false).len() * 2 {
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        shown.push(app.side_tab());
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        app.handle_side_tab_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        shown.push(app.side_tab());
    }
    assert!(!shown.iter().any(|tab| tab.is_channel_tab()), "{shown:?}");
}

#[test]
fn test_a_channel_has_only_the_chat_tab() {
    let (mut app, _rx) = app_with_channel();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.chat_tab(), ChatTab::Chat);

    for tab in ChatTab::ALL {
        app.show_chat_tab(tab);
        assert_eq!(app.chat_tab(), ChatTab::Chat, "{tab:?} is an agent tab");
        app.restore_chat_tab(tab);
        assert_eq!(app.chat_tab(), ChatTab::Chat, "{tab:?} put back");
    }
    app.focus = PanelFocus::Chat;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    for command in ["/log", "/bench"] {
        app.handle_command(command);
    }
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(
        app.chat_tab_layout(80).cells.is_empty(),
        "the bar has no tabs"
    );

    // Back among the agents the tabs come back.
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    app.show_chat_tab(ChatTab::Terminal);
    assert_eq!(app.chat_tab(), ChatTab::Terminal);
    assert!(!app.chat_tab_layout(80).cells.is_empty());
}

#[test]
fn test_opening_a_channel_puts_the_chat_on_chat() {
    let (mut app, _rx) = app_with_agents();
    app.show_chat_tab(ChatTab::Terminal);
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            id: "c1".to_string(),
            slug: "zode".to_string(),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            ..crate::events::ChannelSummary::default()
        },
    ]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert_eq!(
        app.chat_tab(),
        ChatTab::Chat,
        "the channel app has only CHAT"
    );
}

#[test]
fn test_channels_with_nothing_open_hide_the_right_column_and_refuse_to_send() {
    let (mut app, mut rx) = app_with_agents();
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert!(app.app_empty(), "no channel to open");
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert_eq!(app.app_placeholder().as_deref(), Some("Pick a channel"));

    app.input = "hello".to_string();
    app.cursor_pos = app.input.len();
    app.submit_input();
    assert_eq!(app.input(), "hello", "the draft is kept");
    assert!(rx.try_recv().is_err(), "nothing is sent");
    assert!(app
        .notification
        .as_ref()
        .is_some_and(|(text, _)| text.contains("Pick a channel")));

    // The first channel listed opens as it comes.
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            id: "c1".to_string(),
            slug: "zode".to_string(),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            ..crate::events::ChannelSummary::default()
        },
    ]));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenChannel(id)) if id == "c1"));
    assert_eq!(app.app_placeholder().as_deref(), Some("Message #zode"));
    assert!(app.sandbox_panel_visible());
}

#[test]
fn test_members_side_tab_moves_its_highlight_and_switches_nothing() {
    let (_dir, mut app) = app_in_channel("co");
    app.focus = PanelFocus::Sandbox;
    assert_eq!(app.selected_channel_member(), 0);
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.selected_channel_member(), 2, "clamped to the members");
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.selected_channel_member(), 0);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.active_agent_id(), "co", "rows only show");
    assert!(app.channel_open());
}

// ========================================================================
// Channel coordinator chat
// ========================================================================

/// The app of [`app_with_workspace`] with `#zode` open and the chat on
/// `active`; `co` coordinates the channel, `RE36` and `SUB2` work in it.
fn app_in_channel(active: &str) -> (tempfile::TempDir, App) {
    use crate::events::{AgentSummary, ChannelSummary};
    let member = |id: &str, code: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        code: Some(code.to_string()),
        name: name.to_string(),
        depth,
        ..AgentSummary::default()
    };
    let (dir, mut app, _rx) = app_with_workspace();
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![
            member("co", "CO1", "Lead", 0),
            member("re", "RE36", "Reviewer", 1),
            member("sb", "SUB2", "Scout", 2),
        ],
        ..ChannelSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
    app.open_channel_by_id("c1");
    // Opening puts the coordinator on screen; the session confirms it,
    // then the test's agent comes on screen as a member's chat would.
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
    app.focus = PanelFocus::Chat;
    (dir, app)
}

#[test]
fn test_at_in_a_coordinator_chat_lists_members_before_files() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "ask @");
    let paths = mention_paths(&app);
    assert_eq!(paths[..3], ["CO1", "RE36", "SUB2"], "{paths:?}");
    assert!(paths[3..].contains(&"src/main.rs".to_string()), "{paths:?}");
    let roles: Vec<_> = app
        .mention_matches()
        .iter()
        .filter_map(|candidate| candidate.member.as_ref().map(|member| member.role))
        .collect();
    assert_eq!(roles, ["coordinator", "agent", "subagent"]);

    type_keys(&mut app, "rev");
    assert_eq!(mention_paths(&app), ["RE36"], "names match too");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.input(), "ask @RE36 ");
    assert!(app.mention_matches().is_empty(), "inserting closes it");
}

#[test]
fn test_at_matches_member_codes_ignoring_case_and_tab_finishes_them() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "@sub");
    assert_eq!(mention_paths(&app), ["SUB2"]);
    app.handle_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.input(), "@SUB2 ");
}

#[test]
fn test_at_outside_a_coordinator_chat_lists_only_files() {
    let (_dir, mut app) = app_in_channel("re");
    type_keys(&mut app, "@");
    let matches = app.mention_matches();
    assert!(!matches.is_empty());
    assert!(
        matches.iter().all(|candidate| candidate.member.is_none()),
        "{matches:?}"
    );

    app.switch_app(AgentsTab::Projects);
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.process_command(UiCommand::SetSandboxRoot(_dir.path().to_path_buf()));
    app.input.clear();
    app.cursor_pos = 0;
    type_keys(&mut app, "@RE");
    assert_eq!(mention_paths(&app), ["README.md"]);
}

#[test]
fn test_the_chat_bar_names_the_channel_and_has_no_tabs() {
    let (_dir, app) = app_in_channel("co");
    assert_eq!(
        app.channel_chat_title(),
        Some(("#zode".to_string(), String::new()))
    );
    assert!(app.chat_tab_layout(80).cells.is_empty());
}

#[test]
fn test_coordinator_chat_shows_the_channel_posts() {
    use crate::events::{ChannelPostAuthor, ChannelPostView};
    let (_dir, mut app) = app_in_channel("co");
    assert!(app.pane_is_empty());
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 1,
            author: ChannelPostAuthor::User,
            text: "@RE36 look".to_string(),
            at_ms: 1,
            ..ChannelPostView::default()
        }],
    });
    assert_eq!(app.channel_posts("c1").len(), 1);
    assert!(!app.pane_is_empty(), "the posts fill the chat");
    let ((_, posts), _, _) = app.chat_line_cache();
    assert_eq!(posts.len(), 1);

    app.process_command(UiCommand::SetActiveAgent("re".to_string()));
    let ((_, posts), _, _) = app.chat_line_cache();
    assert!(posts.is_empty(), "only the coordinator's chat shows them");
}

#[test]
fn test_messages_to_members_route_past_the_coordinator() {
    let (_dir, app) = app_in_channel("co");
    assert_eq!(
        app.routed_members("@re36, look and ask @SUB2 too @RE36"),
        Some(vec!["RE36".to_string(), "SUB2".to_string()])
    );
    assert_eq!(app.routed_members("@RE36 and @coordinator"), None);
    assert_eq!(app.routed_members("@RE36 and @CO1"), None);
    assert_eq!(app.routed_members("look at @src/main.rs"), None);
    let (_dir, app) = app_in_channel("re");
    assert_eq!(
        app.routed_members("@SUB2 hi"),
        None,
        "not a coordinator chat"
    );
}

#[test]
fn test_a_message_to_members_does_not_wait_on_the_coordinator() {
    let (_dir, mut app) = app_in_channel("co");
    type_keys(&mut app, "@RE36 look");
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert_ne!(app.state, AppState::Processing);
    assert!(app.just_queued.is_none());
    assert!(
        app.notification
            .as_ref()
            .is_some_and(|(text, _)| text == "Sent to RE36"),
        "{:?}",
        app.notification
    );
}

// ========================================================================
// New-channel form and the CHANNELS tab's menu
// ========================================================================

/// `app_with_channel` with "+ New channel" chosen.
fn app_with_channel_form() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_channel();
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_channel(), app.channels().len());
    app.handle_key(key(KeyCode::Enter));
    (app, rx)
}

#[test]
fn test_new_channel_row_opens_the_form() {
    let (mut app, mut rx) = app_with_channel_form();
    assert_eq!(app.state(), AppState::NewChannel);
    let form = app.channel_form().expect("the form is open");
    assert_eq!(form.focus(), super::channel_form::ChannelField::Slug);
    let text = drawn_text(&mut app);
    for label in ["New channel", "Slug", "Title", "Description", "Esc cancel"] {
        assert!(text.contains(label), "{label}:\n{text}");
    }
    assert!(rx.try_recv().is_err(), "opening sends nothing");
}

#[test]
fn test_an_invalid_slug_shows_the_hint_and_blocks_submit() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "bad slug!");
    let text = drawn_text(&mut app);
    assert!(text.contains(super::channel_form::SLUG_HINT), "{text}");

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    assert_eq!(app.state(), AppState::NewChannel, "still open");
    assert!(rx.try_recv().is_err(), "nothing sent");

    // Enter on the last field submits, but not this slug either.
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::NewChannel);
    assert_eq!(
        app.channel_form().map(|form| form.focus()),
        Some(super::channel_form::ChannelField::Slug),
        "the cursor goes back to the slug"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_a_valid_form_submits_create_channel_and_closes() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "Zode-2");
    let text = drawn_text(&mut app);
    assert!(
        !text.contains(super::channel_form::SLUG_HINT),
        "uppercase is lowercased, so the slug is good:\n{text}"
    );
    app.handle_key(key(KeyCode::Tab));
    type_keys(&mut app, "Zode two");
    app.handle_key(key(KeyCode::Down));
    type_keys(&mut app, "Ship it");
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateChannel { slug, title, description })
            if slug == "zode-2" && title == "Zode two" && description == "Ship it"
    ));
    assert!(rx.try_recv().is_err(), "exactly one event");
}

#[test]
fn test_esc_cancels_the_form_without_an_event() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "zode");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(rx.try_recv().is_err());
}

/// `app_with_channel` where `#zode` is linked to project `beta` and the
/// app also lists project `alpha`.
fn app_with_linkable_channel() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_channel();
    let project = |id: &str, name: &str| crate::events::ProjectSummary {
        id: id.to_string(),
        name: name.to_string(),
        ..crate::events::ProjectSummary::default()
    };
    app.process_command(UiCommand::SetProjects(vec![
        project("p1", "alpha"),
        project("p2", "beta"),
    ]));
    app.process_command(UiCommand::SetChannels(vec![
        crate::events::ChannelSummary {
            id: "c1".to_string(),
            slug: "zode".to_string(),
            title: "Zode release".to_string(),
            coordinator_id: "aa".to_string(),
            members: vec![agent("aa", "Coordinator")],
            projects: vec![crate::events::ChannelProjectView {
                id: "p2".to_string(),
                name: "beta".to_string(),
                folder: "/src/beta".to_string(),
            }],
            ..crate::events::ChannelSummary::default()
        },
    ]));
    (app, rx)
}

/// Draw the app and right-click the first cell showing `text`.
fn right_click_on(app: &mut App, text: &str) {
    right_click_on_where(app, text, |_| true);
}

/// Right-click `#zode`'s row of the list, which shows only its slug, not
/// the chat bar's title row naming the open channel.
fn right_click_on_channel_row(app: &mut App) {
    right_click_on_where(app, "#zode", |row| !row.contains("Zode release"));
}

/// Draw the app and right-click the first cell showing `text` on a row
/// `pick` accepts, e.g. to skip the chat bar's title.
fn right_click_on_where(app: &mut App, text: &str, pick: impl Fn(&str) -> bool) {
    use crossterm::event::{MouseButton, MouseEventKind};
    let drawn = drawn_text(app);
    let rows: Vec<&str> = drawn.lines().collect();
    let row = rows
        .iter()
        .position(|row| row.contains(text) && pick(row))
        .unwrap_or_else(|| panic!("no {text}:\n{drawn}"));
    let line = rows[row];
    let column = line[..line.find(text).unwrap_or(0)].chars().count();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::NONE,
    });
}

fn menu_items(app: &App) -> Vec<super::context_menu::MenuItem> {
    app.context_menu()
        .map(|menu| menu.items.clone())
        .unwrap_or_default()
}

#[test]
fn test_a_channels_menu_offers_link_unlink_and_archive() {
    use super::context_menu::MenuItem;
    let (mut app, _rx) = app_with_linkable_channel();
    right_click_on_channel_row(&mut app);
    assert_eq!(
        menu_items(&app),
        [
            MenuItem::LinkProject,
            MenuItem::UnlinkProject,
            MenuItem::ArchiveChannel
        ]
    );
    let text = drawn_text(&mut app);
    for label in ["Link project", "Unlink project", "Archive channel"] {
        assert!(text.contains(label), "{label}:\n{text}");
    }
}

#[test]
fn test_a_channel_without_links_offers_no_unlink() {
    use super::context_menu::MenuItem;
    let (mut app, _rx) = app_with_channel();
    app.process_command(UiCommand::SetProjects(vec![
        crate::events::ProjectSummary {
            id: "p1".to_string(),
            name: "alpha".to_string(),
            ..crate::events::ProjectSummary::default()
        },
    ]));
    right_click_on_channel_row(&mut app);
    assert_eq!(
        menu_items(&app),
        [MenuItem::LinkProject, MenuItem::ArchiveChannel]
    );
}

#[test]
fn test_link_project_lists_unlinked_projects_and_sends_link_project() {
    let (mut app, mut rx) = app_with_linkable_channel();
    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::Enter));
    let entries = app
        .context_menu()
        .and_then(|menu| menu.submenu.as_ref())
        .map(|submenu| submenu.entries.clone())
        .expect("Enter opens the projects");
    assert_eq!(
        entries,
        [SubmenuEntry::Project {
            id: "p1".to_string(),
            name: "alpha".to_string()
        }],
        "only the project not linked yet"
    );
    assert!(drawn_text(&mut app).contains("alpha"));

    app.handle_key(key(KeyCode::Enter));
    assert!(app.context_menu().is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::LinkProject { channel_id, project_id })
            if channel_id == "c1" && project_id == "p1"
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_unlink_project_in_the_open_channel_sends_unlink_project() {
    let (mut app, mut rx) = app_with_linkable_channel();
    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UnlinkProject { channel_id, project_id })
            if channel_id == "c1" && project_id == "p2"
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_archive_channel_asks_first() {
    let (mut app, mut rx) = app_with_linkable_channel();
    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err(), "not before y");
    app.handle_key(key(KeyCode::Char('n')));
    assert!(rx.try_recv().is_err(), "anything else cancels");

    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveChannel(id)) if id == "c1"));
}

#[test]
fn test_right_click_on_the_channel_side_panel_opens_its_menu() {
    let (mut app, _rx) = app_with_linkable_channel();
    let id = app.channels()[0].id.clone();
    app.open_channel_by_id(&id);
    app.show_side_tab(SidePanelTab::ChannelInfo);
    right_click_on(&mut app, "LINKED PROJECTS");
    assert!(
        menu_items(&app).contains(&super::context_menu::MenuItem::ArchiveChannel),
        "{:?}",
        menu_items(&app)
    );
}

#[test]
fn test_kernel_panel_commands_leave_the_layout_alone_while_a_channel_is_open() {
    let (_dir, mut app) = app_in_channel("co");
    let before = (
        app.record_panel_visible,
        app.side_expanded,
        app.sandbox_panel_visible,
    );
    app.show_members();
    app.show_policies();
    app.show_inbox();
    assert_eq!(
        (
            app.record_panel_visible,
            app.side_expanded,
            app.sandbox_panel_visible,
        ),
        before
    );
    assert!(
        !app.record_panel_visible(),
        "the channel still fills the column"
    );
}
