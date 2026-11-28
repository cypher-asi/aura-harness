//! A running group of steps shows only its summary line until the user
//! opens it, for a local turn, a cloud turn and a turn the chat
//! reattached to; a click or `Ctrl+O` opens it with the steps and what
//! they printed, and the same folds it again.

use super::*;
use crate::events::{ActivityData, MessageData, MessageRole, ToolData, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a finished thought said and a finished command printed: the
/// previews an open running group shows.
const THOUGHT: &str = "claim lives in the fleet";
const PRINTED: &str = "fleet/claim.rs";

/// A thought, then a command, finished, as a turn streams them.
fn stream_steps(app: &mut crate::App, agent: Option<&str>) {
    let send = |app: &mut crate::App, command: UiCommand| match agent {
        Some(agent) => app.process_command(UiCommand::ForAgent {
            agent_id: agent.to_string(),
            command: Box::new(command),
        }),
        None => app.process_command(command),
    };
    send(app, UiCommand::StartThinking);
    send(
        app,
        UiCommand::AppendThinking(format!("one\ntwo\n{THOUGHT}")),
    );
    send(app, UiCommand::FinishThinking);
    for id in ["t1", "t2"] {
        send(
            app,
            UiCommand::ShowTool(ToolData {
                id: id.into(),
                name: "run_command".into(),
                args: r#"{"shell_script":"rg claim"}"#.into(),
            }),
        );
        send(
            app,
            UiCommand::CompleteTool {
                id: id.into(),
                result: format!("a\nb\n{PRINTED}"),
                success: true,
            },
        );
    }
}

/// The chat as text, and its running group's summary row.
fn shown(app: &mut crate::App) -> (String, Vec<String>) {
    let rows = render_rows(app, 120);
    (rows.join("\n"), rows)
}

/// Only the running group's summary: no step, no preview, no count row.
fn assert_folded(app: &mut crate::App) {
    let (text, rows) = shown(app);
    let summary: Vec<&String> = rows.iter().filter(|r| r.contains(" · step 3")).collect();
    assert_eq!(summary.len(), 1, "one summary line:\n{text}");
    assert!(!summary[0].contains('▾'), "folded:\n{text}");
    for hidden in [
        "Thought briefly",
        "Ran rg claim",
        THOUGHT,
        PRINTED,
        "3 steps",
    ] {
        assert!(!text.contains(hidden), "{hidden:?} shows:\n{text}");
    }
}

/// The running group with its numbered steps and their previews.
fn assert_open(app: &mut crate::App) {
    let (text, _) = shown(app);
    assert!(text.contains("1 ▸ Thought"), "{text}");
    assert!(text.contains("Ran rg claim"), "{text}");
    assert!(text.contains(THOUGHT), "{text}");
    assert!(text.contains(PRINTED), "{text}");
}

/// Click the running group's summary line.
fn click_summary(app: &mut crate::App) {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let buffer = render_buffer(app, 120);
    let row = row_of(&buffer, " · step 3");
    let (column, _) = find_cell(&buffer, row, "step 3").unwrap();
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
}

fn ctrl_o(app: &mut crate::App) {
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
}

#[test]
fn test_a_running_local_turn_shows_only_its_summary_until_clicked() {
    let mut app = crate::App::new();
    submit_message(&mut app, "where is claim");
    stream_steps(&mut app, None);
    assert_folded(&mut app);

    click_summary(&mut app);
    assert_open(&mut app);
    click_summary(&mut app);
    assert_folded(&mut app);
}

#[test]
fn test_ctrl_o_opens_and_folds_a_running_group() {
    let mut app = crate::App::new();
    submit_message(&mut app, "where is claim");
    stream_steps(&mut app, None);
    ctrl_o(&mut app);
    let (text, _) = shown(&mut app);
    assert!(
        text.contains("3 ▾ Ran rg claim"),
        "every step opens:\n{text}"
    );
    assert!(text.contains(PRINTED), "{text}");
    ctrl_o(&mut app);
    assert_folded(&mut app);
}

#[test]
fn test_a_running_cloud_turn_shows_only_its_summary() {
    // A cloud turn starts the chat's turn itself, with no typed prompt,
    // and streams its steps to the agent's chat.
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetActiveAgent("cl".to_string()));
    app.process_command(UiCommand::ForAgent {
        agent_id: "cl".to_string(),
        command: Box::new(UiCommand::TurnStarted { prompt: None }),
    });
    stream_steps(&mut app, Some("cl"));
    assert_folded(&mut app);
    click_summary(&mut app);
    assert_open(&mut app);
}

#[test]
fn test_a_reattached_cloud_turn_starts_folded() {
    // After a restart the chat loads the turn's steps from the record and
    // the turn goes on.
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetActiveAgent("cl".to_string()));
    let step = |activity| MessageData {
        activity: Some(activity),
        ..MessageData::new(MessageRole::System, String::new())
    };
    let tool = |id: &str| {
        step(ActivityData::Tool {
            id: id.to_string(),
            name: "run_command".to_string(),
            args: r#"{"shell_script":"rg claim"}"#.to_string(),
            result: Some(format!("a\nb\n{PRINTED}")),
            is_error: false,
            duration_ms: Some(10),
            policy: None,
        })
    };
    let history = vec![
        MessageData::new(MessageRole::User, "where is claim"),
        step(ActivityData::Thinking {
            text: format!("one\ntwo\n{THOUGHT}"),
            duration_ms: Some(10),
        }),
        tool("t1"),
        tool("t2"),
    ];
    for command in [
        UiCommand::LoadConversation(history),
        UiCommand::TurnStarted { prompt: None },
    ] {
        app.process_command(UiCommand::ForAgent {
            agent_id: "cl".to_string(),
            command: Box::new(command),
        });
    }
    assert_folded(&mut app);
    click_summary(&mut app);
    assert_open(&mut app);
}
