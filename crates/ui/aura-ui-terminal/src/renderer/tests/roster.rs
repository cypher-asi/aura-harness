use super::*;

// ========================================================================
// Layout
// ========================================================================

#[test]
fn test_default_layout_shows_agents_column() {
    use crate::events::{AgentDefinitionSummary, AgentSummary, UiCommand};

    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            zns_id: "0://terminal/aa".to_string(),
            is_active: true,
            record_count: 12,
            last_active: "-".to_string(),
            last_active_ms: Some(now_ms - 3 * 60 * 1000),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            zns_id: "0://terminal/bb".to_string(),
            is_active: false,
            record_count: 0,
            last_active: "-".to_string(),
            running: true,
            ..Default::default()
        },
        AgentSummary {
            id: "cc".to_string(),
            name: "reviewer #1".to_string(),
            zns_id: "0://terminal/cc".to_string(),
            definition: Some("reviewer".to_string()),
            depth: 1,
            ..Default::default()
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
            ..Default::default()
        },
    ]));

    let rows = render_rows(&mut app, 200);
    let screen = rows.join("\n");
    assert!(!screen.contains("┌ AGENTS"));
    assert!(screen.contains("CHAT"));
    assert!(screen.contains("KERNEL"));
    assert!(rows
        .iter()
        .any(|r| r.contains("PROJECTS") && r.contains("AGENTS")));
    assert!(!screen.contains("←→"));
    let char_col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let rows_with = |needle: &str| -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| r.contains(needle))
            .map(|(i, _)| i)
            .collect()
    };
    // A project row shows its first agent's name in capitals.
    let [terminal_project] = rows_with("TERMINAL AGENT")[..] else {
        panic!("expected one project row for Terminal Agent");
    };
    let [terminal_agent] = rows_with("Terminal Agent")[..] else {
        panic!("expected one agent row for Terminal Agent");
    };
    let [researcher_project] = rows_with("RESEARCHER")[..] else {
        panic!("expected one project row for Researcher");
    };
    let [researcher_agent] = rows_with("Researcher")[..] else {
        panic!("expected one agent row for Researcher");
    };
    let new_project = rows_with("New project")[0];
    assert!(rows[terminal_project].contains("🗁  TERMINAL AGENT"));
    // An idle agent leaves the icon column blank, even when it is active.
    let agent_panel: String = rows[terminal_agent].chars().take(30).collect();
    assert!(
        !agent_panel.contains('●') && !agent_panel.contains('·'),
        "{agent_panel:?}"
    );
    assert_eq!(
        char_col(&rows[terminal_agent], "Terminal Agent"),
        char_col(&rows[terminal_project], "TERMINAL AGENT")
    );
    assert!(rows[terminal_agent].contains(" 3m "));
    assert!(rows[researcher_project].contains("🗁  RESEARCHER"));
    assert!(!screen.contains("12│"));
    let above_footer = rows[..rows.len() - 1].join("\n");
    assert!(!above_footer.contains("# ") && !above_footer.contains("@ "));
    assert!(!screen.contains('↳') && !screen.contains('▸'));
    // Only Researcher has a spawn, and it is thinking: its ring stands in
    // for the open marker.
    assert!(rows_with("▾").is_empty());
    assert!(!rows[researcher_agent].contains('▾'));

    // A thinking agent shows the chat's braille spinner in the icon column.
    let ring_col = char_col(&rows[terminal_project], "🗁");
    let ring: String = rows[researcher_agent]
        .chars()
        .skip(ring_col)
        .take(crate::animation::AURA_WIDTH)
        .collect();
    assert!(
        is_aura_ring(&ring),
        "expected the braille spinner, got {ring:?}"
    );

    // Icons and names line up down the list, with a blank row between projects.
    assert_eq!(
        char_col(&rows[terminal_project], "🗁"),
        char_col(&rows[new_project], "+")
    );
    assert_eq!(ring_col, char_col(&rows[terminal_project], "🗁"));
    assert_eq!(
        char_col(&rows[new_project], "New project"),
        char_col(&rows[researcher_project], "RESEARCHER")
    );
    // Projects and agents are one line each, with one blank line between projects.
    assert_eq!(terminal_agent, terminal_project + 1);
    assert_eq!(researcher_project, terminal_agent + 2);
    let panel_text = |row: usize| rows[row].chars().skip(2).take(20).collect::<String>();
    assert_eq!(panel_text(terminal_agent + 1).trim(), "");
    assert_eq!(researcher_agent, researcher_project + 1);
    // "+ New project" takes the panel's bottom row in place of a key hint.
    assert!(!screen.contains("space fold") && !screen.contains("^N new"));
    assert!(
        rows[new_project + 1].starts_with(" └"),
        "New project should sit on the panel's last row: {:?}",
        &rows[new_project..=new_project + 1]
    );
    assert!(new_project > rows_with("reviewer #1")[0] + 1);
    // Spawned agents sit further in than the project's first agent.
    assert!(
        char_col(&rows[rows_with("reviewer #1")[0]], "reviewer #1")
            > char_col(&rows[researcher_agent], "Researcher")
    );
    // The spawn's tree line drops from under its parent's name and runs
    // through the empty icon column into the spawn's name.
    let reviewer = &rows[rows_with("reviewer #1")[0]];
    let elbow = char_col(reviewer, "└──");
    assert_eq!(elbow, char_col(&rows[researcher_agent], "Researcher"));
    assert_eq!(char_col(reviewer, "reviewer #1"), elbow + 4, "{reviewer:?}");
    assert!(!screen.contains("Reviews"));

    // Tab to the agents panel. Neither the selected row nor any other
    // shows a cursor arrow.
    for _ in 0..3 {
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Tab,
        ));
    }
    let rows = render_rows(&mut app, 200);
    let panel: Vec<String> = rows.iter().map(|r| r.chars().take(30).collect()).collect();
    assert!(
        panel.iter().all(|r| !r.contains('>')),
        "projects list should not show a cursor arrow: {panel:#?}"
    );
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Right,
    ));
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(screen.contains("@  reviewer"));
    assert!(!screen.contains("Reviews"));
    assert!(screen.contains("+  New agent"));
    let column_of = |needle: &str| {
        rows.iter()
            .find_map(|r| r.find(needle).map(|i| r[..i].chars().count()))
            .unwrap()
    };
    assert_eq!(column_of("@"), column_of("+"));
    assert_eq!(column_of("reviewer"), column_of("New agent"));
    // The roster `@` is gray; the agent name stays white. Selection paints
    // both in the selection text color, so step off the row first.
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Down,
    ));
    let buffer = render_buffer(&mut app, 120);
    let theme = test_theme();
    let reviewer_row = row_of(&buffer, "reviewer");
    let (_, at) = find_cell(&buffer, reviewer_row, "@").unwrap();
    let (_, name) = find_cell(&buffer, reviewer_row, "reviewer").unwrap();
    assert_eq!(at.fg, Some(theme.colors.muted));
    assert_eq!(name.fg, Some(theme.colors.secondary));
    app.handle_key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Up,
    ));
    assert!(!screen.contains("spawn") && !screen.contains(" del"));
    assert!(!screen.contains("Researcher"));
    // "+ New agent" takes the panel's bottom row, like "+ New project".
    let new_agent = rows.iter().position(|r| r.contains("New agent")).unwrap();
    assert!(
        rows[new_agent + 1].starts_with(" └"),
        "New agent should sit on the panel's last row: {:?}",
        &rows[new_agent..=new_agent + 1]
    );
    assert!(new_agent > rows.iter().position(|r| r.contains("@  reviewer")).unwrap() + 1);
}

