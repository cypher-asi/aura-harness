use super::layout::{WireKind, MAX_LINES};
use super::*;
use crate::app::file_view::FileBody;
use crate::events::{AgentSummary, TraceNodeStatus, UiCommand};
use tokio::sync::mpsc;

const LEAD: &str = "aa";
const CODER: &str = "bb";
const TESTER: &str = "cc";

fn lane(agent: &str, name: &str, parent: Option<&str>) -> FlowLane {
    FlowLane {
        agent_id: agent.into(),
        name: name.into(),
        parent: parent.map(str::to_string),
    }
}

fn node(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
    FlowNode {
        id: format!("{agent}:{seq}"),
        agent_id: agent.into(),
        kind,
        label: label.into(),
        status: TraceNodeStatus::Ok,
        seq,
        at_ms,
        ..FlowNode::default()
    }
}

fn linked(mut from: FlowNode, kind: &str, to: &str) -> FlowNode {
    from.links.push((kind.into(), to.into()));
    from
}

/// A lead that gives a coder a task, gets its reply, then gives a
/// tester one.
fn delegation() -> (Vec<FlowLane>, Vec<FlowNode>) {
    let lanes = vec![
        lane(LEAD, "Lead", None),
        lane(CODER, "coder #1", Some(LEAD)),
        lane(TESTER, "tester #1", Some(LEAD)),
    ];
    let nodes = vec![
        node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
        node(LEAD, 2, 110, FlowKind::Model, "sonnet"),
        linked(
            node(LEAD, 3, 120, FlowKind::Tool, "task coder"),
            "spawn",
            "bb:1",
        ),
        node(CODER, 1, 130, FlowKind::Prompt, "write greet()"),
        node(CODER, 2, 140, FlowKind::Tool, "write_file src/greet.rs"),
        linked(
            node(CODER, 3, 150, FlowKind::Reply, "done"),
            "return",
            "aa:6",
        ),
        node(LEAD, 6, 160, FlowKind::Result, "coder #1 replied"),
        node(LEAD, 7, 165, FlowKind::Model, "sonnet"),
        linked(
            node(LEAD, 8, 170, FlowKind::Tool, "task tester"),
            "spawn",
            "cc:1",
        ),
        node(TESTER, 1, 180, FlowKind::Prompt, "test greet()"),
        linked(
            node(TESTER, 2, 190, FlowKind::Reply, "passes"),
            "return",
            "aa:9",
        ),
        node(LEAD, 9, 200, FlowKind::Result, "tester #1 replied"),
        node(LEAD, 10, 210, FlowKind::Reply, "greeting added"),
    ];
    (lanes, nodes)
}

fn app_with(lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    app.show_chat_tab(ChatTab::Graph);
    app.process_command(UiCommand::FlowLoad {
        agent_id: LEAD.into(),
        lanes,
        nodes,
    });
    app.flow_mut().ensure_layout();
    app
}

fn app_with_flow() -> App {
    let (lanes, nodes) = delegation();
    app_with(lanes, nodes)
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
    app.flow_mut().ensure_layout();
}

fn selected(app: &App) -> &str {
    app.flow()
        .selected_node()
        .map_or("", |node| node.id.as_str())
}

/// Each card's head and rows, by label, in time order.
fn cards(app: &App) -> Vec<Vec<&str>> {
    let flow = app.flow();
    flow.layout()
        .cards
        .iter()
        .map(|card| {
            std::iter::once(card.head)
                .chain(card.rows.iter().copied())
                .map(|node| flow.nodes()[node].label.as_str())
                .collect()
        })
        .collect()
}

fn heads(app: &App) -> Vec<&str> {
    cards(app).into_iter().map(|card| card[0]).collect()
}

#[test]
fn a_model_call_is_a_card_with_its_tool_calls_as_rows() {
    let app = app_with_flow();
    assert_eq!(
        cards(&app),
        [
            vec!["add a greeting"],
            vec!["sonnet", "task coder"],
            vec!["write greet()", "write_file src/greet.rs"],
            vec!["done"],
            vec!["coder #1 replied"],
            vec!["sonnet", "task tester"],
            vec!["test greet()"],
            vec!["passes"],
            vec!["tester #1 replied"],
            vec!["greeting added"],
        ]
    );
}

