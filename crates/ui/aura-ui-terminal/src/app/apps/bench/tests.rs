
use super::*;
use crate::events::{AgentSummary, BenchArm, BenchRun, UiCommand};
use tokio::sync::mpsc;

fn view(id: &str) -> BenchView {
    BenchView {
        id: id.to_string(),
        root: "root".to_string(),
        name: format!("Compare {id}"),
        title: format!("work {id}\nmore of it"),
        arms: [BenchArm::default(), BenchArm::default()],
        ..BenchView::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

/// Top-level agent `root` with `kid` under it, `root` on screen, and
/// benchmarks `1` and `2` listed, `2` the newest; and what the app
/// sends.
fn app_with_benches() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let mut app = App::new().with_event_sender(tx);
    let agent = |id: &str, depth| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        depth,
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("root", 0),
        agent("kid", 1),
    ]));
    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    app.process_command(UiCommand::BenchList {
        benches: vec![view("1"), view("2")],
    });
    (app, rx)
}

/// The benchmark requests sent since the last call, as
/// `<agent> <request>`.
fn bench_requests(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::Bench { agent_id, request } => Some(format!("{agent_id} {request:?}")),
            _ => None,
        })
        .collect()
}

fn detail(id: &str) -> BenchMode {
    BenchMode::Detail(id.to_string())
}

#[test]
fn the_list_is_newest_first_and_upserts_replace_in_place() {
    let mut state = BenchState::default();
    state.list(vec![view("1"), view("2")]);
    assert_eq!(state.benches[0].id, "2");
    assert!(state.loaded());
    let mut changed = view("1");
    changed.title = "changed".to_string();
    assert!(!state.upsert(changed));
    assert_eq!(state.benches.len(), 2);
    assert_eq!(state.benches[1].title, "changed");
    assert!(state.upsert(view("3")));
    assert_eq!(state.benches[0].id, "3");
}

#[test]
fn every_tree_s_benchmarks_are_listed_whoever_is_on_screen() {
    let mut app = App::new();
    app.active_agent_id = "user".to_string();
    let mut elsewhere = view("2");
    elsewhere.root = "other".to_string();
    app.cmd_bench_list(vec![view("1"), elsewhere]);
    assert_eq!(app.bench.benches.len(), 2);
    app.bench.mode = Some(detail("2"));

    app.active_agent_id = "other".to_string();
    let mut third = view("3");
    third.root = "third".to_string();
    app.cmd_bench_upsert(third);
    assert_eq!(app.bench.benches[0].id, "3", "any tree's new benchmark");
    assert_eq!(app.bench.benches.len(), 3);

    app.cmd_bench_list(vec![view("1")]);
    assert_eq!(app.bench.mode, None, "the benchmark shown is gone");
}

#[test]
fn bench_comes_after_agents_and_puts_no_agent_on_screen() {
    let (mut app, mut rx) = app_with_benches();
    app.show_chat_tab(ChatTab::Graph);
    bench_requests(&mut rx);

    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);
    assert_eq!(bench_requests(&mut rx), ["root List"]);
    assert_eq!(app.bench.mode, Some(detail("2")), "the newest");
    assert_eq!(app.selected_bench(), 0);
    assert_eq!(
        app.app_chat_title(),
        Some(("Compare 2".to_string(), "work 2".to_string()))
    );
    assert_eq!(app.chat_tab(), ChatTab::Chat, "GRAPH is put away");
    assert!(
        app.chat_tab_layout(80).cells.is_empty(),
        "the bar has no tabs"
    );
    app.show_chat_tab(ChatTab::Log);
    assert_eq!(app.chat_tab(), ChatTab::Chat, "nor can one be shown");
    assert!(app.no_agent_on_screen());
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());

    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.chat_tab(), ChatTab::Graph, "root has its tab back");
    assert!(!app.no_agent_on_screen());
}

#[test]
fn going_between_channels_and_bench_keeps_the_agent_s_tab() {
    let (mut app, _rx) = app_with_benches();
    app.show_chat_tab(ChatTab::Graph);
    app.show_agents_tab(AgentsTab::Channels);
    app.show_agents_tab(AgentsTab::Bench);
    app.show_agents_tab(AgentsTab::Projects);
    assert_eq!(app.chat_tab(), ChatTab::Graph);
}

#[test]
fn enter_shows_a_benchmark_and_bench_comes_back_on_it() {
    let (mut app, _rx) = app_with_benches();
    app.focus = PanelFocus::Swarm;
    app.show_agents_tab(AgentsTab::Bench);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_bench(), 1);
    assert_eq!(
        app.bench.mode,
        Some(detail("2")),
        "the highlight alone shows nothing"
    );

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.bench.mode, Some(detail("1")));
    assert_eq!(app.focus(), PanelFocus::Chat, "its keys come first");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.bench.scroll, 1);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus(), PanelFocus::Swarm, "the list has them back");
    assert_eq!(app.bench.mode, Some(detail("1")), "still shown");

    app.show_agents_tab(AgentsTab::Projects);
    app.show_agents_tab(AgentsTab::Bench);
    assert_eq!(app.bench.mode, Some(detail("1")), "what BENCH was left on");
    assert_eq!(app.bench.scroll, 1, "where it was left");

    // A list without it shows the newest in its place.
    app.process_command(UiCommand::BenchList {
        benches: vec![view("2")],
    });
    assert_eq!(app.bench.mode, Some(detail("2")));
    assert_eq!(app.bench.scroll, 0);
}

