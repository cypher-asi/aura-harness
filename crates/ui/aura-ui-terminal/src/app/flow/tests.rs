use super::layout::{CardKind, WireKind, MAX_LINES};
use super::timeline::MarkKind;
use super::*;
use crate::app::file_view::FileBody;
use crate::events::{AgentSummary, FlowBench, FlowUsage, TraceNodeStatus, UiCommand};
use tokio::sync::mpsc;

const LEAD: &str = "aa";
const CODER: &str = "bb";
const TESTER: &str = "cc";

fn lane(agent: &str, name: &str, parent: Option<&str>) -> FlowLane {
    FlowLane {
        agent_id: agent.into(),
        name: name.into(),
        parent: parent.map(str::to_string),
        ..FlowLane::default()
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

/// A `task` call at `seq` that starts `agent` and gets back `result`.
fn task(seq: u64, at_ms: u64, label: &str, agent: &str, result: &str) -> FlowNode {
    linked(
        linked(
            node(LEAD, seq, at_ms, FlowKind::Tool, label),
            "spawn",
            &format!("{agent}:1"),
        ),
        "result",
        result,
    )
}

/// A lead turn whose two model calls give a coder a task, get its reply,
/// then give a tester one.
fn delegation() -> (Vec<FlowLane>, Vec<FlowNode>) {
    let lanes = vec![
        lane(LEAD, "Lead", None),
        lane(CODER, "coder #1", Some(LEAD)),
        lane(TESTER, "tester #1", Some(LEAD)),
    ];
    let nodes = vec![
        node(LEAD, 1, 100, FlowKind::Prompt, "add a greeting"),
        node(LEAD, 2, 110, FlowKind::Model, "sonnet"),
        task(3, 120, "task coder", CODER, "aa:6"),
        node(CODER, 1, 130, FlowKind::Prompt, "write greet()"),
        node(CODER, 2, 140, FlowKind::Tool, "write_file src/greet.rs"),
        linked(
            node(CODER, 3, 150, FlowKind::Reply, "done"),
            "return",
            "aa:6",
        ),
        node(LEAD, 6, 160, FlowKind::Result, "coder #1 replied"),
        node(LEAD, 7, 165, FlowKind::Model, "sonnet"),
        task(8, 170, "task tester", TESTER, "aa:9"),
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

/// Three lead turns, the first starting a coder and the last a tester.
fn three_turns() -> (Vec<FlowLane>, Vec<FlowNode>) {
    let lanes = vec![
        lane(LEAD, "Lead", None),
        lane(CODER, "coder #1", Some(LEAD)),
        lane(TESTER, "tester #1", Some(LEAD)),
    ];
    let nodes = vec![
        node(LEAD, 1, 100, FlowKind::Prompt, "first"),
        node(LEAD, 2, 110, FlowKind::Model, "sonnet"),
        task(3, 120, "task coder", CODER, ""),
        node(CODER, 1, 130, FlowKind::Prompt, "code"),
        node(CODER, 2, 140, FlowKind::Reply, "coded"),
        node(LEAD, 4, 200, FlowKind::Prompt, "second"),
        node(LEAD, 5, 210, FlowKind::Model, "sonnet"),
        node(LEAD, 6, 300, FlowKind::Prompt, "third"),
        node(LEAD, 7, 310, FlowKind::Model, "sonnet"),
        task(8, 320, "task tester", TESTER, ""),
        node(TESTER, 1, 330, FlowKind::Prompt, "test"),
        node(TESTER, 2, 340, FlowKind::Reply, "tested"),
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

fn bands(app: &App) -> Vec<usize> {
    let layout = app.flow().layout();
    layout.cards.iter().map(|card| card.band).collect()
}

#[test]
fn a_turn_is_a_main_card_with_a_subcard_for_each_model_call() {
    let app = app_with_flow();
    assert_eq!(
        cards(&app),
        [
            vec!["add a greeting", "greeting added"],
            vec!["sonnet", "task coder"],
            vec!["sonnet", "task tester"],
            vec!["write greet()", "write_file src/greet.rs", "done"],
            vec!["test greet()", "passes"],
        ]
    );
    let layout = app.flow().layout();
    let kinds: Vec<CardKind> = layout.cards.iter().map(|card| card.kind).collect();
    use CardKind::{Call, Turn};
    assert_eq!(kinds, [Turn, Call, Call, Turn, Turn]);
    let blocks: Vec<_> = layout.blocks.iter().map(|b| b.cards.clone()).collect();
    assert_eq!(blocks, [0..3, 3..4, 4..5]);
    assert_eq!(app.flow().card_kind_label(0), "turn");
    assert_eq!(app.flow().card_facts(0), "2 steps");

    // What a task came back with is the task's row.
    let result = app.flow().nodes().iter().position(|n| n.id == "aa:6");
    let result = result.unwrap();
    assert_eq!(layout.card_of(result), Some(1));
    assert_eq!(layout.row_of(result), Some(0));
}

#[test]
fn a_result_no_row_stands_for_and_a_message_are_subcards_of_their_own() {
    let app = app_with(
        vec![lane(LEAD, "Lead", None)],
        vec![
            node(LEAD, 1, 100, FlowKind::Prompt, "go"),
            node(LEAD, 2, 110, FlowKind::Message, "from coder"),
            node(LEAD, 3, 120, FlowKind::Result, "coder replied"),
            node(LEAD, 4, 130, FlowKind::Model, "sonnet"),
        ],
    );
    let layout = app.flow().layout();
    let kinds: Vec<CardKind> = layout.cards.iter().map(|card| card.kind).collect();
    use CardKind::{Call, Note, Turn};
    assert_eq!(kinds, [Turn, Note, Note, Call]);
    assert!(!layout.cards[1].folds());
}

#[test]
fn the_harness_calls_are_steps_of_the_turn_they_are_for() {
    let app = app_with(
        vec![lane(LEAD, "Lead", None)],
        vec![
            node(LEAD, 1, 100, FlowKind::Gate, "haiku"),
            node(LEAD, 2, 101, FlowKind::Prompt, "go"),
            node(LEAD, 3, 110, FlowKind::Model, "sonnet"),
            node(LEAD, 4, 120, FlowKind::Reply, "went"),
            node(LEAD, 5, 130, FlowKind::Title, "haiku"),
            node(LEAD, 6, 140, FlowKind::Gate, "haiku"),
            node(LEAD, 7, 141, FlowKind::Prompt, "again"),
            node(LEAD, 8, 150, FlowKind::Model, "sonnet"),
            node(LEAD, 9, 155, FlowKind::Judge, "haiku"),
            node(LEAD, 10, 160, FlowKind::Tool, "read_file a.rs"),
        ],
    );
    assert_eq!(
        cards(&app),
        [
            vec!["go", "went"],
            vec!["haiku"],
            vec!["sonnet"],
            vec!["haiku"],
            vec!["again"],
            vec!["haiku"],
            vec!["sonnet", "read_file a.rs"],
            vec!["haiku"],
        ],
        "each prompt's classifier opens its turn; tools go to the agent's call"
    );
    let flow = app.flow();
    let kinds: Vec<&str> = (0..8).map(|card| flow.card_kind_label(card)).collect();
    assert_eq!(
        kinds,
        ["turn", "gate", "model", "title", "turn", "gate", "model", "judge"]
    );

    // Classified, the next prompt not yet recorded: its call waits at the
    // end of the turn before.
    let mut app = app;
    app.process_command(UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: Vec::new(),
        nodes: vec![node(LEAD, 11, 170, FlowKind::Gate, "haiku")],
    });
    app.flow_mut().ensure_layout();
    assert_eq!(heads(&app).len(), 9);
    assert_eq!(app.flow().layout().blocks.len(), 2);
}

#[test]
fn calls_and_turns_have_a_line_of_tokens_cost_and_time() {
    let used = |mut node: FlowNode, input_tokens, output_tokens, cost_usd, ms| {
        node.detail = "tokens · time".into();
        node.usage = Some(FlowUsage {
            input_tokens,
            output_tokens,
            cost_usd,
            duration_ms: Some(ms),
        });
        node
    };
    let mut app = app_with(
        vec![lane(LEAD, "Lead", None)],
        vec![
            node(LEAD, 1, 1_000, FlowKind::Prompt, "go"),
            used(
                node(LEAD, 2, 5_000, FlowKind::Model, "sonnet"),
                48_200,
                516,
                Some(0.042),
                3_200,
            ),
            node(LEAD, 3, 5_100, FlowKind::Tool, "read_file a.rs"),
            used(
                node(LEAD, 4, 9_000, FlowKind::Model, "sonnet"),
                50_000,
                40,
                None,
                1_500,
            ),
            node(LEAD, 5, 9_500, FlowKind::Reply, "went"),
        ],
    );
    let flow = app.flow();
    let stats = |card: usize| flow.card_stats(card).map(|(l, r)| format!("{l} | {r}"));
    assert_eq!(stats(1).as_deref(), Some("↑48.2k ↓516 | $0.042 · 3.2s"));
    assert_eq!(stats(2).as_deref(), Some("↑50.0k ↓40 | 1.5s"));
    assert_eq!(
        stats(0).as_deref(),
        Some("↑98.2k ↓556 | $0.042 · 8.5s"),
        "the turn: both calls, from its prompt to its reply"
    );
    assert_eq!(flow.card_facts(1), "", "the title no longer says it");

    // The line is under the head; the rows come after it.
    let layout = flow.layout();
    let call = &layout.cards[1];
    assert_eq!((call.first_row_line(), call.height()), (3, 5));
    let rect = layout.card_rect(1);
    let at = |line: usize| layout.spot_at(cells(rect.x + 4), cells(rect.y + line) + 0.5);
    assert_eq!(at(2), Some(Spot::Head(1)));
    assert_eq!(at(3), Some(Spot::Row(1, 0)));

    // A call that is not done yet has no line.
    app.process_command(UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: Vec::new(),
        nodes: vec![node(LEAD, 6, 9_600, FlowKind::Model, "sonnet")],
    });
    app.flow_mut().ensure_layout();
    assert_eq!(app.flow().card_stats(3), None);
}

#[test]
fn the_fold_button_knows_when_the_pointer_is_over_it() {
    let mut app = app_with_flow();
    let rect = app.flow().layout().card_rect(0);
    let marker = (cells(rect.x + 2) + 0.5, cells(rect.y) + 0.5);
    assert!(app.hover_flow_at(Some(marker)), "onto the button");
    assert!(app.flow().fold_hovered(0));
    assert!(!app.flow().fold_hovered(1));
    assert!(
        !app.hover_flow_at(Some((marker.0 + 1.0, marker.1))),
        "still on it"
    );
    let label = (cells(rect.x + 8) + 0.5, cells(rect.y) + 0.5);
    assert!(app.hover_flow_at(Some(label)), "off it, onto the title");
    assert!(!app.flow().fold_hovered(0));
    assert!(app.hover_flow_at(Some(marker)));
    assert!(app.hover_flow_at(None), "off the canvas");
    assert!(!app.flow().fold_hovered(0));
}

#[test]
fn a_turns_cards_go_down_its_spine_and_no_wire_runs_backwards() {
    let app = app_with_flow();
    let layout = app.flow().layout();
    let columns: Vec<usize> = layout.cards.iter().map(|card| card.column).collect();
    assert_eq!(columns, [0, 0, 0, 1, 1]);
    let tops: Vec<usize> = layout.cards.iter().map(|card| card.top).collect();
    assert_eq!(tops, [0, 5, 10, 0, 0]);
    assert_eq!(bands(&app), [0, 0, 0, 1, 2]);
    let main = layout.card_rect(0);
    let sub = layout.card_rect(1);
    assert_eq!(sub.x, main.x + layout::SUB_INDENT);
    assert_eq!(sub.right(), main.right());
    for wire in &layout.wires {
        let (from, to) = (&layout.cards[wire.from], &layout.cards[wire.to]);
        if layout.stacked(wire) {
            assert!(
                wire.kind == WireKind::Spine && from.top < to.top,
                "{wire:?}"
            );
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
    assert_eq!(between(WireKind::Spine), [(0, None, 1), (0, None, 2)]);
    // The task calls start the agents from their rows; the replies back
    // to them run back in time, so they are not drawn.
    assert_eq!(between(WireKind::Spawn), [(1, Some(0), 3), (2, Some(0), 4)]);
    assert!(between(WireKind::Return).is_empty());
    assert!(between(WireKind::Next).is_empty());
}

#[test]
fn turns_are_open_until_folded_and_stay_folded_as_the_flow_grows() {
    let (lanes, nodes) = three_turns();
    let mut app = app_with(lanes, nodes);
    let all_open = [
        "first", "sonnet", "code", "second", "sonnet", "third", "sonnet", "test",
    ];
    assert_eq!(heads(&app), all_open);
    assert!(app.flow().layout().cards.iter().all(|card| !card.folded));

    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char(' '));
    let layout = app.flow().layout();
    assert!(layout.cards[0].folded);
    assert_eq!(app.flow().card_facts(0), "+1 step");
    // A folded turn's task row is gone; its wire leaves from the turn.
    assert!(layout
        .wires
        .iter()
        .any(|wire| (wire.from, wire.row, wire.to, wire.kind) == (0, None, 1, WireKind::Spawn)));

    // A new turn comes in open and leaves the others as they were.
    app.process_command(UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: Vec::new(),
        nodes: vec![
            node(LEAD, 20, 400, FlowKind::Prompt, "fourth"),
            node(LEAD, 21, 410, FlowKind::Model, "sonnet"),
        ],
    });
    app.flow_mut().ensure_layout();
    let folded = |app: &App| -> Vec<bool> {
        let layout = app.flow().layout();
        layout.cards.iter().map(|card| card.folded).collect()
    };
    let expected = [true, false, false, false, false, false, false, false, false];
    assert_eq!(folded(&app), expected);

    // So does showing another agent and coming back to this one.
    app.process_command(UiCommand::SetActiveAgent(CODER.into()));
    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    let (lanes, mut nodes) = three_turns();
    nodes.push(node(LEAD, 20, 400, FlowKind::Prompt, "fourth"));
    nodes.push(node(LEAD, 21, 410, FlowKind::Model, "sonnet"));
    app.process_command(UiCommand::FlowLoad {
        agent_id: LEAD.into(),
        lanes,
        nodes,
    });
    app.flow_mut().ensure_layout();
    assert_eq!(folded(&app), expected);
    assert_eq!(selected(&app), "aa:1", "the selection came back too");
}

#[test]
fn agents_that_take_turns_share_a_band_when_the_view_is_short() {
    let (lanes, nodes) = three_turns();
    let mut app = app_with(lanes, nodes);
    // Every lead turn in one column: the tester starts where the coder
    // did, so it needs a band of its own.
    assert_eq!(bands(&app), [0, 0, 1, 0, 0, 0, 0, 2]);
    let layout = app.flow().layout();
    assert_eq!(layout.bands.len(), 3);
    let lead = &layout.blocks[layout.cards[5].block];
    assert_eq!(layout.bands[0].height, lead.top + lead.height + 2);
    assert_eq!(layout.height, layout.bands[2].y + layout.bands[2].height);

    // Too tall for the view: the lead's turns go side by side, and the
    // tester starts well after the coder ended.
    app.flow_mut().set_room(20);
    app.flow_mut().ensure_layout();
    assert_eq!(bands(&app), [0, 0, 1, 0, 0, 0, 0, 1]);
    let layout = app.flow().layout();
    assert!(layout.height <= 20, "{}", layout.height);
    assert!(layout
        .cards
        .iter()
        .all(|card| card.kind != CardKind::Turn || card.top == 0));
}

#[test]
fn a_column_wraps_right_when_the_view_has_no_room_for_the_next_turn() {
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
    // Ten rows for cards: two turns of four, and the row between.
    app.flow_mut().set_room(12);
    app.flow_mut().ensure_layout();
    let layout = app.flow().layout();
    let places: Vec<(usize, usize)> = layout
        .cards
        .iter()
        .map(|card| (card.column, card.top))
        .collect();
    assert_eq!(places, [(0, 0), (0, 5), (1, 0)]);
    assert!(layout.height <= 12, "{}", layout.height);
    let wires: Vec<_> = layout
        .wires
        .iter()
        .map(|w| (w.from, w.to, w.kind))
        .collect();
    assert_eq!(wires, [(1, 2, WireKind::Next)], "only the wrap is wired");

    // A taller view fits them all in one column.
    app.flow_mut().set_room(40);
    app.flow_mut().ensure_layout();
    let layout = app.flow().layout();
    assert!(layout.cards.iter().all(|card| card.column == 0));
    assert!(layout.wires.is_empty());
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
    assert_eq!(heads(&app), ["do both", "code", "test"]);
    assert_eq!(bands(&app), [0, 1, 2]);
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
    assert_eq!(cards(&app), [vec!["first", "third"], vec!["second"]]);
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
fn folding_a_turn_or_a_call_moves_its_wires_and_links_open_them() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(selected(&app), "aa:1");
    let layout = app.flow().layout();
    assert!(layout.cards[0].folded);
    assert_eq!(layout.cards[0].steps, 2);
    assert_eq!(app.flow().card_facts(0), "+2 steps");
    assert_eq!(
        heads(&app),
        ["add a greeting", "write greet()", "test greet()"]
    );
    let layout = app.flow().layout();
    assert!(
        layout
            .wires
            .iter()
            .any(|wire| (wire.from, wire.row, wire.to, wire.kind) == (0, None, 1, WireKind::Spawn)),
        "the task call's wire leaves from the turn: {:?}",
        layout.wires
    );

    // A link into the folded turn opens it again.
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), "bb:3");
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:6");
    assert_eq!(app.flow().layout().cards.len(), 5);
    assert_eq!(
        app.flow().selected_place(),
        Some((1, Some(0))),
        "on the task's row"
    );

    // On a model call, the call folds: its rows go, its wires leave from
    // its head.
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(selected(&app), "aa:2");
    let layout = app.flow().layout();
    assert!(layout.cards[1].folded && layout.cards[1].rows.is_empty());
    assert_eq!(app.flow().card_facts(1), "+1 tool");
    assert!(layout
        .wires
        .iter()
        .any(|wire| (wire.from, wire.row, wire.to) == (1, None, 3)));
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(selected(&app), "aa:6");
    assert!(!app.flow().layout().cards[1].folded, "the link opened it");

    press(&mut app, KeyCode::Char('z'));
    assert_eq!(
        heads(&app),
        ["add a greeting", "write greet()", "test greet()"],
        "every turn folded"
    );
    assert_eq!(selected(&app), "aa:1");
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.flow().layout().cards.len(), 5);
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

/// [`app_with_flow`] drawn by a front end in an 40 × 10 view.
fn hosted_app() -> App {
    let mut app = app_with_flow();
    app.set_host_canvas(true);
    press(&mut app, KeyCode::Home);
    let area = Rect::new(0, 0, 40, 10);
    let flow = app.flow_mut();
    let pan = flow.sync_view(40.0, 10.0);
    flow.set_view(Some(FlowView { area, pan }), true);
    app
}

#[test]
fn zoom_keeps_the_point_under_the_pointer_where_it_is() {
    let mut app = hosted_app();
    assert_eq!(app.flow().zoom(), 1.0);
    app.zoom_flow(2.0, Some((20.0, 6.0)));
    assert_eq!(app.flow().zoom(), 2.0);
    assert_eq!(app.flow().pan(), (10.0, 3.0), "(20, 6) stays under it");

    // Zoomed in, the view shows less of the canvas, and pans by canvas
    // cells.
    let flow = app.flow_mut();
    assert_eq!(flow.sync_view(40.0, 10.0), (10.0, 3.0));
    flow.pan_by(-4.0, 0.0);
    assert_eq!(flow.pan(), (6.0, 3.0));

    // Out to the least, around the view's middle, which may take the view
    // off the canvas.
    app.zoom_flow(0.01, None);
    assert_eq!(app.flow().zoom(), MIN_ZOOM);
    assert_eq!(app.flow().pan(), (-64.0, -14.5));
    app.zoom_flow(100.0, None);
    assert_eq!(app.flow().zoom(), MAX_ZOOM);
}

#[test]
fn the_zoom_keys_work_only_when_a_front_end_draws_the_canvas() {
    let mut app = app_with_flow();
    press(&mut app, KeyCode::Char('+'));
    app.zoom_flow(2.0, None);
    assert_eq!(app.flow().zoom(), 1.0, "the terminal does not zoom");

    let mut app = hosted_app();
    press(&mut app, KeyCode::Char('+'));
    assert_eq!(app.flow().zoom(), ZOOM_STEP);
    press(&mut app, KeyCode::Char('-'));
    press(&mut app, KeyCode::Char('-'));
    assert!((app.flow().zoom() - 1.0 / ZOOM_STEP).abs() < 1e-6);
    press(&mut app, KeyCode::Char('0'));
    assert!((app.flow().zoom() - 1.0).abs() < 1e-6);
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

    // The model call's marker folds the call; the turn's, the turn.
    let rect = app.flow().layout().card_rect(1);
    assert!(app.click_flow_at(cells(rect.x + 2) + 0.5, cells(rect.y) + 0.5));
    app.flow_mut().ensure_layout();
    assert!(app.flow().layout().cards[1].folded);
    assert!(!app.flow().layout().cards[0].folded);
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

#[test]
fn a_long_flow_draws_only_what_is_on_screen() {
    let nodes: Vec<FlowNode> = (0..u64::try_from(MAX_NODES).unwrap() / 3)
        .flat_map(|turn| {
            let at = turn * 10;
            [
                node(LEAD, 3 * turn + 1, at, FlowKind::Prompt, "go"),
                node(LEAD, 3 * turn + 2, at + 1, FlowKind::Model, "sonnet"),
                node(LEAD, 3 * turn + 3, at + 2, FlowKind::Tool, "read_file"),
            ]
        })
        .collect();
    let mut app = app_with(vec![lane(LEAD, "Lead", None)], nodes);
    app.flow_mut().set_room(40);
    let started = std::time::Instant::now();
    app.flow_mut().ensure_layout();
    let laid_out = started.elapsed();
    let layout = app.flow().layout();
    assert_eq!(layout.blocks.len(), MAX_NODES / 3);
    assert!(layout.columns > 100, "{}", layout.columns);

    let (xs, ys) = (2000..2120, 0..40);
    let cards = layout.visible_cards(&xs, &ys);
    assert!(!cards.is_empty() && cards.len() < 40, "{}", cards.len());
    for card in &cards {
        assert!(layout.card_rect(*card).meets(&xs, &ys));
    }
    let wires = layout.visible_wires(&xs, &ys);
    assert!(wires.len() < 20, "{}", wires.len());
    let everything = (0..layout.width, 0..layout.height);
    assert_eq!(
        layout.visible_cards(&everything.0, &everything.1).len(),
        layout.cards.len()
    );
    assert!(laid_out.as_millis() < 2000, "{laid_out:?}");
}

fn select(app: &mut App, id: &str) {
    let flow = app.flow_mut();
    let at = flow.index[id];
    flow.select_node(at);
    flow.ensure_layout();
}

#[test]
fn folding_an_agent_puts_its_turns_on_one_card_and_its_wires_still_join() {
    let (lanes, nodes) = three_turns();
    let mut app = app_with(lanes, nodes);
    let owned = |app: &App| -> Vec<Vec<String>> {
        cards(app)
            .into_iter()
            .map(|card| card.into_iter().map(str::to_string).collect())
            .collect()
    };
    let before = owned(&app);
    select(&mut app, "aa:4");
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(
        cards(&app),
        [
            vec!["first", "first", "second", "third"],
            vec!["code", "coded"],
            vec!["test", "tested"],
        ],
        "the lead's turns are rows of one card"
    );
    let layout = app.flow().layout();
    assert_eq!(layout.cards[0].kind, CardKind::Agent);
    assert_eq!(app.flow().card_kind_label(0), "agent");
    assert_eq!(app.flow().card_facts(0), "Lead · 3 turns");
    let spawns: Vec<(usize, usize)> = layout
        .wires
        .iter()
        .filter(|wire| wire.kind == WireKind::Spawn)
        .map(|wire| (wire.from, wire.to))
        .collect();
    assert_eq!(
        spawns,
        [(0, 1), (0, 2)],
        "both tasks leave the agent's card"
    );
    assert_eq!(selected(&app), "aa:4", "the turn selected is its row");
    assert_eq!(app.flow().selected_place(), Some((0, Some(1))));

    // Enter on a row opens the agent again, on that turn.
    press(&mut app, KeyCode::Enter);
    assert_eq!(owned(&app), before);
    assert_eq!(selected(&app), "aa:4");

    // C folds all but the selected agent and its parent; x opens them.
    select(&mut app, "bb:1");
    press(&mut app, KeyCode::Char('C'));
    let kinds: Vec<CardKind> = app.flow().layout().cards.iter().map(|c| c.kind).collect();
    assert_eq!(kinds.iter().filter(|k| **k == CardKind::Agent).count(), 1);
    assert_eq!(
        heads(&app).last(),
        Some(&"test"),
        "only the tester is folded"
    );
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(owned(&app), before);
}

#[test]
fn the_name_above_an_agents_cards_folds_it() {
    let mut app = app_with_flow();
    let layout = app.flow().layout();
    let span = *layout.spans.iter().find(|span| span.lane == 1).unwrap();
    let (x, y) = (
        cells(FlowLayout::column_x(span.first) + 1) + 0.5,
        cells(layout.bands[span.band].y) + 0.5,
    );
    assert_eq!(layout.spot_at(x, y), Some(Spot::Lane(1)));
    assert!(app.click_flow_at(x, y));
    app.flow_mut().ensure_layout();
    assert!(app.flow().lane_is_folded(1));
    let coder = app.flow().layout().lane_turns(1)[0];
    assert_eq!(app.flow().layout().cards[coder].kind, CardKind::Agent);
    assert!(app.click_flow_at(x, y));
    app.flow_mut().ensure_layout();
    assert!(!app.flow().lane_is_folded(1));
}

const HOST: &str = "dd";

fn run_lane(agent: &str, name: &str, arm: usize, group: &str, check: bool) -> FlowLane {
    FlowLane {
        bench: Some(FlowBench {
            group: group.into(),
            run: name.into(),
            arm,
            arm_label: if arm == 0 { "gate" } else { "opus" }.into(),
            check_passed: Some(check),
            cost_usd: 0.25,
            wall_ms: 61_000,
            error: None,
        }),
        ..lane(agent, name, Some(if arm == 0 { "ea" } else { "eb" }))
    }
}

/// A lead whose benchmark ran two tasks on two arms; `a-1` gave a coder
/// work, and `b-1` failed its check after a model call failed.
fn benchmark() -> (Vec<FlowLane>, Vec<FlowNode>) {
    let arm = |agent: &str, name: &str| FlowLane {
        bench_arm: true,
        ..lane(agent, name, Some(HOST))
    };
    let lanes = vec![
        lane(LEAD, "Lead", None),
        lane(HOST, "Compare #1", Some(LEAD)),
        arm("ea", "A · gate"),
        arm("eb", "B · opus"),
        run_lane("a1", "a-1", 0, "t1", true),
        run_lane("b1", "b-1", 1, "t1", false),
        run_lane("a2", "a-2", 0, "t2", true),
        run_lane("b2", "b-2", 1, "t2", true),
        lane(CODER, "coder #1", Some("a1")),
    ];
    let mut failed = node("b1", 2, 150, FlowKind::Model, "opus");
    failed.status = TraceNodeStatus::Error;
    let nodes = vec![
        node(LEAD, 1, 50, FlowKind::Prompt, "benchmark it"),
        node("a1", 1, 95, FlowKind::Gate, "haiku"),
        node("a1", 2, 100, FlowKind::Prompt, "write the file"),
        node("a1", 3, 110, FlowKind::Model, "sonnet"),
        linked(
            node("a1", 4, 120, FlowKind::Tool, "task coder"),
            "spawn",
            "bb:1",
        ),
        node(CODER, 1, 130, FlowKind::Prompt, "write it"),
        node(CODER, 2, 150, FlowKind::Reply, "written"),
        node("a1", 5, 200, FlowKind::Reply, "done"),
        node("b1", 1, 100, FlowKind::Prompt, "write the file"),
        failed,
        node("b1", 3, 300, FlowKind::Reply, "gave up"),
        node("a2", 1, 400, FlowKind::Prompt, "rename it"),
        node("a2", 2, 450, FlowKind::Reply, "renamed"),
        node("b2", 1, 400, FlowKind::Prompt, "rename it"),
        node("b2", 2, 500, FlowKind::Reply, "renamed"),
        node(LEAD, 2, 600, FlowKind::Reply, "a won"),
    ];
    (lanes, nodes)
}

fn rows(app: &mut App) -> Vec<(String, usize, String)> {
    app.flow_mut().ensure_timeline();
    app.flow()
        .timeline()
        .timeline()
        .rows
        .iter()
        .map(|row| (row.name.clone(), row.depth, row.note.clone()))
        .collect()
}

/// [`app_with`], on the TIME tab.
fn time_with(lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) -> App {
    let mut app = app_with(lanes, nodes);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Time);
    app
}

fn open_lanes(app: &App) -> Vec<&str> {
    let flow = app.flow();
    flow.lanes()
        .iter()
        .enumerate()
        .filter(|(lane, _)| !flow.lane_is_folded(*lane))
        .map(|(_, lane)| lane.name.as_str())
        .collect()
}

#[test]
fn the_time_tab_lists_a_benchmarks_runs_by_task() {
    let (lanes, nodes) = benchmark();
    let mut app = time_with(lanes, nodes);
    assert_eq!(app.flow().mode(), FlowMode::Timeline);
    let row = |name: &str, depth: usize, note: &str| (name.to_string(), depth, note.to_string());
    assert_eq!(
        rows(&mut app),
        [
            row("Lead", 0, ""),
            row("Compare #1", 1, ""),
            row("t1", 2, "A 1/1 · B 0/1"),
            row("a-1", 3, "gate"),
            row("b-1", 3, "opus"),
            row("t2", 2, "A 1/1 · B 1/1"),
            row("a-2", 3, "gate"),
            row("b-2", 3, "opus"),
        ],
        "the arms give no row, and the coder is folded under its run"
    );
    let timeline = app.flow().timeline().timeline();
    let (start, end) = (timeline.start_ms, timeline.end_ms);
    assert_eq!((start, end), (50, 600));
    let a_1 = &timeline.rows[3];
    assert!(a_1.folds && a_1.folded);
    assert_eq!(
        a_1.hidden,
        [(130, 150)],
        "the coder's time shows on its run"
    );
    assert_eq!(a_1.spans.len(), 1);
    assert_eq!((a_1.spans[0].start_ms, a_1.spans[0].end_ms), (95, 200));
    assert_eq!(a_1.spans[0].marks[0].kind, MarkKind::Gate);
    assert_eq!(a_1.facts.check, Some(true));
    assert_eq!(a_1.facts.cost_usd, Some(0.25));
    let b_1 = &timeline.rows[4];
    assert_eq!(b_1.status, TraceNodeStatus::Error);
    assert_eq!(b_1.spans[0].status, TraceNodeStatus::Error);
    assert!(b_1.spans[0].marks.iter().any(|m| m.kind == MarkKind::Error));
    assert_eq!(
        timeline.rows[2].status,
        TraceNodeStatus::Error,
        "t1 had a failure"
    );
    assert_eq!(timeline.rows[2].facts.cost_usd, Some(0.5));

    // Unfolding a-1 shows its coder; e keeps only what failed.
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(rows(&mut app)[4], row("coder #1", 4, ""));
    press(&mut app, KeyCode::Char('e'));
    let names: Vec<String> = rows(&mut app).into_iter().map(|(name, ..)| name).collect();
    assert_eq!(names, ["Lead", "Compare #1", "t1", "b-1"]);
    press(&mut app, KeyCode::Char('e'));

    // z folds every row under the top one.
    press(&mut app, KeyCode::Char('z'));
    let names: Vec<String> = rows(&mut app).into_iter().map(|(name, ..)| name).collect();
    assert_eq!(names, ["Lead", "Compare #1"]);
}

#[test]
fn enter_on_a_turn_opens_its_cards_with_the_other_agents_folded() {
    let (lanes, nodes) = benchmark();
    let mut app = time_with(lanes, nodes);
    app.flow_mut().ensure_timeline();
    for _ in 0..4 {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Char('l'));
    let state = app.flow().timeline();
    let (row, span) = state.selected_span().expect("b-1's turn");
    assert_eq!(state.timeline().rows[row].name, "b-1");
    assert_eq!(span, 0);

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    assert_eq!(selected(&app), "b1:1");
    assert_eq!(
        open_lanes(&app),
        ["B · opus", "b-1"],
        "the run and the arm that ran it"
    );
    let flow = app.flow();
    let card = flow.selected_card().unwrap();
    assert_eq!(flow.layout().cards[card].kind, CardKind::Turn);

    // Esc goes back to the timeline, on the same turn.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.chat_tab(), ChatTab::Time);
    let state = app.flow().timeline();
    assert_eq!(state.selected_span().map(|(row, _)| row), Some(row));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.chat_tab(), ChatTab::Chat);
}

#[test]
fn graph_and_time_are_tabs_of_the_same_flow() {
    let mut app = app_with_flow();
    assert_eq!(app.flow().mode(), FlowMode::Canvas);
    select(&mut app, "bb:2");
    let shift = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT));
    shift(&mut app, KeyCode::Right);
    assert_eq!(app.chat_tab(), ChatTab::Time);
    let state = app.flow().timeline();
    let row = state.selected_row().unwrap();
    assert_eq!(state.timeline().rows[row].name, "coder #1");
    assert_eq!(state.selected_span(), Some((row, 0)));

    shift(&mut app, KeyCode::Left);
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    assert_eq!(selected(&app), "bb:2", "the same flow, still selected");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.chat_tab(), ChatTab::Chat, "not opened from TIME");

    let tabs: Vec<ChatTab> = (0..5)
        .map(|_| {
            shift(&mut app, KeyCode::Right);
            app.chat_tab()
        })
        .collect();
    assert_eq!(
        tabs,
        [
            ChatTab::Terminal,
            ChatTab::Graph,
            ChatTab::Time,
            ChatTab::Bench,
            ChatTab::Chat
        ]
    );
}

