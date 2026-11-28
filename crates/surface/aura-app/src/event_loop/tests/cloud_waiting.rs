//! A cloud turn waiting for the one already under way on its machine:
//! its peers and its rows see it waiting, not working, until the turn
//! gets going or ends.

use super::*;
use crate::cloud::hub::WAITING;
use crate::cloud::RemoteEvent;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::standing::{state_of, WAITING_STATE};
use crate::event_loop::subagents::lock;
use std::time::Duration;

/// Wait until `agent`'s state is `want`, as its peers see it.
async fn until_state(hub: &AgentHub, agent: AgentId, want: &str) {
    let waited = tokio::time::timeout(Duration::from_secs(5), async {
        while state_of(hub, agent).state != want {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "never {want}: {:?}", state_of(hub, agent));
}

#[tokio::test]
async fn a_turn_waiting_on_its_busy_machine_shows_as_waiting_until_it_runs() {
    let (fixture, _commands) = fixture();
    let member = fixture.second;
    let hub = Arc::clone(&fixture.hub);
    // A cloud turn of its is under way, as submitting one notes.
    lock(&hub.cloud_turns).insert(member);
    assert_eq!(state_of(&hub, member).state, "working");

    let (tx, mut rx) = mpsc::channel(8);
    let ui = AgentUi::new(fixture.commands_tx.clone(), member);
    let marked = Arc::clone(&hub);
    let turn = tokio::spawn(async move {
        crate::event_loop::cloud::forward::forward_marking_waits(marked, member, &ui, &mut rx)
            .await;
    });

    tx.send(RemoteEvent::Status(WAITING.into())).await.unwrap();
    until_state(&hub, member, WAITING_STATE).await;
    let view = state_of(&hub, member);
    assert!(view.why.unwrap().contains("earlier turn"), "says why");
    assert!(hub.status.is_waiting(member), "its rows show it too");

    // The machine took the turn: it works.
    tx.send(RemoteEvent::Status("Working in the cloud".into()))
        .await
        .unwrap();
    until_state(&hub, member, "working").await;

    // Waiting again, then the turn ends: no wait is left behind.
    tx.send(RemoteEvent::Status(WAITING.into())).await.unwrap();
    until_state(&hub, member, WAITING_STATE).await;
    drop(tx);
    turn.await.unwrap();
    assert!(!hub.status.is_waiting(member));
}

#[tokio::test]
async fn a_failure_ends_the_wait() {
    let (fixture, _commands) = fixture();
    let member = fixture.second;
    let hub = &fixture.hub;
    assert!(hub.status.set_waiting(member, true));
    assert!(!hub.status.set_waiting(member, true), "already noted");
    hub.status.fail(
        member,
        "wait for the turn under way on its machine timed out",
    );
    assert!(!hub.status.is_waiting(member));
    assert_eq!(state_of(hub, member).state, "failed");
}
