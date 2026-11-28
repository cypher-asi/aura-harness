//! Agents whose work failed show it in the Stop button's red, on their
//! PROJECTS row and their MEMBERS row, and a member starting on its cloud
//! machine says so instead of `Idle`.

use super::channels::channels_app;
use super::*;
use crate::events::{AgentSummary, UiCommand};

/// The cell `needle` starts at, on the first row with it.
pub(super) fn cell_at<'a>(
    buffer: &'a ratatui::buffer::Buffer,
    needle: &str,
) -> &'a ratatui::buffer::Cell {
    let y = row_of(buffer, needle);
    let row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect();
    let x = row[..row.find(needle).unwrap()].chars().count();
    &buffer[(u16::try_from(x).unwrap(), y)]
}

/// The cell left of the one `needle` starts at: its icon column, past
/// the gap between them.
pub(super) fn icon_before<'a>(
    buffer: &'a ratatui::buffer::Buffer,
    needle: &str,
) -> &'a ratatui::buffer::Cell {
    let y = row_of(buffer, needle);
    let row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect();
    let at = row[..row.find(needle).unwrap()].chars().count();
    let icon = (0..at)
        .rev()
        .find(|&x| {
            !buffer[(u16::try_from(x).unwrap(), y)]
                .symbol()
                .trim()
                .is_empty()
        })
        .unwrap();
    &buffer[(u16::try_from(icon).unwrap(), y)]
}

#[test]
fn a_failed_agent_shows_red_on_its_projects_row() {
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "ok".to_string(),
            name: "Steady agent".to_string(),
            ..AgentSummary::default()
        },
        AgentSummary {
            id: "bad".to_string(),
            name: "Broken agent".to_string(),
            error: Some("Its turn failed: the model refused".to_string()),
            ..AgentSummary::default()
        },
    ]));
    let buffer = render_buffer(&mut app, 140);
    let red = test_theme().colors.error;
    assert_eq!(cell_at(&buffer, "Broken agent").fg, red);
    let icon = icon_before(&buffer, "Broken agent");
    assert_eq!((icon.symbol(), icon.fg), ("!", red));
    assert_ne!(cell_at(&buffer, "Steady agent").fg, red);
}

/// The channels app with a member that failed and one starting in the
/// cloud.
pub(super) fn members_with_failure_and_start() -> crate::App {
    let member = |id: &str, code: &str| AgentSummary {
        id: id.to_string(),
        name: code.to_string(),
        definition: Some("rust-engineer".to_string()),
        code: Some(code.to_string()),
        ..AgentSummary::default()
    };
    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels[0].members = vec![
        AgentSummary {
            id: "co".to_string(),
            name: "Coordinator".to_string(),
            definition: Some("coordinator".to_string()),
            ..AgentSummary::default()
        },
        AgentSummary {
            error: Some("Could not start on a cloud machine: no boxd key".to_string()),
            title: Some("Footer links".to_string()),
            ..member("r1", "REN3")
        },
        AgentSummary {
            starting: true,
            ..member("r2", "REN4")
        },
    ];
    app.process_command(UiCommand::SetChannels(channels));
    app.open_channel_by_id("c1");
    app
}

#[test]
fn a_failed_member_says_why_in_red_and_a_starting_one_says_it_starts() {
    let mut app = members_with_failure_and_start();
    let buffer = render_buffer(&mut app, 160);
    let colors = &test_theme().colors;
    let failed = cell_at(&buffer, "Failed: Could");
    assert_eq!(failed.fg, colors.error, "red, over its title");
    let starting = cell_at(&buffer, "Starting in the cloud…");
    assert_eq!(starting.fg, colors.muted);
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    assert!(
        !rows.iter().any(|row| row.contains("Footer links")),
        "the failure shows in place of the title"
    );
}
