use super::*;

// ========================================================================
// Folding agents over their spawns
// ========================================================================

/// Project `aa` with spawns `bb` (which spawned `cc`) and `dd`, then
/// project `ee`. Rows from line 4: header 4, aa 5, bb 6, cc 7, dd 8.
fn app_with_spawn_tree() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_agent_areas();
    let spawned = |id: &str, depth: usize| crate::events::AgentSummary {
        depth,
        ..agent(id, &id.to_uppercase())
    };
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("bb", 1),
        spawned("cc", 2),
        spawned("dd", 1),
        agent("ee", "EE"),
    ]));
    (app, rx)
}

/// Indices of the agents with a row.
fn shown_agents(app: &App) -> Vec<usize> {
    app.project_rows()
        .iter()
        .filter_map(|row| match row {
            ProjectListRow::Agent { index } => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn folding_an_agent_hides_what_it_spawned_and_nested_folds_survive() {
    let (mut app, _rx) = app_with_spawn_tree();
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    assert!(app.agent_has_spawns(0) && app.agent_has_spawns(1));
    assert!(!app.agent_has_spawns(2) && !app.agent_has_spawns(3) && !app.agent_has_spawns(4));

    app.toggle_agent(1);
    assert!(app.agent_folded(1));
    assert_eq!(shown_agents(&app), [0, 1, 3, 4]);
    app.toggle_agent(0);
    assert_eq!(shown_agents(&app), [0, 4]);
    app.toggle_agent(0);
    assert_eq!(shown_agents(&app), [0, 1, 3, 4], "bb stays folded inside");
    app.toggle_agent(1);
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);

    // A leaf has nothing to fold.
    app.toggle_agent(2);
    assert!(!app.agent_folded(2));
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
}

#[test]
fn folding_an_agent_parks_a_hidden_highlight_on_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    let cc = app.row_of_agent(2).expect("cc shown");
    app.selected_agent = cc;
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 },
        "opening keeps the same row"
    );

    // A highlight below the fold follows its row up and back down.
    app.selected_agent = app.row_of_agent(3).expect("dd shown");
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 3 }
    );
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 3 }
    );
}

#[test]
fn space_folds_an_agent_with_spawns_and_a_leaf_folds_its_project() {
    let (mut app, mut rx) = app_with_spawn_tree();
    app.focus = PanelFocus::Swarm;
    app.selected_agent = app.row_of_agent(1).expect("bb shown");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(app.agent_folded(1));
    assert_eq!(shown_agents(&app), [0, 1, 3, 4], "the project stays open");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(!app.agent_folded(1));

    app.selected_agent = app.row_of_agent(3).expect("dd shown");
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert!(rx.try_recv().is_err(), "folding switches no agent");
}

#[test]
fn clicking_an_agents_marker_folds_it_and_its_name_opens_it() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_spawn_tree();
    let click = |app: &mut App, column, row| {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
    };

    // bb's marker sits after one space and one level of guides.
    click(&mut app, 6, 6);
    assert!(app.agent_folded(1));
    assert!(rx.try_recv().is_err(), "the marker switches no agent");
    click(&mut app, 8, 6);
    assert!(!app.agent_folded(1));

    // Past the marker the row switches to the agent as before.
    click(&mut app, 10, 6);
    assert!(!app.agent_folded(1));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    // A leaf's icon column is no button.
    click(&mut app, 10, 7);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "cc"));

    // A top-level agent's marker folds it rather than starting a drag.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 5));
    assert!(app.agent_folded(0));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 12));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 12));
    assert!(!app.project_dragging());
    assert_eq!(agent_ids(&app), ["aa", "bb", "cc", "dd", "ee"]);
    assert!(rx.try_recv().is_err());
}

#[test]
fn switching_to_a_hidden_subagent_opens_the_agents_above_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.toggle_agent(1);
    app.toggle_agent(0);
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 2 }
    );

    // Stepping onto one from the keys opens it too.
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.toggle_agent(0);
    app.switch_agent_by(1);
    assert!(!app.agent_folded(0));
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn a_folded_agents_hidden_active_spawn_parks_the_highlight_on_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    app.toggle_agent(1);
    app.select_visible_active();
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn set_agents_drops_folds_of_agents_gone_or_without_spawns() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.toggle_agent(1);
    app.toggle_agent(0);
    assert_eq!(app.collapsed_agents.len(), 2);

    let spawned = |id: &str| crate::events::AgentSummary {
        depth: 1,
        ..agent(id, id)
    };
    // cc is gone, so bb has nothing left to fold.
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("bb"),
        spawned("dd"),
        agent("ee", "EE"),
    ]));
    assert_eq!(
        app.collapsed_agents,
        HashSet::from(["aa".to_string()]),
        "aa still folds over bb and dd"
    );

    app.process_command(UiCommand::SetAgents(vec![agent("ee", "EE")]));
    assert!(app.collapsed_agents.is_empty());
}

