use super::*;

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
    let mut app = two_priced_calls();
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
