//! Something shown while a reply streams (an inbox line, a notice, an
//! error) never cuts the reply off: the rest of the reply keeps streaming
//! into it, and the line reads after the whole reply.

use super::*;
use crate::events::{MessageData, MessageRole as Role};

/// Stream a reply with `shown` arriving halfway, then end the turn as
/// `finish` does.
fn reply_with_line_halfway(shown: UiCommand, finish: &[UiCommand]) -> Vec<String> {
    let (mut app, _rx) = app_with_agents();
    let mut commands = vec![
        UiCommand::TurnStarted {
            prompt: Some("start".to_string()),
        },
        UiCommand::StartStreaming,
        UiCommand::AppendText(
            "It isn't something you asked for. Your messages sit on your dis".into(),
        ),
        shown,
        UiCommand::AppendText("k, sealed.\n\nThe second paragraph.".into()),
    ];
    commands.extend(finish.iter().cloned());
    for command in commands {
        app.process_command(command);
    }
    app.messages()
        .iter()
        .map(|m| m.content().to_string())
        .collect()
}

const FULL: &str = "It isn't something you asked for. Your messages sit on your disk, sealed.\n\nThe second paragraph.";

#[test]
fn an_inbox_line_mid_reply_reads_after_the_whole_reply() {
    let inbox = UiCommand::ShowMessage(MessageData::new(
        Role::System,
        "✉ RUS202 replied: Why members stall",
    ));
    for finish in [
        vec![UiCommand::FinishStreaming, UiCommand::Complete],
        vec![UiCommand::Complete],
    ] {
        let shown = reply_with_line_halfway(inbox.clone(), &finish);
        assert_eq!(
            shown,
            ["start", FULL, "✉ RUS202 replied: Why members stall"],
            "{finish:?}"
        );
    }
}

#[test]
fn an_error_or_notice_mid_reply_never_cuts_it() {
    let error = reply_with_line_halfway(
        UiCommand::ShowError("cloud turn failed".into()),
        &[UiCommand::FinishStreaming],
    );
    assert_eq!(error[1], FULL);
    assert_eq!(error.len(), 3);
    // A reply that streamed nothing still goes once it ends.
    let (mut app, _rx) = app_with_agents();
    for command in [
        UiCommand::StartStreaming,
        UiCommand::ShowMessage(MessageData::new(Role::System, "Background command ended")),
        UiCommand::FinishStreaming,
    ] {
        app.process_command(command);
    }
    let contents: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(contents, ["Background command ended"]);
}
