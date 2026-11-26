use super::*;

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