/// An app showing the roster of `definitions`, each a name and its
/// abbreviation.
fn roster_app(definitions: &[(&str, Option<&str>)]) -> crate::App {
    use crate::app::{AgentsTab, PaneTab};
    use crate::events::{AgentDefinitionSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgentDefinitions(
        definitions
            .iter()
            .map(|&(name, abbreviation)| AgentDefinitionSummary {
                name: name.to_string(),
                description: format!("What {name} does"),
                abbreviation: abbreviation.map(str::to_string),
            })
            .collect(),
    ));
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Roster));
    app
}

/// The text of each of `lines`.
fn line_texts(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn roster_rows_end_in_their_abbreviation_lined_up_at_the_far_right() {
    let mut app = roster_app(&[
        ("rust-engineer", Some("REN")),
        ("reviewer", Some("REV")),
        ("explorer", Some("EXP")),
    ]);
    let rows = render_rows(&mut app, 120);
    // The panel's text from just after the name to its right border.
    let end_of = |name: &str| -> String {
        let row = &rows[line_with(&rows, &format!("@  {name}"))];
        let after = &row[row.find(name).unwrap() + name.len()..];
        after[..after.find('│').expect("border")].to_string()
    };
    for (name, abbreviation) in [
        ("rust-engineer", "REN"),
        ("reviewer", "REV"),
        ("explorer", "EXP"),
    ] {
        let end = end_of(name);
        assert!(
            end.ends_with(&format!(" {abbreviation} ")),
            "{name}: {end:?}"
        );
        assert!(
            end.trim_start().starts_with(abbreviation.trim_end()),
            "{end:?}"
        );
    }
    let column = |name: &str| {
        let row = &rows[line_with(&rows, &format!("@  {name}"))];
        char_column(row, name) + name.len() + end_of(name).len() - 4
    };
    assert_eq!(column("rust-engineer"), column("reviewer"));
    assert_eq!(column("reviewer"), column("explorer"));

    // "+ New agent" has none.
    let new_agent = &rows[line_with(&rows, "+  New agent")];
    let after = &new_agent[new_agent.find("New agent").unwrap() + "New agent".len()..];
    let after = &after[..after.find('│').expect("border")];
    assert!(after.chars().all(|c| c == ' '), "{new_agent:?}");

    // The abbreviation is in the time color on the row's fill, on the
    // selected first row as on the others.
    let theme = test_theme();
    let buffer = render_buffer(&mut app, 120);
    for (name, abbreviation) in [("rust-engineer", "REN"), ("explorer", "EXP")] {
        let line = row_of(&buffer, name);
        let (_, cell) = find_cell(&buffer, line, &format!("{abbreviation} ")).unwrap();
        let (_, at) = find_cell(&buffer, line, "@").unwrap();
        assert_eq!(cell.bg, at.bg, "{name}");
        let time = if name == "rust-engineer" {
            theme.colors.on_selection
        } else {
            theme.colors.muted
        };
        assert_eq!(cell.fg, Some(time), "{name}");
    }
}

#[test]
fn a_narrow_roster_cuts_the_name_then_drops_the_abbreviation() {
    use crate::animation::AURA_WIDTH;
    let app = roster_app(&[("rust-engineer", Some("REN")), ("reviewer", Some("REV"))]);
    let theme = test_theme();
    // ` `, the icon column and its pad, then ` REV `.
    let around = 1 + AURA_WIDTH + 1 + 5;
    for width in around..=around + 20 {
        let inner = Rect::new(0, 0, u16::try_from(width).unwrap(), 10);
        let lines = super::super::agents::roster::build_definition_lines(&app, inner, &theme);
        let texts = line_texts(&lines);
        for (line, text) in lines.iter().zip(&texts) {
            assert_eq!(line.width(), width, "width {width}: {text:?}");
        }
        let [engineer, reviewer, new_agent] = &texts[..] else {
            panic!("{texts:?}");
        };
        if width >= around + 4 {
            assert!(engineer.ends_with(" REN "), "{engineer:?}");
            assert!(reviewer.ends_with(" REV "), "{reviewer:?}");
            assert!(
                engineer.contains("rust-engineer") || engineer.contains('…'),
                "the name is cut first: {engineer:?}"
            );
        } else {
            assert!(!engineer.contains("RE"), "{engineer:?}");
            assert!(!reviewer.contains("REV"), "{reviewer:?}");
        }
        let lead = " ".repeat(AURA_WIDTH);
        assert!(new_agent.starts_with(&format!(" +{lead}")), "{new_agent:?}");
        assert!(!new_agent.contains("RE"), "{new_agent:?}");
    }

    // Roster rows without an abbreviation keep the plain layout.
    let plain = roster_app(&[("reviewer", None)]);
    let inner = Rect::new(0, 0, 24, 10);
    let texts = line_texts(&super::super::agents::roster::build_definition_lines(
        &plain, inner, &theme,
    ));
    let lead = " ".repeat(AURA_WIDTH);
    let name_width = 24 - 2 - AURA_WIDTH;
    assert_eq!(texts[0], format!(" @{lead}{:<name_width$}", "reviewer"));
    assert_eq!(texts[1], format!(" +{lead}{:<name_width$}", "New agent"));
}

#[test]
fn test_format_ago_steps_up_at_each_unit() {
    use crate::components::format_ago;
    const MIN: u64 = 60_000;
    let now = 100 * 7 * 24 * 60 * MIN;
    let ago = |ms: u64| format_ago(now - ms, now);
    assert_eq!(ago(0), "now");
    assert_eq!(ago(MIN - 1), "now");
    assert_eq!(ago(MIN), "1m");
    assert_eq!(ago(60 * MIN - 1), "59m");
    assert_eq!(ago(60 * MIN), "1h");
    assert_eq!(ago(24 * 60 * MIN), "1d");
    assert_eq!(ago(7 * 24 * 60 * MIN - 1), "6d");
    assert_eq!(ago(7 * 24 * 60 * MIN), "1w");
    assert_eq!(format_ago(now + MIN, now), "now");
}
