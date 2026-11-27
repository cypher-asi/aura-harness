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
    // One at work shows the spinner in place of its `@`.
    assert!(rows[first].contains("@  rust-engineer"), "{}", rows[first]);
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
