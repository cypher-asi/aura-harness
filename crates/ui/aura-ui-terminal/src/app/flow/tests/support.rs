use super::*;

pub(super) fn lane(agent: &str, name: &str, parent: Option<&str>) -> FlowLane {
    FlowLane {
        agent_id: agent.into(),
        name: name.into(),
        parent: parent.map(str::to_string),
        ..FlowLane::default()
    }
}

pub(super) fn node(agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str) -> FlowNode {
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

pub(super) fn linked(mut from: FlowNode, kind: &str, to: &str) -> FlowNode {
    from.links.push((kind.into(), to.into()));
    from
}

/// A `task` call at `seq` that starts `agent` and gets back `result`.
pub(super) fn task(seq: u64, at_ms: u64, label: &str, agent: &str, result: &str) -> FlowNode {
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
pub(super) fn delegation() -> (Vec<FlowLane>, Vec<FlowNode>) {
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
pub(super) fn three_turns() -> (Vec<FlowLane>, Vec<FlowNode>) {
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

pub(super) fn app_with(lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) -> App {
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

pub(super) fn app_with_flow() -> App {
    let (lanes, nodes) = delegation();
    app_with(lanes, nodes)
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
    app.flow_mut().ensure_layout();
}

pub(super) fn selected(app: &App) -> &str {
    app.flow()
        .selected_node()
        .map_or("", |node| node.id.as_str())
}

/// Each card's head and rows, by label, in time order.
pub(super) fn cards(app: &App) -> Vec<Vec<&str>> {
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

pub(super) fn heads(app: &App) -> Vec<&str> {
    cards(app).into_iter().map(|card| card[0]).collect()
}

pub(super) fn bands(app: &App) -> Vec<usize> {
    let layout = app.flow().layout();
    layout.cards.iter().map(|card| card.band).collect()
}

/// [`app_with_flow`] drawn by a front end in an 40 × 10 view.
pub(super) fn hosted_app() -> App {
    let mut app = app_with_flow();
    app.set_host_canvas(true);
    press(&mut app, KeyCode::Home);
    let area = Rect::new(0, 0, 40, 10);
    let flow = app.flow_mut();
    let pan = flow.sync_view(40.0, 10.0);
    flow.set_view(Some(FlowView { area, pan }), true);
    app
}

pub(super) fn select(app: &mut App, id: &str) {
    let flow = app.flow_mut();
    let at = flow.index[id];
    flow.select_node(at);
    flow.ensure_layout();
}

pub(super) const HOST: &str = "dd";

pub(super) fn run_lane(agent: &str, name: &str, arm: usize, group: &str, check: bool) -> FlowLane {
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
pub(super) fn benchmark() -> (Vec<FlowLane>, Vec<FlowNode>) {
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

pub(super) fn rows(app: &mut App) -> Vec<(String, usize, String)> {
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
pub(super) fn time_with(lanes: Vec<FlowLane>, nodes: Vec<FlowNode>) -> App {
    let mut app = app_with(lanes, nodes);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.chat_tab(), ChatTab::Time);
    app
}

pub(super) fn open_lanes(app: &App) -> Vec<&str> {
    let flow = app.flow();
    flow.lanes()
        .iter()
        .enumerate()
        .filter(|(lane, _)| !flow.lane_is_folded(*lane))
        .map(|(_, lane)| lane.name.as_str())
        .collect()
}

/// The timeline's selected row, and its selected turn.
pub(super) type TimelinePlace = (Option<usize>, Option<(usize, usize)>);

pub(super) fn timeline_place(app: &App) -> TimelinePlace {
    let state = app.flow().timeline();
    (state.selected_row(), state.selected_span())
}

/// [`app_with_flow`] on the TIME tab, its timeline moved off the turn of
/// the card selected; and where the timeline was moved to.
pub(super) fn time_moved_off_the_cards() -> (App, TimelinePlace) {
    let mut app = app_with_flow();
    app.show_chat_tab(ChatTab::Time);
    assert_eq!(selected(&app), "aa:10");
    let synced = timeline_place(&app);
    assert!(synced.1.is_some(), "TIME opens on the selected card's turn");

    press(&mut app, KeyCode::Char('j'));
    let moved = timeline_place(&app);
    assert_ne!(moved.0, synced.0, "j moved the timeline to another row");
    assert_ne!(moved.1, synced.1, "and to another turn");
    assert_eq!(selected(&app), "aa:10", "the cards' selection stays");
    (app, moved)
}

/// `node` with a `tokens · time` detail and what it used.
pub(super) fn used(
    mut node: FlowNode,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    ms: u64,
) -> FlowNode {
    node.detail = "tokens · time".into();
    node.usage = Some(FlowUsage {
        input_tokens,
        output_tokens,
        cost_usd,
        duration_ms: Some(ms),
    });
    node
}

/// A lead turn with two model calls, the first priced, a tool call between.
pub(super) fn two_priced_calls() -> App {
    app_with(
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
    )
}

/// Folds the first turn, then follows links into it, which opens it again.
pub(super) fn fold_turn_and_link_back(app: &mut App) {
    press(app, KeyCode::Home);
    press(app, KeyCode::Char(' '));
    press(app, KeyCode::Char(']'));
    press(app, KeyCode::Down);
    press(app, KeyCode::Down);
    press(app, KeyCode::Char('f'));
}

/// Folds the selected model call, then follows links back into it.
pub(super) fn fold_call_and_link_back(app: &mut App) {
    press(app, KeyCode::Char(' '));
    press(app, KeyCode::Char(']'));
    press(app, KeyCode::Down);
    press(app, KeyCode::Down);
    press(app, KeyCode::Char('f'));
}

/// Each card's head and rows, owned.
pub(super) fn owned_cards(app: &App) -> Vec<Vec<String>> {
    cards(app)
        .into_iter()
        .map(|card| card.into_iter().map(str::to_string).collect())
        .collect()
}

/// [`three_turns`] with the lead folded from its second turn.
pub(super) fn lead_folded() -> App {
    let (lanes, nodes) = three_turns();
    let mut app = app_with(lanes, nodes);
    select(&mut app, "aa:4");
    press(&mut app, KeyCode::Char('c'));
    app
}

/// [`benchmark`] on the TIME tab, its timeline laid out.
pub(super) fn benchmark_on_time() -> App {
    let (lanes, nodes) = benchmark();
    let mut app = time_with(lanes, nodes);
    app.flow_mut().ensure_timeline();
    app
}
