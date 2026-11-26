use crate::events::{
    BenchArm, BenchLiveRun, BenchRun, BenchStatus, BenchTaskRow, BenchTaskSetup, BenchView,
    UiCommand,
};
use crate::{App, Theme};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn bench(id: &str, name: &str) -> BenchView {
    BenchView {
        id: id.to_string(),
        root: "root".to_string(),
        name: name.to_string(),
        title: "fix the parser".to_string(),
        source: "2 example tasks".to_string(),
        status: BenchStatus::Running,
        repeats: 1,
        tasks: vec![
            BenchTaskRow {
                id: "swe/ansible-1".to_string(),
                ..BenchTaskRow::default()
            },
            BenchTaskRow {
                id: "swe/flipt-2".to_string(),
                setup: BenchTaskSetup::Fetching { pull: true },
                ..BenchTaskRow::default()
            },
        ],
        arms: [
            BenchArm {
                label: "gate".to_string(),
                total_cost_usd: 0.40,
                ..BenchArm::default()
            },
            BenchArm {
                label: "claude/claude-opus-5-5".to_string(),
                ..BenchArm::default()
            },
        ],
        runs: vec![BenchRun {
            name: "a-1".to_string(),
            check_passed: Some(true),
            ..BenchRun::default()
        }],
        live: vec![BenchLiveRun {
            name: "b-1".to_string(),
            arm: 1,
            wall_ms: Some(30_000),
            cost_usd: 0.25,
            ..BenchLiveRun::default()
        }],
        ..BenchView::default()
    }
}

fn rows(app: &mut App) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn find<'a>(rows: &'a [String], needle: &str) -> Option<(usize, &'a String)> {
    rows.iter()
        .enumerate()
        .find(|(_, row)| row.contains(needle))
}

fn click(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });
}

#[test]
fn no_panel_without_a_running_benchmark() {
    let mut app = App::new();
    assert!(find(&rows(&mut app), "Spent").is_none());
    let mut done = bench("1", "Compare #1");
    done.status = BenchStatus::Done;
    app.process_command(UiCommand::BenchUpsert(Box::new(done)));
    assert!(find(&rows(&mut app), "COMPARE #1").is_none());
}

#[test]
fn a_running_benchmark_shows_its_progress_above_the_input_from_any_tab() {
    let mut app = App::new();
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
    let rows = rows(&mut app);

    let (header, tabs) = find(&rows, "COMPARE #1 ×").expect("the tab");
    assert!(tabs.contains('—'), "{tabs:?}");
    let (work, line) = find(&rows, "├ Work").expect("work");
    assert!(line.contains("2 example tasks · swe"), "{line:?}");
    assert!(find(&rows, "A gate  vs  B opus-5-5").is_some());
    assert!(find(&rows, "├ Setup   1/2 tasks ready · 1 fetching").is_some());
    assert!(find(&rows, "└ Spent   A $0.40 · B $0.25").is_some());
    let (status, line) = find(&rows, "Running · 1 working · 1 of 4 done").expect("status");
    assert!(!line.contains("click to"), "no keys: {line:?}");
    let (bar, line) = find(&rows, "25%").expect("bar");
    assert!(line.contains('█') && line.contains('▒') && line.contains('░'));
    let (input, _) = find(&rows, "→ ").expect("the input");
    assert!(header < work && work < status && status < bar && bar < input);
    assert!(rows[header - 1].contains('├'), "{:?}", rows[header - 1]);
}

#[test]
fn a_failed_check_is_an_accent_block_and_nothing_is_blue() {
    let mut app = App::new();
    let mut failed = bench("1", "Compare #1");
    failed.runs[0].check_passed = Some(false);
    app.process_command(UiCommand::BenchUpsert(Box::new(failed)));
    let theme = Theme::cyber();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &theme))
        .unwrap();
    let panel = app.bench_panel_areas().panel.expect("panel");
    let buffer = terminal.backend().buffer();
    let cells: Vec<_> = (panel.top()..panel.bottom())
        .flat_map(|y| (panel.left()..panel.right()).map(move |x| &buffer[(x, y)]))
        .collect();
    let block = cells
        .iter()
        .find(|cell| cell.symbol() == "▓")
        .expect("the failed check");
    assert_eq!(block.fg, theme.colors.success);
    assert!(cells.iter().all(|cell| cell.fg != theme.colors.warning));
}

#[test]
fn a_finished_benchmark_keeps_a_full_bar_until_its_tab_is_closed() {
    let mut app = App::new();
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
    let mut done = bench("1", "Compare #1");
    done.status = BenchStatus::Done;
    done.tasks[1].setup = BenchTaskSetup::Ready;
    done.live.clear();
    done.runs = vec![BenchRun::default(); 4];
    app.process_command(UiCommand::BenchUpsert(Box::new(done)));
    let shown = rows(&mut app);
    let (_, status) = find(&shown, "Done · 4 of 4 done").expect("done");
    assert!(!status.contains("working"), "{status:?}");
    let (_, bar) = find(&shown, "100%").expect("bar");
    assert!(!bar.contains('░'), "{bar:?}");

    let close = app.bench_panel_areas().tabs[0].close.expect("the ×");
    click(&mut app, close.x + 1, close.y);
    assert!(find(&rows(&mut app), "COMPARE #1").is_none());
}

#[test]
fn tabs_pick_and_the_toggle_folds_to_the_tabs_and_the_bar() {
    let mut app = App::new();
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("2", "Compare #2"))));
    let shown = rows(&mut app);
    assert!(
        find(&shown, "COMPARE #1   COMPARE #2 ×").is_some(),
        "the newest is picked"
    );

    let first = app.bench_panel_areas().tabs[0].tab;
    click(&mut app, first.x, first.y);
    assert!(find(&rows(&mut app), "COMPARE #1 ×   COMPARE #2").is_some());

    let toggle = app.bench_panel_areas().toggle.expect("toggle");
    click(&mut app, toggle.x + 1, toggle.y);
    let shown = rows(&mut app);
    let (header, tabs) = find(&shown, "COMPARE #1 ×").expect("tabs");
    assert!(tabs.contains(" + "), "{tabs:?}");
    assert!(find(&shown, "├ Work").is_none());
    assert!(shown[header + 1].contains("25%"), "{:?}", shown[header + 1]);
}

#[test]
fn a_click_on_the_body_opens_the_benchmark_in_bench() {
    let mut app = App::new();
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
    let shown = rows(&mut app);
    let (row, _) = find(&shown, "├ Work").expect("work");
    let panel = app.bench_panel_areas().panel.expect("panel");
    click(&mut app, panel.x + 4, u16::try_from(row).unwrap());
    assert_eq!(app.agents_tab(), crate::app::AgentsTab::Bench);
    assert_eq!(
        app.bench().shown().map(|b| b.name.as_str()),
        Some("Compare #1")
    );
}

#[test]
fn only_the_benchmarks_of_the_tree_on_screen_have_tabs() {
    use crate::events::AgentSummary;
    let mut app = App::new();
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
    app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
    app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
    let mut other = bench("2", "Compare #2");
    other.root = "elsewhere".to_string();
    app.process_command(UiCommand::BenchUpsert(Box::new(other)));
    let shown = rows(&mut app);
    assert!(find(&shown, "COMPARE #1 ×").is_some());
    assert!(find(&shown, "COMPARE #2").is_none());
}
