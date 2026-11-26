
use super::*;
use crate::app::{AgentsTab, ChatTab};
use crate::events::{BenchArm, BenchRun, BenchView, UiCommand, UiEvent};
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

fn agent(id: &str, depth: usize, bench_run: bool) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        depth,
        bench_run,
        ..AgentSummary::default()
    }
}

fn view(id: &str) -> BenchView {
    BenchView {
        id: id.to_string(),
        root: "lead".to_string(),
        name: format!("Compare {id}"),
        title: format!("work {id}"),
        arms: [BenchArm::default(), BenchArm::default()],
        ..BenchView::default()
    }
}

/// `lead`, under it `c1`'s agent, under that `arm-a` over runs `a-1`,
/// which spawned `scout`, and `a-2`, and `arm-b` over `b-1`; then
/// `other` in a project of its own.
fn agents() -> Vec<AgentSummary> {
    vec![
        agent("lead", 0, false),
        agent("c1", 1, false),
        agent("arm-a", 2, false),
        agent("a-1", 3, true),
        agent("scout", 4, true),
        agent("a-2", 3, true),
        agent("arm-b", 2, false),
        agent("b-1", 3, true),
        agent("other", 0, false),
    ]
}

/// [`agents`], `lead` on screen in PROJECTS, and benchmarks `c1` and
/// `c2` listed, `c2` the newest, with no agents; and what the app
/// sends.
fn app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(agents()));
    app.process_command(UiCommand::SetActiveAgent("lead".to_string()));
    app.process_command(UiCommand::BenchList {
        benches: vec![view("c1"), view("c2")],
    });
    (app, rx)
}

/// [`app`] in BENCH with the keys on the list, `c1` unfolded and
/// highlighted.
fn unfolded() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app();
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char(' ')));
    (app, rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

/// The list's rows by id: a benchmark's, an agent's (an arm's in
/// brackets), and `+` for "+ New bench".
fn ids(app: &App) -> Vec<String> {
    app.bench_rows()
        .into_iter()
        .map(|row| match row {
            BenchRow::Bench(index) => app.bench.benches[index].id.clone(),
            BenchRow::Agent(agent) if agent.arm => format!("[{}]", app.bench_agent(agent).id),
            BenchRow::Agent(agent) => app.bench_agent(agent).id.clone(),
            BenchRow::New => "+".to_string(),
        })
        .collect()
}

/// The id of the highlighted row, as [`ids`] writes it.
fn highlighted(app: &App) -> String {
    ids(app).swap_remove(app.selected_bench())
}

fn agent_mode(agent: &str) -> BenchMode {
    BenchMode::Agent {
        bench: "c1".to_string(),
        agent: agent.to_string(),
    }
}

#[test]
fn a_benchmark_unfolds_over_its_arms_runs_and_the_agents_they_spawned() {
    let (mut app, _rx) = app();
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Swarm;
    assert_eq!(ids(&app), ["c2", "c1", "+"], "folded at first");
    assert!(!app.bench_has_agents(0), "c2 has none listed");
    assert!(app.bench_has_agents(1));

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(app.bench_unfolded(1));
    assert_eq!(
        ids(&app),
        ["c2", "c1", "[arm-a]", "a-1", "scout", "a-2", "[arm-b]", "b-1", "+"]
    );
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(ids(&app), ["c2", "c1", "+"]);

    // Archived, they unfold from the Archive, where `c1` is at the top.
    let mut listed = agents();
    let archived: Vec<_> = listed
        .drain(1..8)
        .map(|agent| AgentSummary {
            depth: agent.depth - 1,
            ..agent
        })
        .collect();
    app.process_command(UiCommand::SetAgents(listed));
    app.process_command(UiCommand::SetArchivedAgents(archived));
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(
        ids(&app),
        ["c2", "c1", "[arm-a]", "a-1", "scout", "a-2", "[arm-b]", "b-1", "+"]
    );
    assert!(app.bench_rows().iter().any(|row| matches!(
        row,
        BenchRow::Agent(BenchAgent {
            archived: true,
            top: 0,
            ..
        })
    )));
}

#[test]
fn the_highlight_passes_over_the_arms_and_stays_on_its_row() {
    let (mut app, _rx) = unfolded();
    assert_eq!(highlighted(&app), "c1");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(highlighted(&app), "a-1", "past arm-a");
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(highlighted(&app), "b-1", "past arm-b");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(highlighted(&app), "a-2");

    // A run starting above it does not move it.
    let mut more = agents();
    more.insert(5, agent("a-3", 3, true));
    app.process_command(UiCommand::SetAgents(more));
    assert_eq!(highlighted(&app), "a-2");

    app.handle_key(key(KeyCode::End));
    assert_eq!(highlighted(&app), "+");
    app.handle_key(key(KeyCode::Home));
    assert_eq!(highlighted(&app), "c2");
}

#[test]
fn space_on_an_agent_folds_its_benchmark_onto_it() {
    let (mut app, _rx) = unfolded();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(highlighted(&app), "scout");
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(ids(&app), ["c2", "c1", "+"]);
    assert_eq!(highlighted(&app), "c1");
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(highlighted(&app), "c1", "unfolding leaves it there");
}

#[test]
fn an_agent_shows_in_bench_with_its_tabs_and_takes_the_keys() {
    let (mut app, mut rx) = unfolded();
    while rx.try_recv().is_ok() {}
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);
    assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
    assert_eq!(app.active_agent_id(), "a-1", "on screen at once");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "a-1"));
    assert_eq!(app.focus(), PanelFocus::Chat, "typing goes to it");
    assert!(!app.no_agent_on_screen());
    assert_eq!(app.app_chat_title(), None, "the agent's own header");
    assert!(!app.chat_tab_layout(80).cells.is_empty(), "with its tabs");
    app.show_chat_tab(ChatTab::Log);
    assert_eq!(app.chat_tab(), ChatTab::Log);
    assert_eq!(app.shown_bench_row(), Some(3), "its row is lit");

    // The session confirming it keeps BENCH in front.
    app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);

    // Esc takes LOG back to CHAT, then hands the keys to the list.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert_eq!(app.focus(), PanelFocus::Chat);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus(), PanelFocus::Swarm);

    // Any other agent coming on screen brings PROJECTS back.
    app.process_command(UiCommand::SetActiveAgent("other".to_string()));
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
}

