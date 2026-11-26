use super::*;

fn spawn_tree_app(cc_running: bool) -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let agent = |id: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        depth,
        ..Default::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Lead", 0),
        agent("bb", "planner", 1),
        AgentSummary {
            running: cc_running,
            ..agent("cc", "coder", 2)
        },
        agent("dd", "tester", 1),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app
}

#[test]
fn agents_with_spawns_show_a_fold_marker_and_leaves_do_not() {
    let mut app = spawn_tree_app(false);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(row_naming(&panel, "Lead").contains("▾"), "{panel:#?}");
    assert!(
        row_naming(&panel, "planner").contains("├▾  planner"),
        "{panel:#?}"
    );
    for leaf in ["coder", "tester"] {
        let row = row_naming(&panel, leaf);
        assert!(!row.contains('▾') && !row.contains('▸'), "{row:?}");
    }

    app.toggle_agent(1);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(
        row_naming(&panel, "planner").contains("├▸  planner"),
        "{panel:#?}"
    );
    assert!(!panel.iter().any(|row| row.contains("coder")), "{panel:#?}");
    assert!(row_naming(&panel, "tester").contains("└──"), "{panel:#?}");
}

/// The `AURA_WIDTH` cells after the elbow on `row`, and the cell after them.
fn ring_and_marker(row: &str) -> (String, Option<char>) {
    let at = row.find(['├', '└']).expect("elbow") + '├'.len_utf8();
    let mut cells = row[at..].chars();
    let ring = cells.by_ref().take(crate::animation::AURA_WIDTH).collect();
    (ring, cells.next())
}

#[test]
fn a_thinking_agent_with_spawns_shows_the_aura_without_a_marker() {
    let mut app = spawn_tree_app(false);
    let bb = app.agents()[1].clone();
    let mut agents = app.agents().to_vec();
    agents[1] = crate::events::AgentSummary {
        running: true,
        ..bb
    };
    app.process_command(crate::events::UiCommand::SetAgents(agents));
    let panel = agents_panel(&render_rows(&mut app, 200));
    let planner = row_naming(&panel, "planner");
    let (ring, marker) = ring_and_marker(planner);
    assert!(is_aura_ring(&ring), "{planner:?}");
    assert_eq!(marker, Some(' '), "{planner:?}");
    assert!(
        !planner.contains('▾') && !planner.contains('▸'),
        "{planner:?}"
    );
    assert!(
        planner.contains(" planner"),
        "names stay lined up: {planner:?}"
    );
}

#[test]
fn a_folded_agent_shows_the_aura_while_a_hidden_spawn_thinks() {
    let mut app = spawn_tree_app(true);
    let panel = agents_panel(&render_rows(&mut app, 200));
    assert!(
        row_naming(&panel, "planner").contains("├▾  planner"),
        "open, its spawn shows its own ring: {panel:#?}"
    );

    app.toggle_agent(1);
    let panel = agents_panel(&render_rows(&mut app, 200));
    let planner = row_naming(&panel, "planner");
    let (ring, marker) = ring_and_marker(planner);
    assert!(is_aura_ring(&ring), "{planner:?}");
    assert_ne!(marker, Some('▸'), "no arrow beside the aura: {planner:?}");
    assert!(!planner.contains('▾'), "{planner:?}");

    // Folding the top agent too carries the aura up to it.
    app.toggle_agent(0);
    let panel = agents_panel(&render_rows(&mut app, 200));
    let lead = row_naming(&panel, "Lead");
    assert!(!lead.contains('▸') && !lead.contains('▾'), "{lead:?}");
    assert!(
        lead.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)),
        "{lead:?}"
    );
}

#[test]
fn unfocused_the_folded_agent_over_the_active_one_is_lit() {
    let mut app = spawn_tree_app(false);
    app.process_command(crate::events::UiCommand::SetActiveAgent("cc".to_string()));
    app.toggle_agent(1);
    assert_ne!(app.focus(), crate::app::PanelFocus::Swarm);
    let buffer = render_buffer(&mut app, 120);
    let selection = Some(test_theme().colors.selection);
    let bg = |name: &str| {
        let (_, style) = find_cell(&buffer, row_of(&buffer, name), name).unwrap();
        style.bg
    };
    assert_eq!(bg("planner"), selection, "planner stands for coder");
    assert_ne!(bg("tester"), selection);
    assert_ne!(bg("Lead"), selection);
}
