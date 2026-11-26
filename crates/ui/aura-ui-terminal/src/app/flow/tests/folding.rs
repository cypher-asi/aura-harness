use super::*;

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
    app.show_chat_tab(ChatTab::Graph);
    assert_eq!(app.flow().agent_id(), CODER, "the flow left the lead");
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
fn folding_a_turn_moves_its_wires_and_a_link_opens_it() {
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
}

#[test]
fn folding_a_call_moves_its_wires_to_its_head_and_a_link_opens_it() {
    let mut app = app_with_flow();
    fold_turn_and_link_back(&mut app);
    assert_eq!(selected(&app), "aa:6");

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
}

#[test]
fn z_folds_every_turn_and_opens_them_again() {
    let mut app = app_with_flow();
    fold_turn_and_link_back(&mut app);
    fold_call_and_link_back(&mut app);
    assert_eq!(selected(&app), "aa:6");

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
fn folding_an_agent_puts_its_turns_on_one_card_and_its_wires_still_join() {
    let app = lead_folded();
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
}

#[test]
fn enter_on_a_folded_agents_row_opens_it_on_that_turn() {
    let (lanes, nodes) = three_turns();
    let before = owned_cards(&app_with(lanes, nodes));
    let mut app = lead_folded();

    // Enter on a row opens the agent again, on that turn.
    press(&mut app, KeyCode::Enter);
    assert_eq!(owned_cards(&app), before);
    assert_eq!(selected(&app), "aa:4");
}

#[test]
fn shift_c_folds_all_but_the_selected_agent_and_its_parent_and_x_opens_them() {
    let (lanes, nodes) = three_turns();
    let before = owned_cards(&app_with(lanes, nodes));
    let mut app = lead_folded();
    press(&mut app, KeyCode::Enter);

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
    assert_eq!(owned_cards(&app), before);
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
