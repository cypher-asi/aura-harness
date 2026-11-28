use super::*;

/// The window point at the middle of cell `(col, row)` of the grid.
fn cell_point(stage: &Stage, col: u16, row: u16) -> Point {
    let (cell, area) = (stage.shell.metrics.cell, stage.shell.grid_area());
    let origin = stage.shell.metrics.origin(area, stage.shell.cols);
    let bar = tab_bar::height(&stage.shell.metrics);
    Point::new(
        origin.x + (f32::from(col) + 0.5) * cell.width,
        origin.y + (f32::from(row) + 0.5) * cell.height + bar,
    )
}

#[test]
fn the_wheel_over_the_agents_list_scrolls_it_and_switches_no_agent() {
    use aura_ui::events::AgentSummary;

    let mut stage = stage();
    let agents = (0..40)
        .map(|n| AgentSummary {
            id: format!("a{n:02}"),
            name: format!("Agent {n:02}"),
            zns_id: format!("0://terminal/a{n:02}"),
            ..AgentSummary::default()
        })
        .collect();
    stage.shell.commands([
        UiCommand::SetAgents(agents),
        UiCommand::SetActiveAgent("a00".to_string()),
    ]);
    stage.draw();
    let agents = {
        let tui = &stage.shell.tabs.front().tui;
        let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Agents);
        pane.expect("the agents panel has a bar").area
    };
    let (selected, scroll) = {
        let tui = &stage.shell.tabs.front().tui;
        (tui.selected_agent(), tui.agents_scroll())
    };

    let middle = cell_point(
        &stage,
        agents.x + agents.width / 2,
        agents.y + agents.height / 2,
    );
    stage.shell.pointer_moved(middle);
    for _ in 0..3 {
        stage
            .shell
            .wheel(MouseScrollDelta::LineDelta(0.0, -1.0), stage.now);
        stage.draw();
    }
    let tui = &stage.shell.tabs.front().tui;
    assert_eq!(tui.selected_agent(), selected, "the highlight stays");
    assert_eq!(tui.active_agent_id(), "a00", "no other agent opens");
    assert!(tui.agents_scroll() > scroll, "the list scrolled");
}
