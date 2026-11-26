//! An agent's peer hooks without background agents behind them: what
//! they send, list and show, and the strangers they refuse.

use super::super::*;
use super::support::{code_of, new_channel, start, take_all};
use crate::event_loop::channels::create_channel;
use crate::event_loop::subagents::tests::{fixture, Fixture};
use tokio::sync::mpsc::UnboundedReceiver;

/// A link to `fixture`'s hub.
fn link(fixture: &Fixture) -> HubLink {
    let link = HubLink::default();
    link.set(&fixture.hub);
    link
}

/// `#site`'s coordinator and a member, with the member's hooks, which
/// fall back to no background agents, and the notices sent from then on.
struct Hooked {
    fixture: Fixture,
    notices: UnboundedReceiver<Notice>,
    coordinator: AgentId,
    member: AgentId,
    hooks: PeerHooks,
}

async fn member_hooks() -> Hooked {
    let mut fixture = fixture("unused");
    let mut notices = fixture.hub.take_notices();
    let channel = create_channel(&fixture.hub, new_channel("site", None))
        .await
        .unwrap();
    let coordinator = channel.coordinator;
    let member = start(&mut fixture, coordinator, "reviewer").await;
    take_all(&mut notices);
    let hooks = PeerHooks::new(link(&fixture), member, None);
    Hooked {
        fixture,
        notices,
        coordinator,
        member,
        hooks,
    }
}

#[tokio::test]
async fn peer_hooks_reach_peers_and_go_on_with_the_last_subject() {
    let mut hooked = member_hooks().await;
    let (coordinator, member) = (hooked.coordinator, hooked.member);
    hooked
        .hooks
        .deliver_message_in_thread(
            AgentMessage::new("coordinator", "The footer is done."),
            Some("Footer"),
        )
        .await
        .unwrap();
    let [told] = take_all(&mut hooked.notices).try_into().unwrap();
    assert_eq!(told.to, coordinator);
    assert!(told.prompt.contains(": Footer\n"), "{}", told.prompt);

    // Without a subject a message goes on with the last one, either way.
    PeerHooks::new(link(&hooked.fixture), coordinator, None)
        .deliver_message(AgentMessage::new(
            &code_of(&hooked.fixture, member),
            "Thanks.",
        ))
        .await
        .unwrap();
    let [answered] = take_all(&mut hooked.notices).try_into().unwrap();
    assert_eq!(answered.to, member);
    assert!(
        answered.prompt.starts_with(&format!(
            "{}: Footer\n",
            code_of(&hooked.fixture, coordinator)
        )),
        "{}",
        answered.prompt
    );
}

#[tokio::test]
async fn peer_hooks_refuse_strangers_without_a_host() {
    let hooked = member_hooks().await;
    let hooks = &hooked.hooks;
    let error = hooks
        .deliver_message(AgentMessage::new("NOPE9", "Hello"))
        .await
        .unwrap_err();
    assert!(error.contains("unknown agent 'NOPE9'"), "{error}");
    assert!(hooks
        .lifecycle("coordinator", None, None, "pause")
        .await
        .is_err());
    assert!(hooks.snapshot("NOPE9").await.is_err());
}

#[tokio::test]
async fn peer_hooks_list_the_agent_and_its_peers_and_show_a_peer() {
    let hooked = member_hooks().await;
    let (hooks, coordinator) = (&hooked.hooks, hooked.coordinator);
    let listed = hooks.list_agents(None).await.unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows[0]["is_you"], true);
    assert_eq!(rows[0]["agent_id"], hooked.member.to_hex());
    assert_eq!(rows[1]["agent_id"], coordinator.to_hex());
    assert_eq!(rows[1]["relation"], "coordinator");
    assert_eq!(rows[1]["working"], false);

    let state = hooks
        .snapshot(&code_of(&hooked.fixture, coordinator))
        .await
        .unwrap();
    assert_eq!(state["relation"], "coordinator");
}