#[test]
fn cards_go_down_a_column_and_no_wire_runs_backwards() {
    let app = app_with_flow();
    let layout = app.flow().layout();
    let columns: Vec<usize> = layout.cards.iter().map(|card| card.column).collect();
    assert_eq!(columns, [0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
    let tops: Vec<usize> = layout.cards.iter().map(|card| card.top).collect();
    assert_eq!(tops, [0, 4, 0, 5, 0, 4, 0, 4, 0, 4]);
    for wire in &layout.wires {
        let (from, to) = (&layout.cards[wire.from], &layout.cards[wire.to]);
        if layout.stacked(wire) {
            assert!(wire.kind == WireKind::Next && from.top < to.top, "{wire:?}");
        } else {
            assert!(from.column < to.column, "{wire:?}");
        }
    }
    let between = |kind: WireKind| -> Vec<(usize, Option<usize>, usize)> {
        layout
            .wires
            .iter()
            .filter(|wire| wire.kind == kind)
            .map(|wire| (wire.from, wire.row, wire.to))
            .collect()
    };
    // The task calls start the agents from their rows; the replies come
    // back to the results.
    assert_eq!(between(WireKind::Spawn), [(1, Some(0), 2), (5, Some(0), 6)]);
    assert_eq!(between(WireKind::Return), [(3, None, 4), (7, None, 8)]);
    assert_eq!(
        between(WireKind::Next),
        [
            (0, None, 1),
            (2, None, 3),
            (1, None, 4),
            (4, None, 5),
            (6, None, 7),
            (5, None, 8),
            (8, None, 9)
        ]
    );
}

#[test]
fn agents_that_take_turns_share_a_band() {
    let app = app_with_flow();
    let layout = app.flow().layout();
    let bands: Vec<usize> = layout.cards.iter().map(|card| card.band).collect();
    assert_eq!(bands, [0, 0, 1, 1, 0, 0, 1, 1, 0, 0]);
    assert_eq!(layout.bands.len(), 2);
    // A band is as tall as its tallest column, with its names above.
    let column = layout.cards[1].top + layout.cards[1].height();
    assert_eq!(layout.bands[0].height, column + 2);
    assert_eq!(layout.height, layout.bands[1].y + layout.bands[1].height);
}

#[test]
fn a_column_wraps_right_when_the_view_has_no_room_for_the_next_card() {
    let nodes = (1..=5)
        .map(|seq| {
            let kind = if seq % 2 == 1 {
                FlowKind::Prompt
            } else {
                FlowKind::Reply
            };
            node(LEAD, seq, 100 + seq, kind, "turn")
        })
        .collect();
    let mut app = app_with(vec![lane(LEAD, "Lead", None)], nodes);
    // Ten rows for cards: two of three, and the row between.
    app.flow_mut().set_room(12);
    app.flow_mut().ensure_layout();
    let layout = app.flow().layout();
    let places: Vec<(usize, usize)> = layout
        .cards
        .iter()
        .map(|card| (card.column, card.top))
        .collect();
    assert_eq!(places, [(0, 0), (0, 4), (1, 0), (1, 4), (2, 0)]);
    assert!(layout.height <= 12, "{}", layout.height);
    let stacked: Vec<bool> = layout.wires.iter().map(|w| layout.stacked(w)).collect();
    assert_eq!(stacked, [true, false, true, false]);

    // A taller view fits them all in one column.
    app.flow_mut().set_room(40);
    app.flow_mut().ensure_layout();
    assert!(app
        .flow()
        .layout()
        .cards
        .iter()
        .all(|card| card.column == 0));
}

#[test]
fn bands_too_tall_for_the_view_share_its_rows() {
    let mut app = app_with_flow();
    app.flow_mut().set_room(30);
    app.flow_mut().ensure_layout();
    let layout = app.flow().layout();
    assert_eq!(layout.height, 20, "both fit as they are");
    assert_eq!(layout.cards[1].top, 4);

    app.flow_mut().set_room(16);
    app.flow_mut().ensure_layout();
    let layout = app.flow().layout();
    assert_eq!(layout.bands.len(), 2);
    assert!(layout.height <= 16, "{}", layout.height);
    assert!(
        layout.cards.iter().all(|card| card.top == 0),
        "no room to stack"
    );
}

#[test]
fn agents_working_at_once_get_a_band_each() {
    let app = app_with(
        vec![
            lane(LEAD, "Lead", None),
            lane(CODER, "coder #1", Some(LEAD)),
            lane(TESTER, "tester #1", Some(LEAD)),
        ],
        vec![
            node(LEAD, 1, 100, FlowKind::Prompt, "do both"),
            node(CODER, 1, 110, FlowKind::Prompt, "code"),
            node(TESTER, 1, 120, FlowKind::Prompt, "test"),
            node(CODER, 2, 130, FlowKind::Reply, "coded"),
            node(TESTER, 2, 140, FlowKind::Reply, "tested"),
        ],
    );
    let bands: Vec<usize> = app
        .flow()
        .layout()
        .cards
        .iter()
        .map(|card| card.band)
        .collect();
    assert_eq!(bands, [0, 1, 2, 1, 2]);
}

#[test]
fn a_lane_keeps_its_record_order_when_clocks_disagree() {
    let app = app_with(
        vec![lane(LEAD, "Lead", None)],
        vec![
            node(LEAD, 2, 90, FlowKind::Model, "second"),
            node(LEAD, 1, 100, FlowKind::Prompt, "first"),
            node(LEAD, 3, 95, FlowKind::Reply, "third"),
        ],
    );
    assert_eq!(heads(&app), ["first", "second", "third"]);
}

#[test]
fn a_card_with_many_rows_lists_what_fits_and_counts_the_rest() {
    let mut nodes = vec![node(LEAD, 1, 100, FlowKind::Model, "sonnet")];
    nodes.extend((2..=16).map(|seq| node(LEAD, seq, 100 + seq, FlowKind::Tool, "read_file")));
    let mut app = app_with(vec![lane(LEAD, "Lead", None)], nodes);
    let card = app.flow().layout().cards[0].clone();
    assert_eq!(card.rows.len(), 15);
    assert_eq!(card.lines(), MAX_LINES);
    assert_eq!(card.line_of(Some(3)), 5);
    assert_eq!(card.line_of(Some(14)), MAX_LINES + 1, "on the last line");
    assert_eq!(app.flow().more_label(0), "⋯ 6 more");

    // The newest row is selected; the last line shows it.
    assert_eq!(selected(&app), "aa:16");
    assert_eq!(app.flow().last_line_row(0), Some(14));
    press(&mut app, KeyCode::Home);
    assert_eq!(app.flow().last_line_row(0), None);
}

#[test]
fn opening_the_tab_asks_for_the_flow_of_the_agent_on_screen() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    assert!(rx.try_recv().is_err(), "nothing asked while on CHAT");

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
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

    // Switching agents shows the new agent's flow.
    app.process_command(UiCommand::SetActiveAgent(CODER.into()));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenFlow(id)) if id == CODER));
    assert!(app.flow().nodes().is_empty());

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.chat_tab(), ChatTab::Chat);
}

