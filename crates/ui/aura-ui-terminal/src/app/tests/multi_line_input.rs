use super::*;

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
    app.chat_state = AppState::LoginEmail;
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
    app.chat_state = AppState::Idle;
    type_chars(&mut app, "one");
    app.handle_key(shift_enter());
    type_chars(&mut app, "two");
    app.handle_key(key(KeyCode::Enter));
    app.chat_state = AppState::Idle;
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
    app.chat_state = AppState::Idle;
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
    for tab in [ChatTab::Graph, ChatTab::Time, ChatTab::Log] {
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
fn a_press_on_the_stop_button_under_an_approval_denies_the_call() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let (mut app, mut rx, mut terminal, stop) = busy_app_with_stop_button();
    app.process_command(approval_request("req-1", ""));
    assert_eq!(app.state(), AppState::AwaitingApproval);
    draw(&mut app, &mut terminal);

    // The backdrop hides the button: the press is off the approval.
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        stop.x,
        stop.y,
    ));
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::Deny(id) if id == "req-1")),
        "{events:?}"
    );
    assert!(!events.iter().any(|e| matches!(e, UiEvent::Cancel(_))));
    assert!(app.pending_approval().is_none());
    assert_eq!(app.state(), AppState::Processing, "the turn goes on");
}

#[test]
fn switching_to_an_agent_not_in_memory_is_immediate_and_shows_a_quick_load_at_once() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert_eq!(app.active_agent_id(), "bb", "before the session answers");
    assert!(app.chat_loading());
    assert!(app.chat_animating());
    assert!(app.chat_fading().is_none());

    load_chat_of(&mut app, "bb", "bb's history");
    assert!(!app.chat_loading());
    assert!(app.chat_fading().is_none(), "a quick load does not fade in");
    assert!(app.messages().iter().any(|m| m.content() == "bb's history"));
}

#[test]
fn a_chat_that_was_said_to_be_loading_fades_in_once_loaded() {
    let (mut app, mut rx) = app_with_agents();
    app.handle_command("/agent 2");
    let _ = rx.try_recv();
    app.switching.chat_loading_since =
        Instant::now().checked_sub(super::super::run_log::LOADING_DELAY);
    assert!(app.chat_loading_shown());

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
