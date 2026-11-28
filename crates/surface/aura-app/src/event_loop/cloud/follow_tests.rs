//! A cloud turn's chat shown from its record: a turn followed after Aura
//! restarted, or one whose stream was attached to again.

use super::forward::{forward, ChatReload, TurnEnd, TurnExtras};
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::mpsc;

fn chat() -> (AgentUi, mpsc::Receiver<UiCommand>) {
    let (tx, rx) = mpsc::channel(32);
    (AgentUi::new(tx, AgentId::new([7; 32])), rx)
}

fn commands(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<UiCommand> {
    let mut out = Vec::new();
    while let Ok(command) = rx.try_recv() {
        match command {
            UiCommand::ForAgent { command, .. } => out.push(*command),
            other => out.push(other),
        }
    }
    out
}

/// A reload that counts how often it ran.
fn counted() -> (ChatReload, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let reload: ChatReload = Box::new(move || {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
        })
    });
    (reload, count)
}

#[tokio::test]
async fn a_stream_that_shows_only_the_end_rebuilds_the_chat_on_each_copy() {
    let (ui, mut rx) = chat();
    let (tx, mut events) = mpsc::channel(8);
    for event in [
        RemoteEvent::Text("half a re".into()),
        RemoteEvent::Status("Reconnecting to the cloud machine".into()),
        RemoteEvent::Copied,
        RemoteEvent::Copied,
        RemoteEvent::Done(Box::default()),
    ] {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let (reload, reloads) = counted();
    let mut extras = TurnExtras {
        reload: Some(reload),
        ..TurnExtras::default()
    };

    let end = forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

    assert_eq!(end, TurnEnd::Done);
    assert_eq!(reloads.load(Ordering::SeqCst), 2, "once per copy");
    assert!(extras.from_record, "and once more when the hub is done");
    let sent = commands(&mut rx);
    assert!(
        sent.iter()
            .any(|command| matches!(command, UiCommand::FinishStreaming)),
        "the half reply is closed: {sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|command| matches!(command, UiCommand::SetContext(_))),
        "a turn that ended unwatched has no context to show: {sent:?}"
    );
    assert!(matches!(sent.last(), Some(UiCommand::Complete)), "{sent:?}");
}

#[tokio::test]
async fn a_turn_streamed_to_its_end_keeps_its_chat() {
    let (ui, _rx) = chat();
    let (tx, mut events) = mpsc::channel(8);
    tx.send(RemoteEvent::Text("all of it".into()))
        .await
        .unwrap();
    tx.send(RemoteEvent::Done(Box::new(
        aura_core_protocol::SessionUsage {
            estimated_context_tokens: 10,
            ..Default::default()
        },
    )))
    .await
    .unwrap();
    drop(tx);
    let (reload, reloads) = counted();
    let mut extras = TurnExtras {
        reload: Some(reload),
        ..TurnExtras::default()
    };

    forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

    assert_eq!(reloads.load(Ordering::SeqCst), 0);
    assert!(!extras.from_record);
}
