use super::*;

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
    app.chat_state = AppState::Processing;

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
    app.side.tab = SidePanelTab::Trace;
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
    app.focus = super::super::PanelFocus::Swarm;
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
