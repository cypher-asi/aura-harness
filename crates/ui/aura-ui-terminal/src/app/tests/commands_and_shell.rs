use super::*;

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
    app.ticks.processing_since = Instant::now().checked_sub(Duration::from_secs(600));
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
    app.input.text.clear();
    app.input.cursor_pos = 0;
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.input(), "hello agent");
}
