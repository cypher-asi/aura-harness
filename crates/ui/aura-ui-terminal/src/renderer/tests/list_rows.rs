use super::*;
use crate::events::{AgentSummary, BenchArm, BenchStatus, BenchTaskRow, BenchView, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Bring the app numbered `digit` to the front of the agents panel.
fn show_app(app: &mut crate::App, digit: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(digit), KeyModifiers::ALT));
}

/// `Compare #n`, Gate against Opus on example tasks `ids`, in `status`.
fn bench(n: u32, ids: &[&str], status: BenchStatus) -> BenchView {
    let arm = |label: &str| BenchArm {
        label: label.to_string(),
        ..BenchArm::default()
    };
    BenchView {
        id: format!("b{n}"),
        name: format!("Compare #{n}"),
        status,
        tasks: ids
            .iter()
            .map(|id| BenchTaskRow {
                id: (*id).to_string(),
                ..BenchTaskRow::default()
            })
            .collect(),
        arms: [arm("gate"), arm("claude/claude-opus-5-5")],
        ..BenchView::default()
    }
}

/// `Planner` in PROJECTS, `Drafter` folded over `checker` in the Archive,
/// all last active two days ago, and BENCH in front listing `#52` running,
/// `#51` stopped and `#50` done.
fn lists_app() -> crate::App {
    let two_days_ago = aura_core_types::time::now_ms() - 2 * 24 * 60 * 60 * 1000;
    let agent = |id: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        depth,
        last_active_ms: Some(two_days_ago),
        ..AgentSummary::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![agent("aa", "Planner", 0)]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetArchivedAgents(vec![
        agent("dd", "Drafter", 0),
        agent("ee", "checker", 1),
    ]));
    let swe = ["swe_bench_pro_hard/a", "swe_bench_pro_hard/b"];
    app.process_command(UiCommand::BenchList {
        benches: vec![
            bench(50, &["coding/write-file"], BenchStatus::Done),
            bench(51, &swe, BenchStatus::Cancelled),
            bench(52, &["polyglot/go-bowling"], BenchStatus::Running),
        ],
    });
    show_app(&mut app, '4');
    app
}

#[test]
fn names_line_up_from_one_app_to_the_next() {
    let mut app = lists_app();
    let column = |panel: &[String], name: &str| char_column(row_naming(panel, name), name);
    let bench = agents_panel(&render_rows(&mut app, 200));
    let name = column(&bench, "#51 swe-pro ×2");
    assert_eq!(column(&bench, "New bench"), name, "{bench:#?}");
    show_app(&mut app, '2');
    let projects = agents_panel(&render_rows(&mut app, 200));
    assert_eq!(column(&projects, "Planner"), name, "{projects:#?}");
    show_app(&mut app, '5');
    let archive = agents_panel(&render_rows(&mut app, 200));
    assert_eq!(column(&archive, "Drafter"), name, "{archive:#?}");
}

#[test]
fn a_running_benchmark_spins_in_its_icon_column_and_none_marks_its_status() {
    let mut app = lists_app();
    let panel = agents_panel(&render_rows(&mut app, 200));
    let running = row_naming(&panel, "#52 polyglot ×1");
    let name = char_column(running, "#52");
    let icon: String = running.chars().skip(name - 3).take(3).collect();
    assert!(is_aura_ring(&icon), "{running:?}");
    for label in ["#52 polyglot", "#51 swe-pro", "#50 coding"] {
        let row = row_naming(&panel, label);
        assert!(!row.contains('✓') && !row.contains('■'), "{row:?}");
    }
}

#[test]
fn a_stopped_benchmark_is_muted() {
    let mut app = lists_app();
    let buffer = render_buffer(&mut app, 200);
    let color = |label: &str| find_cell(&buffer, row_of(&buffer, label), label).map(|(_, s)| s.fg);
    let colors = test_theme().colors;
    assert_eq!(color("#51 swe-pro"), Some(Some(colors.muted)), "stopped");
    assert_eq!(color("#50 coding"), Some(Some(colors.foreground)), "done");
}

#[test]
fn an_archived_agent_says_how_long_ago_under_a_day_in_its_icon_column() {
    let mut app = lists_app();
    show_app(&mut app, '5');
    let rows = render_rows(&mut app, 200);
    let panel = agents_panel(&rows);
    let agent = row_naming(&panel, "Drafter");
    let day = row_naming(&panel, "EARLIER");
    assert_eq!(
        char_column(day, "EARLIER"),
        char_column(agent, "▸"),
        "{panel:#?}"
    );
    let at = line_with(&panel, "Drafter");
    let inside = rows[at].split('│').nth(1).unwrap_or_default();
    assert!(inside.trim_end().ends_with("2d"), "{inside:?}");
}

#[test]
fn a_day_header_is_drawn_like_a_project_header() {
    let mut app = lists_app();
    let mut header = |digit: char, label: &str| {
        show_app(&mut app, digit);
        let row = line_with(&agents_panel(&render_rows(&mut app, 200)), label);
        let buffer = render_buffer(&mut app, 200);
        find_cell(&buffer, u16::try_from(row).unwrap(), label).map(|(_, style)| style)
    };
    let project = header('2', "PLANNER");
    assert!(project.is_some());
    assert_eq!(header('5', "EARLIER"), project);
}