#[test]
fn a_click_shows_a_benchmark_and_keeps_the_keys_on_the_list() {
    let (mut app, _rx) = app_with_benches();
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Swarm;
    app.set_bench_rows_area(Rect::new(1, 2, 30, 5));
    app.set_new_bench_area(Rect::new(1, 7, 30, 1));
    assert_eq!(app.bench_row_at(5, 3), Some(1));
    assert_eq!(app.bench_row_at(5, 4), None, "below the rows");
    assert_eq!(app.bench_row_at(5, 7), Some(2), "+ New bench");

    app.click_bench_row(5, 3);
    assert_eq!(app.bench.mode, Some(detail("1")));
    assert_eq!(app.focus(), PanelFocus::Swarm);
    app.click_bench_row(5, 7);
    assert_eq!(app.bench.mode, Some(BenchMode::Form));
    assert_eq!(app.focus(), PanelFocus::Chat, "the form takes the keys");
}

#[test]
fn the_form_opens_from_the_list_and_esc_shows_the_benchmark_before_it() {
    let (mut app, mut rx) = app_with_benches();
    app.focus = PanelFocus::Swarm;
    app.show_agents_tab(AgentsTab::Bench);
    bench_requests(&mut rx);

    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.bench.mode, Some(BenchMode::Form));
    assert_eq!(app.selected_bench(), 2, "+ New bench");
    assert_eq!(app.shown_bench_row(), Some(2));
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert_eq!(bench_requests(&mut rx), ["root Sessions"]);
    assert_eq!(
        app.app_chat_title(),
        Some(("New benchmark".to_string(), String::new()))
    );

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.bench.mode, Some(detail("2")));
    assert_eq!(app.selected_bench(), 0);
    assert_eq!(app.focus(), PanelFocus::Swarm);
}

#[test]
fn a_benchmark_the_form_starts_shows_once_the_session_reports_it() {
    let (mut app, mut rx) = app_with_benches();
    app.show_agents_tab(AgentsTab::Bench);
    app.show_bench(BenchMode::Form);
    app.focus = PanelFocus::Chat;
    app.bench.form.estimate = Some(Ok("about $1".to_string()));
    app.bench.form.field = FormField::Go;
    bench_requests(&mut rx);

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(bench_requests(&mut rx), ["root Start"]);
    // The session's answer can come before the benchmark's first
    // report.
    app.process_command(UiCommand::BenchNotice(Ok("Compare 3 started".to_string())));
    assert_eq!(app.bench.mode, Some(BenchMode::Form));
    assert!(!app.bench.form.ready(), "a fresh form");
    app.process_command(UiCommand::BenchUpsert(Box::new(view("3"))));
    assert_eq!(app.bench.mode, Some(detail("3")));
    assert_eq!(app.selected_bench(), 0);

    // Benchmarks started elsewhere are only listed.
    app.process_command(UiCommand::BenchUpsert(Box::new(view("4"))));
    assert_eq!(app.bench.mode, Some(detail("3")));
    assert_eq!(app.selected_bench(), 1, "still on it");
}

#[test]
fn bench_sends_no_message_but_fills_the_form_and_runs_commands() {
    let (mut app, mut rx) = app_with_benches();
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Chat;
    app.input.text = "hello".to_string();
    app.input.cursor_pos = 5;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.input(), "hello", "the draft stays");
    let (text, _) = app.notifications.current.clone().expect("a refusal");
    assert!(text.contains("BENCH has no chat"), "{text}");
    assert!(std::iter::from_fn(|| rx.try_recv().ok())
        .all(|event| !matches!(event, UiEvent::UserMessage { .. })));

    // From the list too, Enter puts the text on the form's row.
    app.show_bench(BenchMode::Form);
    app.bench.form.field = FormField::Prompt;
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.bench.form.prompt, "hello");
    assert_eq!(app.input(), "");

    app.bench.form.field = FormField::Work;
    app.input.text = "/agents".to_string();
    app.input.cursor_pos = 7;
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.swarm_panel_visible(), "commands run");
}

#[test]
fn an_agent_coming_on_screen_brings_projects_back_on_its_tab() {
    let (mut app, _rx) = app_with_benches();
    app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
    app.show_chat_tab(ChatTab::Log);
    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    app.show_chat_tab(ChatTab::Graph);
    app.show_agents_tab(AgentsTab::Bench);

    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    assert_eq!(app.agents_tab(), AgentsTab::Bench, "the same agent again");

    app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.chat_tab(), ChatTab::Log, "kid was left on LOG");
    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    assert_eq!(app.chat_tab(), ChatTab::Graph, "root was left on GRAPH");
}

#[test]
fn a_diff_goes_to_projects_and_a_run_s_log_shows_here() {
    let (mut app, mut rx) = app_with_benches();
    let mut done = view("2");
    done.status = BenchStatus::Done;
    done.runs = vec![BenchRun {
        name: "a-1".to_string(),
        agent_id: "kid".to_string(),
        ..BenchRun::default()
    }];
    app.process_command(UiCommand::BenchUpsert(Box::new(done)));
    app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
    app.show_chat_tab(ChatTab::Graph);
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Chat;
    bench_requests(&mut rx);

    // The diff shows in the chat of the agent it was run from.
    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "root");
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert_eq!(
        bench_requests(&mut rx),
        [r#"root Diff { id: "2", run: None }"#]
    );
    // The session confirms the switch.
    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
    assert_eq!(app.chat_tab(), ChatTab::Graph, "kid was left on GRAPH");

    // The log of the run picked, on the agent that ran it, under the
    // benchmark.
    app.process_command(UiCommand::SetActiveAgent("root".to_string()));
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Chat;
    app.handle_key(key(KeyCode::Right));
    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);
    assert_eq!(app.active_agent_id(), "kid");
    assert_eq!(app.chat_tab(), ChatTab::Log);
    assert_eq!(
        app.bench().mode,
        Some(BenchMode::Agent {
            bench: "2".to_string(),
            agent: "kid".to_string(),
        })
    );
}
