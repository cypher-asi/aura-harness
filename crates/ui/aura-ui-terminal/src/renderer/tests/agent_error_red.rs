//! Every way an agent errors out reads in the Stop button's red: failing
//! to start, failing to get a cloud machine, a turn that ended in an
//! error and a stall. Its PROJECTS row and icon, its MEMBERS row and the
//! error in its chat are each drawn in exactly the color the Stop button
//! is drawn in.

use super::agent_failures::{cell_at, icon_before};
use super::*;
use crate::events::{AgentSummary, MessageData, MessageRole, UiCommand};

/// How each error state reads, as the harness words it.
const REASONS: [(&str, &str); 4] = [
    (
        "start",
        "Work could not start: its session could not be opened",
    ),
    (
        "provision",
        "Could not start on a cloud machine: the org has no room for one",
    ),
    ("turn", "Its turn failed: the model refused"),
    (
        "stall",
        "Stalled: it went idle without finishing. Its reports were given up.",
    ),
];

/// An agent whose work errored out `why`, busy with a turn of its own so
/// the Stop button shows, its chat showing the error as the harness does.
fn errored(why: &str) -> crate::App {
    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "bad".to_string(),
        name: "Broken agent".to_string(),
        error: Some(why.to_string()),
        ..AgentSummary::default()
    }]));
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::ShowError(why.to_string()));
    app
}

/// The first `n` words of `text`, which fit on a row.
fn words(text: &str, n: usize) -> String {
    text.split(' ').take(n).collect::<Vec<_>>().join(" ")
}

#[test]
fn every_error_state_reads_in_the_stop_buttons_red_on_its_row_icon_and_chat() {
    for (state, why) in REASONS {
        let mut app = errored(why);
        let buffer = render_buffer(&mut app, 160);
        let stop = cell_at(&buffer, "■ Stop").fg;
        assert_eq!(
            stop,
            crate::renderer::failure_color(&test_theme()),
            "{state}"
        );
        assert_eq!(
            cell_at(&buffer, "Broken agent").fg,
            stop,
            "{state}: PROJECTS row"
        );
        let icon = icon_before(&buffer, "Broken agent");
        assert_eq!(
            (icon.symbol(), icon.fg),
            ("!", stop),
            "{state}: PROJECTS icon"
        );
        let said = words(why, 3);
        let mark = icon_before(&buffer, &said);
        assert_eq!(
            (mark.symbol(), mark.fg),
            ("✗", stop),
            "{state}: chat prompt"
        );
        assert_eq!(cell_at(&buffer, &said).fg, stop, "{state}: chat reason");
    }
}

#[test]
fn every_error_state_reads_in_the_stop_buttons_red_on_its_members_row() {
    let stop = {
        let mut busy = errored(REASONS[0].1);
        cell_at(&render_buffer(&mut busy, 160), "■ Stop").fg
    };
    for (state, why) in REASONS {
        let mut app = super::agent_failures::members_with_failure_and_start();
        let mut channels = app.channels().to_vec();
        channels[0].members[1].error = Some(why.to_string());
        app.process_command(UiCommand::SetChannels(channels));
        let buffer = render_buffer(&mut app, 160);
        let row = format!("Failed: {}", words(why, 2));
        assert_eq!(cell_at(&buffer, &row).fg, stop, "{state}: MEMBERS row");
        let name_row = row_of(&buffer, &row) - 1;
        let icon = (0..buffer.area.width)
            .map(|x| &buffer[(x, name_row)])
            .find(|cell| cell.symbol() == "!")
            .expect("the failed member's icon");
        assert_eq!(icon.fg, stop, "{state}: MEMBERS icon");
    }
}

#[test]
fn a_plain_notice_stays_muted() {
    let mut app = app_with_model(None);
    app.process_command(UiCommand::ShowMessage(MessageData::new(
        MessageRole::System,
        "Now running in the cloud",
    )));
    let buffer = render_buffer(&mut app, 160);
    let muted = test_theme().colors.muted;
    assert_eq!(cell_at(&buffer, "Now running").fg, muted);
    assert_eq!(icon_before(&buffer, "Now running").symbol(), "*");
}
