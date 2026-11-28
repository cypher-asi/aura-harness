//! A member's row as its state changes: a start's step and time where an
//! old failure was, and the whole reason of a failure the row cuts off
//! under it while pointed at.

use super::channels::channels_app;
use super::*;
use crate::events::{AgentSummary, StartStep, UiCommand};
use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

/// The channels app with its member `r1` (`RUS2`) as `state` makes it.
fn channel_with(state: AgentSummary) -> crate::App {
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
            id: "r1".to_string(),
            name: "RUS2".to_string(),
            definition: Some("rust-engineer".to_string()),
            code: Some("RUS2".to_string()),
            ..state
        },
    ];
    app.process_command(UiCommand::SetChannels(channels));
    app.open_channel_by_id("c1");
    app
}

#[test]
fn a_starting_member_shows_its_step_and_for_how_long() {
    let since_ms = aura_core_types::time::now_ms() - 86_000;
    let mut app = channel_with(AgentSummary {
        starting: true,
        start_step: Some(StartStep {
            label: "Preparing project snapshot".to_string(),
            index: 5,
            total: 9,
            since_ms,
        }),
        ..AgentSummary::default()
    });
    let rows = render_rows(&mut app, 200);
    let screen = rows.join("\n");
    assert!(screen.contains("Starting 5/9 · 1m 2"), "{screen}");
    assert!(!screen.contains("Failed"), "{screen}");
    let mut wide = channel_with(AgentSummary {
        starting: true,
        ..AgentSummary::default()
    });
    let plain = render_rows(&mut wide, 200).join("\n");
    assert!(
        plain.contains("Starting in the cloud…"),
        "no step yet: {plain}"
    );
}

/// A move of the pointer onto `column`, `row`.
fn moved(column: usize, row: usize) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    }
}

#[test]
fn a_failure_the_row_cuts_off_shows_whole_under_the_pointer() {
    let why = "Could not start on a cloud machine: `cargo test --workspace --no-run` on \
               aura-proj-575d-build-fa8472 exited with 101: error[E0063]: missing field \
               `run_started` in initializer of `TurnExtras`";
    let mut app = channel_with(AgentSummary {
        error: Some(why.to_string()),
        ..AgentSummary::default()
    });
    let rows = render_rows(&mut app, 140);
    assert!(
        !rows.join("\n").contains("TurnExtras"),
        "the row cuts it off"
    );
    let (row, column) = rows
        .iter()
        .enumerate()
        .find_map(|(y, text)| {
            text.find("Failed: ")
                .map(|x| (y, text[..x].chars().count()))
        })
        .unwrap_or_else(|| panic!("no failure in:\n{}", rows.join("\n")));
    app.handle_mouse(moved(column, row));

    let pointed = render_rows(&mut app, 140).join("\n");
    assert!(pointed.contains("TurnExtras"), "{pointed}");
    assert!(pointed.contains("run_started"), "{pointed}");
}
