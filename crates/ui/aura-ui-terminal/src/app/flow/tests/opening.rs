use super::*;

#[test]
fn enter_opens_the_node_as_json_and_closing_it_comes_back() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    app.flow_mut().nodes[2].meta = vec![("args".into(), r#"{"agent":"coder"}"#.into())];
    press(&mut app, KeyCode::Enter);
    let Some(DetailView::File(file)) = app.detail_view() else {
        panic!("not a file view: {:?}", app.detail_view());
    };
    assert_eq!(file.title, "GRAPH TOOL task coder");
    let FileBody::Text(json) = &file.body else {
        panic!("no text");
    };
    assert!(json.contains("\"agent\": \"Lead\""), "{json}");
    assert!(
        json.contains("\"args\": {\n      \"agent\": \"coder\""),
        "{json}"
    );
    assert!(json.contains("\"kind\": \"spawn\""), "{json}");
    assert!(json.contains("\"label\": \"write greet()\""), "{json}");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.detail_view().is_none());
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    assert_eq!(selected(&app), "aa:3");
}

#[test]
fn shift_enter_on_an_empty_line_is_the_graphs_enter() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(app.input(), "", "no line break on GRAPH");
    assert!(
        matches!(app.detail_view(), Some(DetailView::File(_))),
        "the node opens: {:?}",
        app.detail_view()
    );
    app.handle_key(KeyEvent::from(KeyCode::Esc));

    // A line already started takes the break.
    press(&mut app, KeyCode::Char('/'));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(app.input(), "/\n");
}

#[test]
fn an_opened_turn_shows_its_totals_and_a_spawned_agent_who_started_it() {
    let (lanes, mut nodes) = delegation();
    nodes[0].at_ms = 1_700_000_000_123;
    nodes[1].usage = Some(FlowUsage {
        input_tokens: 1200,
        output_tokens: 300,
        cost_usd: Some(0.01),
        duration_ms: Some(2500),
    });
    let mut app = app_with(lanes, nodes);
    let opened = |app: &mut App, id: &str| {
        app.flow_mut().selected = Some(id.into());
        press(app, KeyCode::Enter);
        let Some(DetailView::File(file)) = app.detail_view() else {
            panic!("not a file view: {:?}", app.detail_view());
        };
        let FileBody::Text(json) = &file.body else {
            panic!("no text");
        };
        let json = json.clone();
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        json
    };

    let turn = opened(&mut app, "aa:1");
    assert!(turn.contains("\"input_tokens\": 1200"), "{turn}");
    assert!(turn.contains("\"duration_ms\": 2500"), "{turn}");
    assert!(turn.contains(".123\""), "time to the millisecond: {turn}");
    assert!(!turn.contains("spawned by"), "{turn}");

    let child = opened(&mut app, "bb:1");
    assert!(child.contains("\"spawned by\": \"Lead (aa)\""), "{child}");
}

#[test]
fn o_opens_the_agent_of_the_selected_card() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    let listed = |id: &str, name: &str| AgentSummary {
        id: id.into(),
        name: name.into(),
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![
        listed(LEAD, "Lead"),
        listed(CODER, "coder #1"),
    ]));
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app.show_chat_tab(ChatTab::Graph);
    let (lanes, nodes) = delegation();
    app.process_command(UiCommand::FlowLoad {
        agent_id: LEAD.into(),
        lanes,
        nodes,
    });
    while rx.try_recv().is_ok() {}

    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Char(']'));
    // The tester runs in the background and has no chat.
    assert_eq!(selected(&app), "cc:1");
    press(&mut app, KeyCode::Char('o'));
    assert!(rx.try_recv().is_err());
    assert!(app.notification().is_some());

    press(&mut app, KeyCode::Char('r'));
    let (message, _) = app.notification().unwrap();
    assert!(message.contains("tester #1"), "{message}");

    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "bb:1");
    press(&mut app, KeyCode::Char('o'));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == CODER));
    assert_eq!(
        app.active_agent_id(),
        CODER,
        "the coder comes on screen at once"
    );
}

#[test]
fn typing_still_reaches_the_input() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Char('/'));
    assert_eq!(app.input(), "/");
    // Once the line has text, the flow's letters are text too.
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.input(), "/g");
    assert_eq!(selected(&app), "aa:10");
}
