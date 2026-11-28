//! A cloud turn whose conversation was not copied here says so: in its
//! chat, as a muted note, and in the log.

use super::super::copy_note::{copy_failed_note, report_copy_failed};
use super::super::submit::copied_session;
use super::*;
use tokio::sync::mpsc;

fn chat() -> (AgentUi, mpsc::Receiver<UiCommand>) {
    let (tx, rx) = mpsc::channel(32);
    (AgentUi::new(tx, AgentId::new([7; 32])), rx)
}

#[tokio::test]
async fn a_failed_copy_sent_before_a_failure_is_kept_by_the_forward() {
    let (ui, _chat) = chat();
    let (tx, mut events) = mpsc::channel(8);
    tx.send(RemoteEvent::CopyFailed("node unreachable".into()))
        .await
        .unwrap();
    tx.send(RemoteEvent::Failed("boom".into())).await.unwrap();
    drop(tx);
    let mut extras = TurnExtras::default();
    let end = forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;
    assert_eq!(end, TurnEnd::Failed("boom".into()), "the turn still fails");
    assert_eq!(extras.copy_failed.as_deref(), Some("node unreachable"));
}

#[tokio::test]
async fn a_failed_copy_sent_after_the_turn_ended_is_kept_by_the_drain() {
    let (ui, _chat) = chat();
    let (tx, mut events) = mpsc::channel(8);
    tx.send(RemoteEvent::CopyFailed("timed out".into()))
        .await
        .unwrap();
    drop(tx);
    let mut copy_failed = None;
    let copied = copied_session(
        &ui,
        &mut events,
        None,
        AgentId::generate(),
        &mut copy_failed,
    )
    .await;
    assert!(copied.is_none());
    assert_eq!(copy_failed.as_deref(), Some("timed out"));
}

#[tokio::test]
async fn a_failed_copy_shows_a_muted_note_in_the_chat() {
    let (ui, mut chat) = chat();
    report_copy_failed(&ui, "RUS9", "timed out.").await;
    let mut notes = Vec::new();
    while let Ok(command) = chat.try_recv() {
        let command = match command {
            UiCommand::ForAgent { command, .. } => *command,
            other => other,
        };
        if let UiCommand::ShowMessage(message) = command {
            notes.push(message);
        }
    }
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].role, aura_ui::events::MessageRole::System, "muted");
    assert_eq!(notes[0].content, copy_failed_note("timed out"));
    assert!(
        notes[0].content.contains("(timed out)"),
        "{}",
        notes[0].content
    );
}