#[test]
fn a_benchmark_and_its_agents_take_turns_with_the_chat_tabs() {
    let (mut app, _rx) = app();
    app.show_chat_tab(ChatTab::Graph);
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char(' ')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.chat_tab(), ChatTab::Chat, "a-1 starts on CHAT");
    app.show_chat_tab(ChatTab::Log);

    // A benchmark puts the agent's tabs away, and the agent coming
    // back has them back.
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    assert!(app.chat_tab_layout(80).cells.is_empty());
    assert!(app.no_agent_on_screen());
    app.focus = PanelFocus::Swarm;
    for _ in 0..2 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
    assert_eq!(app.chat_tab(), ChatTab::Log);

    // Leaving and coming back: each app shows what it was left on,
    // on its tab.
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
    assert_eq!(app.active_agent_id(), "lead");
    assert_eq!(app.chat_tab(), ChatTab::Graph);
    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);
    assert_eq!(app.active_agent_id(), "a-1");
    assert_eq!(app.chat_tab(), ChatTab::Log);

    // CHANNELS has only CHAT; the agent gets LOG back after it.
    app.show_agents_tab(AgentsTab::Channels);
    assert_eq!(app.chat_tab(), ChatTab::Chat);
    app.show_agents_tab(AgentsTab::Bench);
    assert_eq!(app.chat_tab(), ChatTab::Log);
}

#[test]
fn the_form_opened_over_an_agent_goes_back_to_it() {
    let (mut app, _rx) = unfolded();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    app.show_chat_tab(ChatTab::Log);
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.bench.mode, Some(BenchMode::Form));
    assert_eq!(app.chat_tab(), ChatTab::Chat);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
    assert_eq!(app.chat_tab(), ChatTab::Log);
}

