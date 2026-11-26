
use super::*;
use crate::events::{LogRunView, LogStatus, LogStepView, LogTurnView};

fn step(seq: u64, spawned: Vec<usize>) -> LogStepView {
    LogStepView {
        seq,
        kind: "model".into(),
        label: format!("step {seq}"),
        fields: vec![("seq".into(), seq.to_string())],
        spawned,
        json: format!("{{\"seq\":{seq}}}"),
        ..LogStepView::default()
    }
}

fn run(id: &str, depth: usize, turns: Vec<LogTurnView>) -> LogRunView {
    LogRunView {
        run_id: id.into(),
        title: id.into(),
        depth,
        turns,
        json: format!("{{\"run\":\"{id}\"}}"),
        ..LogRunView::default()
    }
}

fn turn(n: u32, steps: Vec<LogStepView>) -> LogTurnView {
    LogTurnView {
        turn: n,
        steps,
        status: LogStatus::Ok,
        json: format!("{{\"turn\":{n}}}"),
        ..LogTurnView::default()
    }
}

fn view() -> LogView {
    LogView {
        agent_id: "a".into(),
        path: "/tmp/runs/r/log.json".into(),
        json: "{\"whole\":true}".into(),
        runs: vec![
            run(
                "root",
                0,
                vec![
                    turn(1, vec![step(1, vec![]), step(2, vec![1])]),
                    turn(2, vec![step(5, vec![])]),
                ],
            ),
            run("child", 1, vec![turn(1, vec![step(1, vec![])])]),
        ],
    }
}

fn panels(state: &LogState) -> Vec<LogPanel> {
    state.rows().into_iter().map(|r| r.panel).collect()
}

#[test]
fn a_new_log_opens_on_its_last_turn_with_its_details_shut() {
    let mut state = LogState::default();
    state.load(view());
    assert_eq!(
        panels(&state),
        vec![
            LogPanel::Details("root".into()),
            LogPanel::Turn("root".into(), 1),
            LogPanel::Turn("root".into(), 2),
            LogPanel::Step("root".into(), 5),
        ],
        "the run asked for has no row of its own"
    );
    assert_eq!(state.selected, Some(LogPanel::Details("root".into())));
}

#[test]
fn a_subagent_sits_under_the_step_that_spawned_it() {
    let mut state = LogState::default();
    state.load(view());
    state.toggle(&LogPanel::Turn("root".into(), 1));
    let rows = state.rows();
    let at = rows
        .iter()
        .position(|r| r.panel == LogPanel::Step("root".into(), 2))
        .unwrap();
    assert_eq!(rows[at + 1].panel, LogPanel::Run("child".into()));
    assert!(rows[at + 1].depth > rows[at].depth);
    state.expand_all();
    assert!(panels(&state).contains(&LogPanel::Step("child".into(), 1)));
}

#[test]
fn runs_no_turn_started_follow_the_turns() {
    let mut group = run("agent-a", 0, vec![]);
    group.runs = vec![1, 2];
    let mut state = LogState::default();
    state.load(LogView {
        runs: vec![group, run("a-1", 1, vec![]), run("b-1", 1, vec![])],
        ..view()
    });
    let rows = state.rows();
    let listed: Vec<(&LogPanel, usize)> = rows.iter().map(|r| (&r.panel, r.depth)).collect();
    let (a, b) = (LogPanel::Run("a-1".into()), LogPanel::Run("b-1".into()));
    assert_eq!(&listed[listed.len() - 2..], [(&a, 0), (&b, 0)]);
}

#[test]
fn closing_a_panel_keeps_the_selection_on_screen() {
    let mut state = LogState::default();
    state.load(view());
    state.selected = Some(LogPanel::Step("root".into(), 5));
    state.toggle(&LogPanel::Turn("root".into(), 2));
    let shown = panels(&state);
    assert!(!shown.contains(&LogPanel::Step("root".into(), 5)));
    assert!(shown.contains(state.selected.as_ref().unwrap()));
}

#[test]
fn the_run_asked_for_stays_open() {
    let mut state = LogState::default();
    state.load(view());
    let before = panels(&state);
    state.toggle(&LogPanel::Run("root".into()));
    state.collapse_all();
    assert!(state.is_expanded(&LogPanel::Run("root".into())));
    assert_eq!(panels(&state)[0], before[0]);
}