#[test]
fn the_selection_follows_new_cards_until_it_is_moved() {
    let mut app = app_with_flow();
    assert_eq!(selected(&app), "aa:10", "starts on the newest card");

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
    assert_eq!(selected(&app), "aa:10");
    app.process_command(more(12));
    app.flow_mut().ensure_layout();
    assert_eq!(selected(&app), "aa:10", "stays where it was put");

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
fn arrows_move_along_a_lane_through_a_card_and_between_bands() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    assert_eq!(selected(&app), "aa:1");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:2", "the card under it");
    press(&mut app, KeyCode::Up);
    assert_eq!(selected(&app), "aa:1", "and over it");
    press(&mut app, KeyCode::Right);
    assert_eq!(selected(&app), "aa:2", "the next card of the lead");
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(selected(&app), "aa:3", "the card's first row");
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "bb:1", "the nearest card of the band below");
    press(&mut app, KeyCode::Up);
    assert_eq!(selected(&app), "aa:3", "and back up, onto the last row");
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(selected(&app), "aa:2");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "bb:1", "the next prompt");
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "cc:1");
    press(&mut app, KeyCode::Char('['));
    assert_eq!(selected(&app), "bb:1");
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(selected(&app), "bb:3");
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(selected(&app), "bb:1");
    press(&mut app, KeyCode::Char('G'));
    assert_eq!(selected(&app), "aa:10");
}