#[test]
fn a_big_flow_opens_with_every_agent_but_the_one_on_screen_folded() {
    let (lanes, nodes) = benchmark();
    let mut app = app_with(lanes, nodes);
    assert_eq!(app.chat_tab(), ChatTab::Graph, "a benchmark opens on cards");
    assert!(app.flow().is_big());
    assert_eq!(open_lanes(&app), ["Lead"]);
    let layout = app.flow().layout();
    let agents = layout
        .cards
        .iter()
        .filter(|card| card.kind == CardKind::Agent)
        .count();
    assert_eq!(agents, 5, "a card for each agent that worked");

    // One opens on its own; x opens them all.
    select(&mut app, "a1:2");
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(open_lanes(&app), ["Lead", "a-1"]);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(open_lanes(&app).len(), 9);
    press(&mut app, KeyCode::Char('C'));
    assert_eq!(open_lanes(&app), ["A · gate", "a-1"]);
}

#[test]
fn a_flow_that_grows_big_folds_only_the_agents_that_come_after() {
    let mut app = app_with_flow();
    assert!(!app.flow().is_big());
    select(&mut app, "bb:1");
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(open_lanes(&app), ["Lead", "tester #1"]);

    let more = (0..3).map(|n| lane(&format!("e{n}"), &format!("tester #{}", n + 2), Some(LEAD)));
    app.process_command(UiCommand::FlowUpsert {
        agent_id: LEAD.into(),
        lanes: more.collect(),
        nodes: vec![node("e0", 1, 300, FlowKind::Prompt, "test more")],
    });
    app.flow_mut().ensure_layout();
    assert!(app.flow().is_big());
    assert_eq!(
        open_lanes(&app),
        ["Lead", "tester #1"],
        "the agents on screen stay as they were"
    );
}

#[test]
fn the_timeline_draws_a_row_an_agent_with_its_turns_as_bars() {
    let (lanes, nodes) = benchmark();
    let mut app = time_with(lanes, nodes);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
    let theme = crate::Theme::cyber();
    terminal
        .draw(|frame| crate::render(frame, &mut app, &theme))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let find = |text: &str| {
        screen
            .iter()
            .find(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no {text:?} in\n{}", screen.join("\n")))
    };
    assert!(find("agents").contains("cost"));
    assert!(
        find("agents").contains("100ms"),
        "the axis ticks under a second"
    );
    assert!(find("▾ ✗ t1 A 1/1 · B 0/1").contains("$0.500"));
    let a_1 = find("▸ ✓ a-1 gate");
    assert!(a_1.contains('━') && a_1.contains('◆'), "{a_1}");
    assert!(a_1.contains("1m01s"));
    let b_1 = find("✗ b-1 opus");
    assert!(
        b_1.matches('✗').count() >= 3,
        "status, bar and check: {b_1}"
    );
    find("Lead · 1 turn · 0 calls");
    find("Enter opens its cards");
    assert_eq!(app.flow_canvas(), None, "every front end draws it in cells");
}