#[test]
fn clicking_a_fold_marker_keeps_a_wheel_scrolled_list_put() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, _rx) = app_with_agent_areas();
    let spawned = |id: &str, depth: usize| crate::events::AgentSummary {
        depth,
        ..agent(id, id)
    };
    // Rows: header 0, aa 1, a1 2, a2 3, a3 4, bb 5, b1 6.
    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "AA"),
        spawned("a1", 1),
        spawned("a2", 1),
        spawned("a3", 1),
        spawned("bb", 1),
        spawned("b1", 2),
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.set_agent_rows_area(Rect::new(1, 4, 18, 3));
    app.sync_agents_scroll(3);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    app.sync_agents_scroll(3);
    assert_eq!(app.agents_scroll(), 3, "a2, a3 and bb on screen");

    // bb is on the third row; its marker after one level of guides.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 6, 6));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 6, 6));
    assert!(app.agent_folded(4));
    app.sync_agents_scroll(3);
    assert_eq!(app.agents_scroll(), 3);
}

#[test]
fn shift_space_folds_the_project_from_any_agent() {
    let (mut app, mut rx) = app_with_spawn_tree();
    app.focus = PanelFocus::Swarm;
    let shift_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT);
    for agent in [0, 1] {
        app.selected_agent = app.row_of_agent(agent).expect("shown");
        app.handle_key(shift_space);
        assert!(!app.agent_folded(agent), "the agent itself stays open");
        assert!(
            matches!(
                app.project_rows()[0],
                ProjectListRow::Project {
                    expanded: false,
                    ..
                }
            ),
            "agent {agent} folds its project"
        );
        // The highlight parks on the header, so Shift+Space opens it.
        app.handle_key(shift_space);
        assert_eq!(shown_agents(&app), [0, 1, 2, 3, 4]);
    }
    assert!(rx.try_recv().is_err(), "folding switches no agent");
}

#[test]
fn a_busy_agents_ring_is_its_fold_button() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_spawn_tree();
    let mut agents = app.agents().to_vec();
    agents[1].running = true;
    app.process_command(UiCommand::SetAgents(agents));
    let click = |app: &mut App, column| {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, 6));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, 6));
    };
    // bb's ring takes offsets 5 and 6 of the rows, starting at column 1,
    // and the spare column after it offset 7; no arrow is drawn in either
    // state, but all three fold and open it.
    click(&mut app, 6);
    assert!(app.agent_folded(1), "the ring folds an open agent");
    click(&mut app, 6);
    assert!(!app.agent_folded(1), "and opens a folded one");
    click(&mut app, 8);
    assert!(app.agent_folded(1));
    click(&mut app, 8);
    assert!(!app.agent_folded(1));
    assert!(rx.try_recv().is_err(), "the ring switches no agent");
    // Past the icon column the row still switches to the agent.
    click(&mut app, 10);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
}

#[test]
fn opening_a_fold_brings_a_parked_highlight_back_to_the_active_spawn() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    app.selected_agent = app.row_of_agent(2).expect("cc shown");
    app.toggle_agent(0);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 0 },
        "parked on aa"
    );
    app.toggle_agent(0);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 2 },
        "back on cc"
    );

    // With the active agent outside the fold, the highlight stays put.
    app.process_command(UiCommand::SetActiveAgent("ee".to_string()));
    app.selected_agent = app.row_of_agent(1).expect("bb shown");
    app.toggle_agent(1);
    app.toggle_agent(1);
    assert_eq!(
        app.project_rows()[app.selected_agent],
        ProjectListRow::Agent { index: 1 }
    );
}

#[test]
fn the_folded_agent_over_the_active_one_stands_for_it() {
    let (mut app, _rx) = app_with_spawn_tree();
    app.process_command(UiCommand::SetActiveAgent("cc".to_string()));
    assert_eq!(app.shown_active_agent(), Some(2));
    app.toggle_agent(1);
    assert_eq!(app.shown_active_agent(), Some(1));
    app.toggle_agent(0);
    assert_eq!(app.shown_active_agent(), Some(0), "the nearest one shown");
    app.toggle_selected_project();
    assert_eq!(app.shown_active_agent(), None, "under a folded project");
}

#[test]
fn clicking_the_earlier_lines_row_of_a_boot_console_loads_them() {
    use crate::components::{
        BootConsole, LIVE_BOOT_LOG_LINES, MAX_BOOT_LOG_LINES, MORE_PAGE_LINES,
    };
    use crate::events::{BootLogLine, BootUpdate};

    let mut console = BootConsole::new("boot-1", "Moving to the cloud");
    console.apply(BootUpdate::Log(
        (0..MAX_BOOT_LOG_LINES + 100)
            .map(|i| BootLogLine {
                stderr: false,
                text: format!("build line {i}"),
            })
            .collect(),
    ));
    let mut app = App::new();
    app.add_message(Message::boot(console));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let hidden = MAX_BOOT_LOG_LINES + 100 - LIVE_BOOT_LOG_LINES;
    let row = cell_of(&terminal, &format!("… {hidden} earlier lines"))
        .expect("the earlier lines row is on screen");

    click_at(&mut app, row);
    let shown = |app: &App| app.messages()[0].boot_console().unwrap().log_shown();
    assert_eq!(shown(&app), LIVE_BOOT_LOG_LINES + MORE_PAGE_LINES);

    // Past what memory holds, the rest comes back from the spill file.
    for _ in 0..3 {
        app.messages
            .back_mut()
            .unwrap()
            .show_more(crate::components::MoreSection::BootLog);
    }
    let console = app.messages()[0].boot_console().unwrap();
    assert_eq!(console.hidden(), 0);
    assert_eq!(
        console.earlier.front().map(|l| l.text.as_str()),
        Some("build line 0")
    );
}
