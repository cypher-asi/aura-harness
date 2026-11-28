//! A cloud turn picked up after a restart: its chat and its row say
//! "Reconnecting…", never "Waking cloud machine", until the machine's
//! node is reached.

use super::*;
use crate::cloud::hub::{RECONNECTED, RECONNECTING};
use crate::cloud::RemoteEvent;
use crate::event_loop::agent_events::AgentUi;
use aura_engine_cloud::{BootEvent, BootStage, StageState};
use aura_ui::events::BootUpdate;
use std::time::Duration;

/// What `agent`'s chat was sent so far, unwrapped.
fn sent_to(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) -> Vec<UiCommand> {
    let mut out = Vec::new();
    while let Ok(command) = commands.try_recv() {
        if let UiCommand::ForAgent { agent_id, command } = command {
            if agent_id == agent.to_hex() {
                out.push(*command);
            }
        }
    }
    out
}

/// What `agent`'s chat was sent, collected until `done` holds of it or
/// five seconds pass; what came by then either way.
async fn sent_until(
    commands: &mut mpsc::Receiver<UiCommand>,
    agent: AgentId,
    done: impl Fn(&[UiCommand]) -> bool,
) -> Vec<UiCommand> {
    let mut shown = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !done(&shown) && tokio::time::Instant::now() < deadline {
        shown.extend(sent_to(commands, agent));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    shown
}

/// The title of every boot console begun in `commands`.
fn console_titles(commands: &[UiCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            UiCommand::Boot {
                update: BootUpdate::Begin { title },
                ..
            } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

/// Wait until `check` holds, for at most five seconds.
async fn until(check: impl Fn() -> bool, what: &str) {
    let waited = tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "never {what}");
}

#[tokio::test]
async fn a_picked_up_turn_says_reconnecting_in_its_chat_and_row_until_reached() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    let hub = Arc::clone(&fixture.hub);
    let (tx, mut rx) = mpsc::channel(8);
    let ui = AgentUi::new(fixture.commands_tx.clone(), member);
    let marked = Arc::clone(&hub);
    let turn = tokio::spawn(async move {
        crate::event_loop::cloud::forward::forward_marking_waits(marked, member, &ui, &mut rx)
            .await;
    });

    // The hub reconnects: the machine is up, only its node is claimed.
    tx.send(RemoteEvent::Status(RECONNECTING.into()))
        .await
        .unwrap();
    let claim = BootEvent::Stage {
        stage: BootStage::Claim,
        state: StageState::Started,
    };
    tx.send(RemoteEvent::Boot(claim)).await.unwrap();
    until(
        || hub.status.is_reconnecting(member),
        "reconnecting on its row",
    )
    .await;
    // The row and the chat are told separately: the chat's status may
    // still be on its way when the row has it.
    let shown = sent_until(&mut commands, member, |shown| {
        shown
            .iter()
            .any(|c| matches!(c, UiCommand::SetStatus(s) if s == RECONNECTING))
            && !console_titles(shown).is_empty()
    })
    .await;
    assert!(
        shown
            .iter()
            .any(|c| matches!(c, UiCommand::SetStatus(s) if s == RECONNECTING)),
        "its chat says it: {shown:?}"
    );
    assert_eq!(
        console_titles(&shown),
        [RECONNECTING],
        "not a wake: {shown:?}"
    );

    // Reached: its row follows the turn.
    tx.send(RemoteEvent::Status(RECONNECTED.into()))
        .await
        .unwrap();
    until(|| !hub.status.is_reconnecting(member), "reconnected").await;
    drop(tx);
    turn.await.unwrap();
    assert!(!hub.status.is_reconnecting(member));
}

#[tokio::test]
async fn a_machine_that_wakes_still_says_waking() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    let (tx, mut rx) = mpsc::channel(8);
    let ui = AgentUi::new(fixture.commands_tx.clone(), member);
    let marked = Arc::clone(&fixture.hub);
    let turn = tokio::spawn(async move {
        crate::event_loop::cloud::forward::forward_marking_waits(marked, member, &ui, &mut rx)
            .await;
    });
    let wake = BootEvent::Plan(vec![BootStage::Wake, BootStage::Claim]);
    tx.send(RemoteEvent::Boot(wake)).await.unwrap();
    drop(tx);
    turn.await.unwrap();
    let shown = sent_until(&mut commands, member, |shown| {
        !console_titles(shown).is_empty()
    })
    .await;
    assert_eq!(console_titles(&shown), ["Waking cloud machine"]);
}