#[test]
fn coming_back_to_bench_shows_a_benchmark_for_an_agent_gone() {
    let (mut app, _rx) = unfolded();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    app.show_agents_tab(AgentsTab::Projects);
    let mut gone = agents();
    gone.retain(|agent| agent.id != "a-1");
    app.process_command(UiCommand::SetAgents(gone));

    app.show_agents_tab(AgentsTab::Bench);
    assert_eq!(app.bench.mode, Some(BenchMode::Detail("c1".to_string())));
    assert_eq!(app.active_agent_id(), "lead");
    assert!(app.chat_tab_layout(80).cells.is_empty());
}

#[test]
fn a_click_folds_on_the_arrow_and_shows_elsewhere() {
    let (mut app, _rx) = app();
    app.show_agents_tab(AgentsTab::Bench);
    app.focus = PanelFocus::Swarm;
    app.set_bench_rows_area(Rect::new(1, 2, 40, 12));
    app.set_new_bench_area(Rect::new(1, 14, 40, 1));
    // c1 is on screen row 3: its arrow unfolds it, and shows nothing.
    app.click_bench_row(2, 3);
    assert!(app.bench_unfolded(1));
    assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
    assert_eq!(app.bench_row_at(10, 4), None, "arm-a takes no click");

    app.click_bench_row(10, 3);
    assert_eq!(app.bench.mode, Some(BenchMode::Detail("c1".to_string())));
    assert_eq!(
        app.focus(),
        PanelFocus::Swarm,
        "a benchmark leaves the keys"
    );
    app.click_bench_row(10, 5);
    assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
    assert_eq!(app.focus(), PanelFocus::Chat, "an agent takes them");

    // c2 has no agents: its arrow's columns show it.
    app.click_bench_row(2, 2);
    assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
}

#[test]
fn l_shows_a_run_s_log_under_its_benchmark() {
    let (mut app, _rx) = app();
    let mut c1 = view("c1");
    c1.status = crate::events::BenchStatus::Done;
    c1.runs = vec![BenchRun {
        name: "a-2".to_string(),
        agent_id: "a-2".to_string(),
        ..BenchRun::default()
    }];
    app.process_command(UiCommand::BenchUpsert(Box::new(c1)));
    app.show_agents_tab(AgentsTab::Bench);
    app.show_bench(BenchMode::Detail("c1".to_string()));
    app.focus = PanelFocus::Chat;
    app.handle_key(key(KeyCode::Right));
    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
    assert_eq!(app.agents_tab(), AgentsTab::Bench);
    assert_eq!(app.bench.mode, Some(agent_mode("a-2")));
    assert_eq!(app.active_agent_id(), "a-2");
    assert_eq!(app.chat_tab(), ChatTab::Log);
    assert!(app.bench_unfolded(1), "its row shows");
    assert_eq!(highlighted(&app), "a-2");
}

/// The screen at `width` columns, a line per row.
fn screen(app: &mut App, width: u16) -> Vec<String> {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    let theme = crate::Theme::cyber();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &theme))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

#[test]
fn an_unfolded_benchmark_hangs_its_agents_from_its_name() {
    let (mut app, _rx) = unfolded();
    let rows = screen(&mut app, 140);
    let at = |text: &str| {
        rows.iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no {text}:\n{}", rows.join("\n")))
    };
    let before = |row: usize, text: &str| &rows[row][..rows[row].find(text).unwrap()];
    let bench = at("Compare c1");
    assert!(before(bench, "Compare c1").contains('▾'), "{}", rows[bench]);
    // The list's c2, above it: no arrow without agents.
    let lead = before(bench - 1, "Compare c2");
    assert!(!lead.contains('▸') && !lead.contains('▾'), "{lead}");
    for (row, guides) in [
        (bench + 1, "├── arm-a"),
        (bench + 2, "│   ├── a-1"),
        (bench + 3, "│   │   └── scout"),
        (bench + 4, "│   └── a-2"),
        (bench + 5, "└── arm-b"),
        (bench + 6, "    └── b-1"),
    ] {
        assert!(rows[row].contains(guides), "{guides}:\n{}", rows.join("\n"));
    }
    assert_eq!(
        before(bench + 1, "├── arm-a").chars().count(),
        before(bench, "Compare c1").chars().count(),
        "the lines drop from the benchmark's name"
    );
}