#[test]
fn reloading_the_same_run_keeps_what_was_open() {
    let mut state = LogState::default();
    state.load(view());
    state.toggle(&LogPanel::Details("root".into()));
    let mut next = view();
    next.runs[0].turns.push(turn(3, vec![step(9, vec![])]));
    state.load(next);
    assert!(state.is_expanded(&LogPanel::Details("root".into())));
    assert!(
        state.is_expanded(&LogPanel::Turn("root".into(), 3)),
        "a new turn opens"
    );
}

#[test]
fn copy_log_copies_the_whole_json() {
    let mut app = App::new();
    app.log.load(view());
    app.copy_whole_log();
    let buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
    assert_eq!(
        app.take_copy_text(&buffer).as_deref(),
        Some("{\"whole\":true}")
    );
    app.log.selected = Some(LogPanel::Turn("root".into(), 2));
    app.copy_selected_log();
    assert_eq!(app.take_copy_text(&buffer).as_deref(), Some("{\"turn\":2}"));
}

#[test]
fn the_log_of_another_agent_is_ignored() {
    let mut app = App::new();
    app.active_agent_id = "b".into();
    app.cmd_log_view(view());
    assert!(app.log.view.is_none());
}

/// The agents whose logs were asked for since the last call.
fn logs_asked(rx: &mut tokio::sync::mpsc::Receiver<UiEvent>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::Log { agent_id, .. } => Some(agent_id),
            _ => None,
        })
        .collect()
}

fn a_second_ago() -> Option<Instant> {
    Instant::now().checked_sub(LIVE_EVERY)
}

#[test]
fn a_recorded_step_reloads_the_open_log_at_most_once_a_second() {
    use crate::events::UiCommand;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "a".into();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "root".into(),
        nodes: Vec::new(),
    });
    app.refresh_live_log();
    assert!(logs_asked(&mut rx).is_empty(), "the tab is shut");

    app.show_chat_tab(ChatTab::Log);
    app.cmd_log_view(view());
    assert_eq!(logs_asked(&mut rx), ["a"]);
    app.process_command(UiCommand::GraphUpsert {
        run_id: "root".into(),
        nodes: Vec::new(),
    });
    app.refresh_live_log();
    assert!(logs_asked(&mut rx).is_empty(), "not again within a second");
    app.log.requested = a_second_ago();
    app.refresh_live_log();
    assert_eq!(logs_asked(&mut rx), ["a"]);

    app.cmd_log_view(view());
    app.log.requested = a_second_ago();
    app.refresh_live_log();
    assert!(logs_asked(&mut rx).is_empty(), "nothing new was recorded");
}

#[test]
fn watching_the_newest_step_follows_the_steps_that_come_in() {
    let mut state = LogState::default();
    state.load(view());
    state.selected = Some(LogPanel::Step("root".into(), 5));
    let mut next = view();
    next.runs[0].turns[1].steps.push(step(6, vec![]));
    state.load(next);
    assert_eq!(state.selected, Some(LogPanel::Step("root".into(), 6)));

    state.selected = Some(LogPanel::Turn("root".into(), 2));
    let mut next = view();
    next.runs[0].turns[1].steps.push(step(7, vec![]));
    state.load(next);
    assert_eq!(state.selected, Some(LogPanel::Turn("root".into(), 2)));
}

#[test]
fn the_log_of_another_agent_opens_once_the_switch_to_it_lands() {
    use crate::events::UiCommand;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "a".into();
    app.open_log_of("b".into());
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "b"));
    assert_ne!(app.chat_tab(), ChatTab::Log);
    app.process_command(UiCommand::SetActiveAgent("b".into()));
    assert_eq!(app.chat_tab(), ChatTab::Log);
    assert_eq!(logs_asked(&mut rx), ["b"]);

    app.show_chat_tab(ChatTab::Chat);
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    assert_eq!(
        app.chat_tab(),
        ChatTab::Chat,
        "the wait ended with the switch"
    );
}

