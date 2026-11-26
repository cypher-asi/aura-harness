use super::*;

#[test]
fn opening_the_tab_asks_for_the_flow_of_the_agent_on_screen() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    assert!(rx.try_recv().is_err(), "nothing asked while on CHAT");

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == LEAD));
    assert!(!app.flow().loaded());

    // Another agent's flow is not this tab's.
    let (lanes, nodes) = delegation();
    app.process_command(UiCommand::FlowLoad {
        agent_id: CODER.into(),
        lanes: lanes.clone(),
        nodes: nodes.clone(),
    });
    assert!(!app.flow().loaded());
    app.process_command(UiCommand::FlowLoad {
        agent_id: LEAD.into(),
        lanes,
        nodes,
    });
    assert!(app.flow().loaded());

    // Another agent opens on the tab it was left on, CHAT the first time,
    // and asks for nothing there.
    app.process_command(UiCommand::SetActiveAgent(CODER.into()));
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(rx.try_recv().is_err(), "no flow asked for on CHAT");

    app.show_chat_tab(ChatTab::Graph);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == CODER));
    assert!(app.flow().nodes().is_empty());

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.chat_tab(), ChatTab::Chat);

    // The agent left on GRAPH comes back on it, and asks for its flow again.
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == LEAD));
    assert!(rx.try_recv().is_err(), "asked once");
}

#[test]
fn the_selection_follows_new_cards_until_it_is_moved() {
    let mut app = app_with_flow();
    assert_eq!(selected(&app), "aa:10", "starts on the newest node");

    let more = |seq: u64| UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: Vec::new(),
        nodes: vec![node(
            LEAD,
            seq,
            300 + seq,
            FlowKind::Prompt,
            "and a farewell",
        )],
    };
    app.process_command(more(11));
    app.flow_mut().ensure_layout();
    assert_eq!(selected(&app), "aa:11");

    press(&mut app, KeyCode::Left);
    assert_eq!(selected(&app), "aa:1", "the turn before");
    app.process_command(more(12));
    app.flow_mut().ensure_layout();
    assert_eq!(selected(&app), "aa:1", "stays where it was put");

    press(&mut app, KeyCode::End);
    app.process_command(more(13));
    app.flow_mut().ensure_layout();
    assert_eq!(selected(&app), "aa:13");
}

#[test]
fn an_update_changes_the_node_it_is_about() {
    let mut app = app_with_flow();
    let mut update = node(CODER, 2, 0, FlowKind::Tool, "");
    update.status = TraceNodeStatus::Error;
    update.detail = "denied".into();
    app.process_command(UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: Vec::new(),
        nodes: vec![update],
    });
    let flow = app.flow();
    assert_eq!(flow.nodes().len(), 13);
    let tool = flow.nodes().iter().find(|n| n.id == "bb:2").unwrap();
    assert_eq!(tool.label, "write_file src/greet.rs");
    assert_eq!(tool.detail, "denied");
    assert_eq!(tool.status, TraceNodeStatus::Error);

    // Updates for another agent's flow are not this tab's.
    app.process_command(UiCommand::FlowUpsert {
        agent_id: CODER.into(),
        lanes: Vec::new(),
        nodes: vec![node(CODER, 9, 900, FlowKind::Reply, "stray")],
    });
    assert_eq!(app.flow().nodes().len(), 13);
}

#[test]
fn arrows_move_down_a_turn_between_bands_and_along_turns() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    assert_eq!(selected(&app), "aa:1");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:10", "the turn's reply");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:2", "the model call under it");
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(selected(&app), "aa:3", "the call's first row");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:7");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:8");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "bb:1", "the nearest card of the band below");
    press(&mut app, KeyCode::Up);
    assert_eq!(selected(&app), "aa:8", "and back up, onto the last row");
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(selected(&app), "aa:7");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "bb:1", "the next prompt");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "cc:1");
    press(&mut app, KeyCode::Char('['));
    assert_eq!(selected(&app), "bb:1");
    press(&mut app, KeyCode::Char('G'));
    assert_eq!(selected(&app), "aa:10");

    // Sideways is turn to turn, from anywhere in a turn.
    let (lanes, nodes) = three_turns();
    let mut app = app_with(lanes, nodes);
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(selected(&app), "aa:4");
    press(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:7");
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(selected(&app), "aa:4");
    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(selected(&app), "aa:1");
}

#[test]
fn following_links_walks_the_delegation_and_back() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:1");
    assert!(app.notification().is_some(), "a prompt links nowhere");

    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(selected(&app), "aa:3");
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(
        selected(&app),
        "bb:1",
        "the task call to the agent it started"
    );
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:3", "and back");
}

#[test]
fn the_view_follows_the_selection_until_it_is_panned() {
    let mut app = app_with_flow();
    let flow = app.flow_mut();
    let (x, _) = flow.sync_view(40.0, 20.0);
    let rect = flow.layout().card_rect(flow.selected_card().unwrap());
    assert!(x <= cells(rect.x) && cells(rect.right()) <= x + 40.0, "{x}");

    press(&mut app, KeyCode::Home);
    assert_eq!(app.flow_mut().sync_view(40.0, 20.0), (0.0, 0.0));

    // Panned, the view stays put, off the canvas as well as on it; moving
    // the selection brings it back.
    app.pan_flow(30.5, 4.0);
    assert_eq!(app.flow_mut().sync_view(40.0, 5.0), (30.5, 4.0));
    app.pan_flow(-1000.0, 1000.0);
    assert_eq!(app.flow_mut().sync_view(40.0, 5.0), (-969.5, 1004.0));
    press(&mut app, KeyCode::Char('0'));
    assert_eq!(app.flow_mut().sync_view(40.0, 20.0), (0.0, 0.0), "back");
    app.pan_flow(-50.0, -50.0);
    press(&mut app, KeyCode::Char(']'));
    let (x, _) = app.flow_mut().sync_view(40.0, 20.0);
    let flow = app.flow();
    let rect = flow.layout().card_rect(flow.selected_card().unwrap());
    assert!(x <= cells(rect.x), "{x}");
}
