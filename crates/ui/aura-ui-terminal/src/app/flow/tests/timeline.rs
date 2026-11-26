use super::*;

#[test]
fn the_time_tab_lists_a_benchmarks_runs_by_task() {
    let mut app = benchmark_on_time();
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
}

#[test]
fn a_benchmarks_runs_show_their_time_checks_cost_and_failures() {
    let app = benchmark_on_time();
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
}

#[test]
fn a_benchmarks_runs_unfold_filter_to_failures_and_fold() {
    let mut app = benchmark_on_time();
    let row = |name: &str, depth: usize, note: &str| (name.to_string(), depth, note.to_string());

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
            ChatTab::Log,
            ChatTab::Chat
        ]
    );
}

#[test]
fn coming_back_on_time_keeps_where_the_timeline_was() {
    let (mut app, moved) = time_moved_off_the_cards();

    app.process_command(UiCommand::SetActiveAgent(CODER.into()));
    assert_eq!(app.chat_tab(), ChatTab::Chat, "the coder was never shown");

    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    assert_eq!(app.chat_tab(), ChatTab::Time);
    app.flow_mut().ensure_timeline();
    assert_eq!(
        timeline_place(&app),
        moved,
        "where it was moved to, not back on the selected card"
    );
}

#[test]
fn another_agent_opening_time_leaves_the_last_ones_timeline_alone() {
    let (mut app, moved) = time_moved_off_the_cards();

    // The lead's flow is still on hand when the coder opens TIME.
    app.process_command(UiCommand::SetActiveAgent(CODER.into()));
    app.show_chat_tab(ChatTab::Time);
    assert_eq!(app.flow().agent_id(), CODER);

    app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
    assert_eq!(app.chat_tab(), ChatTab::Time);
    assert_eq!(app.flow().agent_id(), LEAD);
    let (lanes, nodes) = delegation();
    app.process_command(UiCommand::FlowLoad {
        agent_id: LEAD.into(),
        lanes,
        nodes,
    });
    app.flow_mut().ensure_timeline();
    assert_eq!(
        timeline_place(&app),
        moved,
        "the lead's timeline was put away where it was moved to"
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