#[test]
fn switching_agents_parks_the_log_and_brings_it_back() {
    use crate::events::UiCommand;
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    app.show_chat_tab(ChatTab::Log);
    app.cmd_log_view(view());
    app.log.selected = Some(LogPanel::Step("root".into(), 5));

    app.process_command(UiCommand::SetActiveAgent("b".into()));
    assert!(app.log.view.is_none(), "a's log is not b's");
    assert!(app.log.shown("b").is_none());

    app.process_command(UiCommand::SetActiveAgent("a".into()));
    assert!(app.log.is_held(), "a's old log waits for the fresh one");
    assert!(app.log.shown("a").is_none(), "nothing old is drawn");
    app.cmd_log_view(view());
    assert!(app.log.shown("a").is_some());
    assert_eq!(
        app.log.selected,
        Some(LogPanel::Step("root".into(), 5)),
        "the fresh log keeps a's folds and selection"
    );
    assert!(app.log.fading().is_some(), "and fades in");
}

#[test]
fn coming_back_to_the_tab_hides_the_old_log_until_the_fresh_one() {
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "a".into();
    app.show_chat_tab(ChatTab::Log);
    app.cmd_log_view(view());
    app.log.revealed_at = None;

    app.show_chat_tab(ChatTab::Chat);
    app.show_chat_tab(ChatTab::Log);
    assert!(app.log.shown("a").is_none(), "the old log is not flashed");
    assert!(app.log.is_loading());

    app.cmd_log_view(view());
    assert!(app.log.shown("a").is_some());
    assert!(app.log.fading().is_some());

    // A live reload on the open tab keeps the log on screen.
    app.request_log();
    assert!(app.log.shown("a").is_some());
}

#[test]
fn an_old_notice_is_not_shown_while_the_log_is_asked_for_again() {
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "a".into();
    app.show_chat_tab(ChatTab::Log);
    app.cmd_log_notice("a", "no run log yet".into());
    app.show_chat_tab(ChatTab::Chat);
    app.show_chat_tab(ChatTab::Log);
    assert!(app.log.is_held() && app.log.is_loading());
}

#[test]
fn a_notice_during_a_reload_keeps_the_log_on_screen() {
    let mut app = App::new();
    app.active_agent_id = "a".into();
    app.cmd_log_view(view());
    app.request_log();
    app.cmd_log_notice("a", "could not read the run".into());
    assert!(app.log.shown("a").is_some());
    assert!(!app.log.is_loading());
}

#[test]
fn a_log_fades_in_only_where_none_was_shown() {
    let mut state = LogState::default();
    state.load(view());
    assert!(state.fading().is_some(), "the first log fades in");
    state.revealed_at = None;
    state.load(view());
    assert!(state.revealed_at.is_none(), "a reload doesn't fade");
}

#[test]
fn the_tab_animates_only_while_it_fades() {
    let mut app = App::new();
    app.active_agent_id = "a".into();
    assert!(!app.log_animating(), "the tab is shut");
    app.show_chat_tab(ChatTab::Log);
    assert!(!app.log_animating(), "a blank tab waiting is still");
    app.cmd_log_view(view());
    assert!(app.log_animating(), "fading in");
    app.log.revealed_at = Instant::now().checked_sub(FADE_IN);
    assert!(!app.log_animating(), "a finished run on screen is still");
}

#[test]
fn l_in_a_benchmark_shows_the_log_of_the_picked_run() {
    use crate::app::{AgentsTab, PanelFocus};
    use crate::events::{BenchArm, BenchRun, BenchView, UiCommand};
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    app.active_agent_id = "a".into();
    app.process_command(UiCommand::BenchList {
        benches: vec![BenchView {
            id: "ab".into(),
            name: "Compare #1".into(),
            arms: [BenchArm::default(), BenchArm::default()],
            runs: vec![BenchRun {
                name: "a-1".into(),
                agent_id: "run".into(),
                ..BenchRun::default()
            }],
            ..BenchView::default()
        }],
    });
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Chat;
    app.handle_key(KeyEvent::from(KeyCode::Right));
    while rx.try_recv().is_ok() {}
    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "run"));
    assert_eq!(app.agents_tab(), AgentsTab::Projects, "where its log is");
}
