use super::*;

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
