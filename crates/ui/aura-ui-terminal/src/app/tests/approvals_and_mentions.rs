use super::*;

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
    assert!(app.input.text.is_empty());
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
    app.ticks.processing_since = Some(Instant::now());
    app.process_command(for_agent("aa", UiCommand::Complete));
    assert!(app.take_alerts().is_empty());

    app.process_command(for_agent("aa", UiCommand::TurnStarted { prompt: None }));
    app.ticks.processing_since = Instant::now().checked_sub(Duration::from_secs(30));
    app.process_command(for_agent("aa", UiCommand::Complete));
    let alerts = app.take_alerts();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].contains("finished after 30s"), "{alerts:?}");
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
