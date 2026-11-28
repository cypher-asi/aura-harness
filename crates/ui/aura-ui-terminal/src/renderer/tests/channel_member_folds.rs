//! The MEMBERS rows fold subagents: a `▸` over a folded member's
//! subagents, `▾` once they show, and a spinner while one folded away
//! works.

use super::channels::channels_app;
use super::*;

/// `#zode` with a member `re` whose subagent `sb` runs, folded.
fn folded_app(running: bool) -> crate::App {
    use crate::events::{AgentSummary, UiCommand};
    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels[0].members = vec![
        AgentSummary {
            id: "co".to_string(),
            name: "Coordinator".to_string(),
            ..AgentSummary::default()
        },
        AgentSummary {
            id: "re".to_string(),
            name: "Reviewer".to_string(),
            definition: Some("reviewer".to_string()),
            ..AgentSummary::default()
        },
        AgentSummary {
            id: "sb".to_string(),
            name: "Scout".to_string(),
            subtitle: Some("Read the parser".to_string()),
            depth: 1,
            running,
            ..AgentSummary::default()
        },
    ];
    app.process_command(UiCommand::SetChannels(channels));
    app.open_channel_by_id("c1");
    app
}

fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
}

#[test]
fn test_a_member_starts_folded_over_its_subagents_with_a_marker() {
    let mut app = folded_app(false);
    let rows = render_rows(&mut app, 140);
    assert!(row_with(&rows, "reviewer").contains("▸  reviewer"));
    assert!(!rows.iter().any(|row| row.contains("Read the parser")));
    assert!(row_with(&rows, "Coordinator").contains("@  Coordinator"));
}

#[test]
fn test_an_unfolded_member_shows_its_subagents_under_an_open_marker() {
    let mut app = folded_app(false);
    app.unfold_channel_members();
    let rows = render_rows(&mut app, 140);
    let member = rows.iter().position(|r| r.contains("reviewer")).unwrap();
    assert!(rows[member].contains("▾  reviewer"), "{}", rows[member]);
    assert!(
        rows[member + 2].contains("└── subagent"),
        "{}",
        rows[member + 2]
    );
    assert!(rows[member + 3].contains("Read the parser"));
}

#[test]
fn test_a_folded_member_spins_while_a_subagent_under_it_works() {
    let mut app = folded_app(true);
    let rows = render_rows(&mut app, 140);
    let row = row_with(&rows, "reviewer");
    assert!(
        !row.contains('▸'),
        "the spinner stands in for the marker: {row}"
    );
    assert!(!row.contains('@'), "{row}");
}