#[test]
fn following_links_walks_the_delegation_and_back() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:1");
    assert!(app.notification().is_some(), "a prompt links nowhere");

    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
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
fn folding_a_turn_leaves_its_prompt_and_moves_its_wires_there() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "aa:3");

    press(&mut app, KeyCode::Char(' '));
    assert_eq!(selected(&app), "aa:1", "the selection moves to the prompt");
    let layout = app.flow().layout();
    assert!(layout.cards[0].folded);
    assert_eq!(layout.cards[0].steps, 5);
    assert_eq!(
        heads(&app),
        [
            "add a greeting",
            "write greet()",
            "done",
            "test greet()",
            "passes"
        ]
    );
    let layout = app.flow().layout();
    assert!(
        layout
            .wires
            .iter()
            .any(|wire| wire.from == 0 && wire.to == 1 && wire.kind == WireKind::Spawn),
        "the task call's wire leaves from the prompt: {:?}",
        layout.wires
    );

    // A link into the folded turn opens it again.
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Right);
    assert_eq!(selected(&app), "bb:3");
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:6");
    assert_eq!(app.flow().layout().cards.len(), 10);

    press(&mut app, KeyCode::Char('z'));
    assert_eq!(
        heads(&app),
        ["add a greeting", "write greet()", "test greet()"],
        "every turn folded"
    );
    assert_eq!(selected(&app), "aa:1");
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.flow().layout().cards.len(), 10);
}

#[test]
fn the_view_follows_the_selection_until_it_is_panned() {
    let mut app = app_with_flow();
    let flow = app.flow_mut();
    let (x, _) = flow.sync_view(80.0, 20.0);
    let rect = flow.layout().card_rect(flow.selected_card().unwrap());
    assert!(x <= cells(rect.x) && cells(rect.right()) <= x + 80.0, "{x}");

    press(&mut app, KeyCode::Home);
    assert_eq!(app.flow_mut().sync_view(80.0, 20.0), (0.0, 0.0));

    // Panned, the view stays put; moving the selection brings it back.
    app.pan_flow(30.5, 4.0);
    assert_eq!(app.flow_mut().sync_view(80.0, 5.0), (30.5, 4.0));
    app.pan_flow(-1000.0, 1000.0);
    let most_y = cells(app.flow().layout().height) - 5.0;
    assert_eq!(
        app.flow_mut().sync_view(80.0, 5.0),
        (0.0, most_y),
        "never off the canvas"
    );
    press(&mut app, KeyCode::Right);
    let (x, _) = app.flow_mut().sync_view(80.0, 20.0);
    let flow = app.flow();
    let rect = flow.layout().card_rect(flow.selected_card().unwrap());
    assert!(x <= cells(rect.x), "{x}");
}

#[test]
fn a_click_on_the_canvas_selects_a_line_then_opens_it_and_folds_at_the_marker() {
    let mut app = app_with_flow();
    let layout = app.flow().layout();
    let rect = layout.card_rect(1);
    let (x, y) = (cells(rect.x + 4) + 0.5, cells(rect.y + 2) + 0.5);
    assert!(app.click_flow_at(x, y));
    assert_eq!(selected(&app), "aa:3", "the row clicked");
    assert!(app.detail_view().is_none());
    assert!(app.click_flow_at(x, y));
    assert!(app.detail_view().is_some(), "the second click opens it");
    app.handle_key(KeyEvent::from(KeyCode::Esc));

    let rect = app.flow().layout().card_rect(0);
    assert!(app.click_flow_at(cells(rect.x + 2) + 0.5, cells(rect.y) + 0.5));
    app.flow_mut().ensure_layout();
    assert!(app.flow().layout().cards[0].folded);

    assert!(!app.click_flow_at(0.5, 0.5), "empty canvas");
}

#[test]
fn enter_opens_the_node_as_json_and_closing_it_comes_back() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
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
    press(&mut app, KeyCode::Char('o'));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == CODER));

    // The tester runs in the background and has no chat.
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(selected(&app), "cc:1");
    press(&mut app, KeyCode::Char('o'));
    assert!(rx.try_recv().is_err());
    assert!(app.notification().is_some());

    press(&mut app, KeyCode::Char('r'));
    let (message, _) = app.notification().unwrap();
    assert!(message.contains("tester #1"), "{message}");
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

#[test]
fn old_nodes_go_once_the_flow_is_too_long() {
    let nodes = (1..=u64::try_from(MAX_NODES).unwrap() + 1)
        .map(|seq| node(LEAD, seq, seq, FlowKind::Model, "call"))
        .collect();
    let mut app = app_with(vec![lane(LEAD, "Lead", None)], nodes);
    let flow = app.flow_mut();
    flow.ensure_layout();
    assert_eq!(flow.nodes().len(), MAX_NODES * 3 / 4);
    assert_eq!(flow.layout().cards.len(), flow.nodes().len());
    assert_eq!(
        flow.selected_node().map(|n| n.seq),
        u64::try_from(MAX_NODES + 1).ok()
    );
}
