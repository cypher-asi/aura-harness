//! The MEMBERS rows: each member's permanent agent, with what it works
//! on in gray below.

use super::channels::channels_app;
use super::*;

/// Two instances of `rust-engineer`, one on a title and one working
/// without one, and a subagent of the first on its subtask.
fn members_app() -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let instance = |id: &str, code: &str, title: Option<&str>, running: bool| AgentSummary {
        id: id.to_string(),
        name: format!("{code}: {}", title.unwrap_or("rust-engineer")),
        definition: Some("rust-engineer".to_string()),
        code: Some(code.to_string()),
        title: title.map(str::to_string),
        running,
        ..AgentSummary::default()
    };
    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels[0].members = vec![
        AgentSummary {
            id: "co".to_string(),
            name: "Coordinator".to_string(),
            definition: Some("coordinator".to_string()),
            code: Some("CO".to_string()),
            ..AgentSummary::default()
        },
        instance("r1", "REN3", Some("Fix parser escapes"), false),
        AgentSummary {
            id: "sb".to_string(),
            name: "SB12: Write the escape tests".to_string(),
            code: Some("SB12".to_string()),
            subtitle: Some("Write the escape tests".to_string()),
            depth: 1,
            ..AgentSummary::default()
        },
        instance("r2", "REN4", None, true),
    ];
    app.process_command(UiCommand::SetChannels(channels));
    app.unfold_channel_members();
    app.open_channel_by_id("c1");
    app
}

#[test]
fn test_members_show_their_agent_and_what_each_instance_works_on_below() {
    let mut app = members_app();
    let rows = render_rows(&mut app, 140);
    let at = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
    };
    let coordinator = at("CO");
    assert!(rows[coordinator].contains("coordinator"));
    assert!(rows[coordinator + 1].contains("Idle"));
    // A member is marked `@` one column in from the border, as AGENTS
    // marks a permanent agent.
    assert!(
        rows[coordinator].contains("│ @  coordinator"),
        "{}",
        rows[coordinator]
    );

    // Each instance names the agent it is of, and its own work.
    let first = at("REN3");
    assert!(rows[first].contains("rust-engineer"));
    assert!(rows[first + 1].contains("Fix parser escapes"));
    let second = at("REN4");
    assert!(rows[second].contains("rust-engineer"));
    assert!(
        rows[second + 1].contains("Working…"),
        "{}",
        rows[second + 1]
    );
    // One with subagents shows `▾` over them, unfolded, in place of its
    // `@`; one at work shows the spinner.
    assert!(rows[first].contains("▾  rust-engineer"), "{}", rows[first]);
    assert!(!rows[second].contains('@'), "{}", rows[second]);

    // A subagent nests under its member, its subtask below its row, with
    // the line from its member where a member has its `@`.
    let subagent = at("SB12");
    assert_eq!(subagent, first + 2);
    assert!(
        rows[subagent].contains("└── subagent"),
        "{}",
        rows[subagent]
    );
    assert!(!rows[subagent].contains('@'), "{}", rows[subagent]);
    let column = |row: &str, needle: &str| row.find(needle).map(|x| row[..x].chars().count());
    assert_eq!(
        column(&rows[subagent + 1], "Write the escape"),
        column(&rows[subagent], "subagent"),
        "the subtask, cut to fit, lines up under the name"
    );
}

/// The cell `needle` starts at, on the first row with it.
fn cell_at<'a>(buffer: &'a ratatui::buffer::Buffer, needle: &str) -> &'a ratatui::buffer::Cell {
    let y = row_of(buffer, needle);
    let row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect();
    let x = row[..row.find(needle).unwrap()].chars().count();
    &buffer[(u16::try_from(x).unwrap(), y)]
}

#[test]
fn test_a_members_status_row_is_drawn_in_gray() {
    let mut app = members_app();
    let buffer = render_buffer(&mut app, 140);
    let muted = test_theme().colors.muted;
    assert_eq!(cell_at(&buffer, "Fix parser escapes").fg, muted);
    assert_ne!(cell_at(&buffer, "rust-engineer").fg, muted);
}

/// Put `agent_id` in the cloud, placed by `owner` when it is another
/// user's agent.
fn place_in_cloud(app: &mut crate::App, agent_id: &str, owner: Option<&str>) {
    use crate::host::{PlacementState, PlacementView};
    app.process_command(crate::events::UiCommand::SetPlacement {
        agent_id: agent_id.to_string(),
        state: PlacementState {
            placement: PlacementView::Cloud,
            owner: owner.map(str::to_string),
            ..PlacementState::default()
        },
    });
}

#[test]
fn test_a_cloud_member_shows_a_cloud_after_its_name() {
    let mut app = members_app();
    place_in_cloud(&mut app, "r1", None);
    place_in_cloud(&mut app, "r2", Some("Alice"));
    let rows = render_rows(&mut app, 140);
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
    };
    let own = row("REN3");
    assert!(own.contains("rust-engineer ☁"), "{own}");
    let foreign = row("REN4");
    // The name is cut to keep the owner in a narrow column.
    assert!(
        foreign.contains("rust-engine") && foreign.contains(" ☁  Alice"),
        "{foreign}"
    );
    let coordinator = row("CO ");
    assert!(!coordinator.contains('☁'), "{coordinator}");
    assert!(!row("SB12").contains('☁'), "a local subagent has no cloud");
}

#[test]
fn test_a_member_in_plan_mode_shows_the_plan_badge_and_keeps_its_cloud() {
    use crate::events::UiCommand;
    use crate::PermissionMode;
    let set_mode = |mode| UiCommand::ForAgent {
        agent_id: "r1".to_string(),
        command: Box::new(UiCommand::SetMode(mode)),
    };
    let mut app = members_app();
    place_in_cloud(&mut app, "r1", None);

    // The member's task starts planning, off screen.
    app.process_command(set_mode(PermissionMode::Plan));
    let rows = render_rows(&mut app, 140);
    let planning = rows.iter().find(|row| row.contains("REN3")).unwrap();
    // A cloud member plans first: its (P) must not hide where it runs.
    assert!(planning.contains("rust-engineer (P) ☁"), "{planning}");
    let buffer = render_buffer(&mut app, 140);
    assert_eq!(cell_at(&buffer, "(P)").fg, test_theme().colors.success);

    app.process_command(set_mode(PermissionMode::Agent));
    let rows = render_rows(&mut app, 140);
    let building = rows.iter().find(|row| row.contains("REN3")).unwrap();
    assert!(building.contains("rust-engineer ☁"), "{building}");
    assert!(!rows.iter().any(|row| row.contains("(P)")));
}

#[test]
fn test_another_users_cloud_is_dimmer_than_your_own() {
    let mut app = members_app();
    place_in_cloud(&mut app, "r1", None);
    place_in_cloud(&mut app, "r2", Some("Alice"));
    let buffer = render_buffer(&mut app, 140);
    let colors = test_theme().colors;
    assert_eq!(cell_at(&buffer, "☁  Alice").fg, colors.faint);
    let own = row_of(&buffer, "REN3");
    let cloud = (0..buffer.area.width)
        .map(|x| &buffer[(x, own)])
        .find(|cell| cell.symbol() == "☁")
        .expect("REN3's row has a cloud");
    assert_eq!(cloud.fg, colors.muted);
}
